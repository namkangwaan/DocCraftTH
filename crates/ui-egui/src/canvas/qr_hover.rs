//! QR codes on the pages: a per-page cache of the QR regions a page scan found, and the hit test
//! that picks the one under a point.
//!
//! A page is scanned the first time it is asked for (when the pointer is over it), once per
//! document generation: an edit makes every cached page stale. Natively the scan (a 150 DPI page
//! render and a QR search) runs on a worker thread and the frame that asks gets nothing until it
//! is done; the worker wakes the UI when it finishes. The web build has no threads, so there the
//! scan runs on the UI thread, once, the first time the page is hovered: one page, bounded by the
//! renderer's pixel caps, which is an acceptable one-off pause for the web app.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use pdfcraft_engine::Document;
use pdfcraft_engine::qr::QrRegion;
use pdfcraft_render::{DocInfo, PageInfo, RenderConfig};

/// The page scan: document bytes, render configuration, page index, page geometry.
type Scanner = Arc<dyn Fn(Arc<Vec<u8>>, RenderConfig, usize, PageInfo) -> Result<Vec<QrRegion>, String> + Send + Sync>;

/// What a scan answers with.
type ScanResult = Result<Vec<QrRegion>, String>;

/// The QR regions found on each page of one document.
pub(crate) struct QrCache {
    scanner: Scanner,
    /// Page index → its scan, for the generation it was made at.
    entries: HashMap<usize, Entry>,
}

/// One page's scan.
struct Entry {
    /// The document generation the scan was made at.
    generation: u64,
    state: State,
}

enum State {
    /// The scan is running; its result arrives here.
    Pending(Receiver<ScanResult>),
    /// The regions found (possibly none).
    Done(Vec<QrRegion>),
    /// The scan failed, panicked or could not start. Not retried until the document changes.
    Failed,
}

impl Entry {
    /// Take the scan's result if it has arrived. A worker that panicked drops its sender, so the
    /// channel closes and the page counts as failed.
    fn poll(&mut self) {
        if let State::Pending(receiver) = &self.state {
            self.state = match receiver.try_recv() {
                Ok(Ok(regions)) => State::Done(regions),
                Ok(Err(_)) | Err(TryRecvError::Disconnected) => State::Failed,
                Err(TryRecvError::Empty) => return,
            };
        }
    }

    fn is_pending(&self) -> bool {
        matches!(self.state, State::Pending(_))
    }
}

/// Wakes the UI when dropped, so a finished scan is picked up even if the scan panicked.
struct RepaintOnDrop(egui::Context);

impl Drop for RepaintOnDrop {
    fn drop(&mut self) {
        self.0.request_repaint();
    }
}

impl Default for QrCache {
    fn default() -> Self {
        Self::with_scanner(Arc::new(|bytes, config, page, info: PageInfo| pdfcraft_engine::qr::scan_page_qr_regions(bytes, config, page, &info)))
    }
}

impl QrCache {
    fn with_scanner(scanner: Scanner) -> Self {
        Self { scanner, entries: HashMap::new() }
    }

    /// The QR regions on `page` of `doc` (user space), as last scanned for the document's current
    /// generation. Empty while the scan runs, when it failed, and for a page `info` does not
    /// have. Starts at most one scan per page and generation, and only one at a time: a page
    /// asked for while another page's scan runs is scanned when asked for again after it.
    #[expect(dead_code, reason = "the hover frame and the context menu call it (QR hover tasks 3 and 4)")]
    pub(crate) fn regions(&mut self, doc: &Document, info: &DocInfo, page: usize, ctx: &egui::Context) -> &[QrRegion] {
        let Some(page_info) = info.pages.get(page) else { return &[] };
        self.regions_for(doc.edit_generation(), doc.display.clone(), page_info, page, ctx)
    }

    fn regions_for(&mut self, generation: u64, bytes: Arc<Vec<u8>>, info: &PageInfo, page: usize, ctx: &egui::Context) -> &[QrRegion] {
        // Scans made for another generation are stale; a late result of one is dropped with its receiver.
        self.entries.retain(|_, entry| entry.generation == generation);
        self.entries.values_mut().for_each(Entry::poll);
        if !self.entries.contains_key(&page) {
            if self.entries.values().any(Entry::is_pending) {
                return &[];
            }
            let state = self.start(bytes, info.clone(), page, ctx);
            self.entries.insert(page, Entry { generation, state });
        }
        match self.entries.get_mut(&page) {
            Some(entry) => {
                entry.poll();
                match &entry.state {
                    State::Done(regions) => regions,
                    State::Pending(_) | State::Failed => &[],
                }
            }
            None => &[],
        }
    }

    /// Start scanning `page`: on a worker thread natively, right here on the web.
    fn start(&self, bytes: Arc<Vec<u8>>, info: PageInfo, page: usize, ctx: &egui::Context) -> State {
        let (sender, receiver) = mpsc::channel();
        let scanner = self.scanner.clone();
        let wake = RepaintOnDrop(ctx.clone());
        let work = move || {
            let _wake = wake;
            let _ = sender.send(scanner(bytes, RenderConfig::default(), page, info));
        };
        #[cfg(not(target_arch = "wasm32"))]
        if std::thread::Builder::new().name("qr-scan".into()).spawn(work).is_err() {
            return State::Failed;
        }
        #[cfg(target_arch = "wasm32")]
        work();
        State::Pending(receiver)
    }

    #[cfg(test)]
    fn is_pending(&self, page: usize) -> bool {
        self.entries.get(&page).is_some_and(Entry::is_pending)
    }
}

/// The region under `at` (user space), edges included. Where regions overlap, the smallest wins:
/// it is the one the pointer is most specifically over. A NaN point is over none.
#[cfg_attr(not(test), expect(dead_code, reason = "the hover frame and the context menu call it (QR hover tasks 3 and 4)"))]
pub(crate) fn region_at(regions: &[QrRegion], at: [f64; 2]) -> Option<&QrRegion> {
    let [x, y] = at;
    let bounds = |r: &QrRegion| {
        let [a, b, c, d] = r.rect_user;
        (a.min(c), b.min(d), a.max(c), b.max(d))
    };
    let area = |r: &QrRegion| {
        let (x0, y0, x1, y1) = bounds(r);
        (x1 - x0) * (y1 - y0)
    };
    regions
        .iter()
        .filter(|r| {
            let (x0, y0, x1, y1) = bounds(r);
            x0 <= x && x <= x1 && y0 <= y && y <= y1
        })
        .min_by(|a, b| area(a).total_cmp(&area(b)))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    use super::*;

    fn region(content: &str, rect_user: [f64; 4]) -> QrRegion {
        QrRegion { content: content.to_string(), rect_user }
    }

    fn page_info() -> PageInfo {
        PageInfo { width: 300.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 0 }
    }

    /// A cache whose scan counts its calls and answers with `answer`.
    fn counting_cache(answer: impl Fn() -> Result<Vec<QrRegion>, String> + Send + Sync + 'static) -> (QrCache, Arc<AtomicUsize>) {
        let calls = Arc::new(AtomicUsize::new(0));
        let counter = calls.clone();
        let cache = QrCache::with_scanner(Arc::new(move |_bytes, _config, _page, _info| {
            counter.fetch_add(1, Ordering::SeqCst);
            answer()
        }));
        (cache, calls)
    }

    /// Ask for the regions until the scan has finished (or two seconds have passed).
    fn settled(cache: &mut QrCache, generation: u64, page: usize, ctx: &egui::Context) -> Vec<QrRegion> {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            let found = cache.regions_for(generation, Arc::new(Vec::new()), &page_info(), page, ctx).to_vec();
            if !cache.is_pending(page) || Instant::now() >= deadline {
                return found;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn region_at_inside_edge_outside() {
        let regions = [region("a", [10.0, 20.0, 50.0, 60.0])];
        assert_eq!(region_at(&regions, [30.0, 40.0]).map(|r| r.content.as_str()), Some("a"));
        assert_eq!(region_at(&regions, [10.0, 60.0]).map(|r| r.content.as_str()), Some("a"));
        assert_eq!(region_at(&regions, [50.0, 20.0]).map(|r| r.content.as_str()), Some("a"));
        assert!(region_at(&regions, [9.9, 40.0]).is_none());
        assert!(region_at(&regions, [30.0, 60.1]).is_none());
        assert!(region_at(&regions, [f64::NAN, 40.0]).is_none());
        assert!(region_at(&[], [30.0, 40.0]).is_none());
    }

    #[test]
    fn overlapping_regions_pick_smallest() {
        let regions = [region("big", [0.0, 0.0, 100.0, 100.0]), region("small", [40.0, 40.0, 60.0, 60.0]), region("mid", [30.0, 30.0, 70.0, 70.0])];
        assert_eq!(region_at(&regions, [50.0, 50.0]).map(|r| r.content.as_str()), Some("small"));
        assert_eq!(region_at(&regions, [35.0, 35.0]).map(|r| r.content.as_str()), Some("mid"));
        assert_eq!(region_at(&regions, [5.0, 5.0]).map(|r| r.content.as_str()), Some("big"));
    }

    #[test]
    fn cache_rescans_only_when_generation_changes() {
        let ctx = egui::Context::default();
        let (mut cache, calls) = counting_cache(|| Ok(vec![region("https://example.com", [1.0, 2.0, 3.0, 4.0])]));
        let found = settled(&mut cache, 1, 0, &ctx);
        assert_eq!(found.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // The same page at the same generation is answered from the cache.
        for _ in 0..5 {
            assert_eq!(cache.regions_for(1, Arc::new(Vec::new()), &page_info(), 0, &ctx).len(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // An edit makes the regions stale: one new scan.
        let found = settled(&mut cache, 2, 0, &ctx);
        assert_eq!(found.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        settled(&mut cache, 2, 0, &ctx);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn failed_scan_is_not_retried_for_the_same_generation() {
        let ctx = egui::Context::default();
        let (mut cache, calls) = counting_cache(|| Err("broken page".to_string()));
        assert!(settled(&mut cache, 7, 3, &ctx).is_empty());
        assert!(settled(&mut cache, 7, 3, &ctx).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        assert!(settled(&mut cache, 8, 3, &ctx).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[test]
    fn panicking_scan_counts_as_failed() {
        let ctx = egui::Context::default();
        let (mut cache, calls) = counting_cache(|| panic!("scanner bug"));
        assert!(settled(&mut cache, 1, 0, &ctx).is_empty());
        assert!(!cache.is_pending(0));
        assert!(settled(&mut cache, 1, 0, &ctx).is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn one_scan_in_flight_at_a_time() {
        let ctx = egui::Context::default();
        let (release, gate) = std::sync::mpsc::channel::<()>();
        let gate = std::sync::Mutex::new(gate);
        let (mut cache, calls) = counting_cache(move || {
            // Hold the first scan until the test lets it go.
            if let Ok(gate) = gate.lock() {
                let _ = gate.recv_timeout(Duration::from_secs(2));
            }
            Ok(Vec::new())
        });
        assert!(cache.regions_for(1, Arc::new(Vec::new()), &page_info(), 0, &ctx).is_empty());
        assert!(cache.is_pending(0));
        // Another page waits for the first scan rather than starting a second one.
        assert!(cache.regions_for(1, Arc::new(Vec::new()), &page_info(), 1, &ctx).is_empty());
        assert!(!cache.is_pending(1));
        // Let both scans through: page 1 starts once page 0 is done.
        let _ = release.send(());
        let _ = release.send(());
        settled(&mut cache, 1, 0, &ctx);
        settled(&mut cache, 1, 1, &ctx);
        assert!(!cache.is_pending(1));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }
}
