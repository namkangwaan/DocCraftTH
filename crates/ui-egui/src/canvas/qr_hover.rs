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

/// The hover frame's colour until the user picks another (Preferences ▸ Appearance): green.
pub(crate) const DEFAULT_QR_HIGHLIGHT: [u8; 3] = [0x22, 0xC5, 0x5E];

/// Characters of the payload the hover tip shows before it cuts it off with `…`.
const TIP_CHARS: usize = 64;

/// The hover tip for a QR code carrying `content`: the payload (cut at a character boundary)
/// and what a right-click offers for it.
pub(crate) fn tip_for(content: &str) -> String {
    let preview = pdfcraft_engine::qr::truncate_chars(content, TIP_CHARS);
    if pdfcraft_engine::qr::is_url(content) {
        let what = crate::i18n::fmt(tl!("QR Code link: {url}"), &[("url", &preview)]);
        format!("{what}\n({})", tl!("Right-click to open in a browser or copy"))
    } else {
        let what = crate::i18n::fmt(tl!("QR Code text: {text}"), &[("text", &preview)]);
        format!("{what}\n({})", tl!("Right-click to copy or check"))
    }
}

/// Frame `rect` (screen) as the QR code under the pointer: a rounded outline in `color` over a
/// light fill of it.
pub(crate) fn paint_frame(painter: &egui::Painter, rect: egui::Rect, color: egui::Color32) {
    let rect = rect.expand(2.0);
    let radius = egui::CornerRadius::same(4);
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    painter.rect_filled(rect, radius, egui::Color32::from_rgba_unmultiplied(r, g, b, 30));
    painter.rect_stroke(rect, radius, egui::Stroke::new(2.5, color), egui::StrokeKind::Outside);
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
    fn tip_for_thai_text_is_char_safe() {
        // 3 bytes a character: a byte cut at 61 or 64 would land inside one.
        let payload = "ทดสอบภาษาไทย".repeat(20);
        let tip = tip_for(&payload);
        let first = tip.lines().next().unwrap_or_default();
        assert!(first.ends_with('…'), "{tip}");
        assert!(first.contains(&pdfcraft_engine::qr::truncate_chars(&payload, TIP_CHARS)), "{tip}");
        assert!(!tip.contains(&payload), "the whole payload is cut short: {tip}");
        assert!(tip.contains("Right-click"), "{tip}");
        // A short payload is shown whole.
        assert!(tip_for("สวัสดี").lines().next().is_some_and(|l| l.ends_with("สวัสดี")));
    }

    #[test]
    fn tip_for_url_mentions_right_click() {
        let url = "https://example.com/menu?table=7";
        let tip = tip_for(url);
        assert!(tip.contains(url), "{tip}");
        assert!(tip.contains("Right-click to open in a browser or copy"), "{tip}");
        // In Thai the hint keeps its wording.
        let th = crate::i18n::Lang::from_code("th").expect("th registered");
        crate::i18n::set_current(th);
        let thai = tip_for(url);
        crate::i18n::set_current(crate::i18n::Lang::EN);
        let thai = crate::i18n::th_breaks::without_soft_breaks(&thai).into_owned();
        assert!(thai.contains(url) && thai.contains("คลิกขวาเพื่อเปิดในเบราว์เซอร์หรือคัดลอก"), "{thai}");
        let text = crate::i18n::th_breaks::without_soft_breaks(&{
            crate::i18n::set_current(th);
            let tip = tip_for("table 7");
            crate::i18n::set_current(crate::i18n::Lang::EN);
            tip
        })
        .into_owned();
        assert!(text.contains("table 7") && text.contains("คลิกขวาเพื่อคัดลอกหรือตรวจสอบ"), "{text}");
    }

    /// On a page turned a quarter (/Rotate 90), the frame drawn for a region lands on the page
    /// where that region shows, and the pointer at the frame's centre is over that region.
    #[test]
    fn rotated_page_frame_lands_on_the_region() {
        use crate::canvas::PageXform;
        // A portrait crop box turned 90°: shown 400 wide and 300 high.
        let page = PageInfo { width: 400.0, height: 300.0, label: String::new(), crop: [0.0, 0.0, 300.0, 400.0], rotation: 90 };
        let info = pdfcraft_render::DocInfo { pages: vec![page.clone()], ..Default::default() };
        let page_rect = egui::Rect::from_min_size(egui::pos2(100.0, 50.0), egui::vec2(400.0, 300.0));
        let xf = PageXform { rect: page_rect, rot: 0, pw: 400.0, ph: 300.0 };
        let regions = [region("https://example.com", [20.0, 30.0, 80.0, 90.0])];
        let frame = xf.user_rect_f64(&info, 0, regions[0].rect_user);
        assert!(page_rect.contains_rect(frame), "{frame:?} outside {page_rect:?}");
        assert!((frame.width() - 60.0).abs() < 0.01 && (frame.height() - 60.0).abs() < 0.01, "{frame:?}");
        // /Rotate 90 puts the bottom-left of user space at the top-left of the page as shown.
        assert!((frame.min.x - 130.0).abs() < 0.01 && (frame.min.y - 70.0).abs() < 0.01, "{frame:?}");
        let (vx, vy) = xf.screen_to_view(frame.center());
        let [ux, uy] = page.view_to_user(vx, vy);
        assert_eq!(region_at(&regions, [f64::from(ux), f64::from(uy)]).map(|r| r.content.as_str()), Some("https://example.com"));
    }

    #[test]
    fn highlight_colour_persists_and_rejects_nonsense() {
        let mut app = crate::PdfCraftApp::default();
        assert_eq!(app.comment_prefs.qr_highlight_color, DEFAULT_QR_HIGHLIGHT);
        app.comment_prefs.qr_highlight_color = [0x12, 0x34, 0x56];
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.comment_prefs.qr_highlight_color, [0x12, 0x34, 0x56]);
        for bad in [r#"{"qr_highlight_color":[300,0,0]}"#, r#"{"qr_highlight_color":"x"}"#, r#"{"qr_highlight_color":[1,2]}"#] {
            let mut app = crate::PdfCraftApp::default();
            app.restore(bad);
            assert_eq!(app.comment_prefs.qr_highlight_color, DEFAULT_QR_HIGHLIGHT, "{bad}");
        }
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
