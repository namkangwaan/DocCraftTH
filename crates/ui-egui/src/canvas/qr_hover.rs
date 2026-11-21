//! QR codes on the pages: a per-page cache of the QR regions a page scan found, and the hit test
//! that picks the one under a point.
//!
//! A page is scanned once the pointer has rested on it for [`DWELL`] seconds, once per document
//! state: an edit or a change to what the page shows (layers, hidden comments) makes every cached
//! page stale. Scrolling past pages, or passing over one on the way elsewhere, scans nothing.
//! Natively the scan (a 150 DPI page render and a QR search) runs on a worker thread and the frame
//! that asks gets nothing until it is done; the worker wakes the UI when it finishes. The web
//! build has no threads, so there the scan runs on the UI thread: one page, bounded by the
//! renderer's pixel caps, after the pointer has stopped on it — a one-off pause for the web app.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Duration;

use pdfcraft_engine::Document;
use pdfcraft_engine::qr::QrRegion;
use pdfcraft_render::{DocInfo, PageInfo, ReadingColors, RenderConfig};

/// The page scan: document bytes, render configuration, page index, page geometry.
type Scanner = Arc<dyn Fn(Arc<Vec<u8>>, RenderConfig, usize, PageInfo) -> Result<Vec<QrRegion>, String> + Send + Sync>;

/// What a scan answers with.
type ScanResult = Result<Vec<QrRegion>, String>;

/// Seconds the pointer rests on a page before the page is scanned.
const DWELL: f64 = 0.4;

/// Seconds between two asks for the same page beyond which the pointer is taken to have left it in
/// between, so the rest starts again. While resting, the cache asks for a repaint when the
/// [`DWELL`] ends, so a pointer that stays is asked about well within this.
const DWELL_GAP: f64 = 0.75;

/// What a scan depends on besides the page: the document's generation (every edit, and the view
/// settings that bump it) and what it draws (layer visibility and hidden comments, which change
/// without a new generation).
#[derive(Clone, Debug, PartialEq)]
struct ScanKey {
    generation: u64,
    layers: Arc<Vec<(i32, i32, bool)>>,
    hide_comments: bool,
}

/// The QR regions found on each page of one document.
pub(crate) struct QrCache {
    scanner: Scanner,
    /// Page index → its scan, for the document state it was made at.
    entries: HashMap<usize, Entry>,
    /// The page the pointer is resting on, before its scan starts.
    wait: Option<Wait>,
}

/// One page's scan.
struct Entry {
    /// The document state the scan was made at.
    key: ScanKey,
    state: State,
}

/// The pointer's rest on a page not yet scanned.
struct Wait {
    page: usize,
    key: ScanKey,
    /// When the rest began, and when the page was last asked for (seconds, egui's clock).
    since: f64,
    last: f64,
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

/// The configuration a page is scanned with: the document's own (its password, layer visibility
/// and hidden comments, so a QR code the page doesn't show is never found), drawn in the
/// document's colours on an opaque page and without installed fonts, as for recognition. Reading
/// colours are left out: inverted or tinted paints would defeat the decoder.
fn scan_config(config: &RenderConfig) -> RenderConfig {
    RenderConfig { reading_colors: ReadingColors::default(), transparent: false, system_fonts: false, ..config.clone() }
}

impl QrCache {
    fn with_scanner(scanner: Scanner) -> Self {
        Self { scanner, entries: HashMap::new(), wait: None }
    }

    /// The QR regions on `page` of `doc` (user space), as last scanned for the document's current
    /// state. Empty until the pointer has rested on the page for [`DWELL`] seconds, while the
    /// scan runs, when it failed, and for a page `info` does not have. Starts at most one scan
    /// per page and document state.
    pub(crate) fn regions(&mut self, doc: &Document, info: &DocInfo, page: usize, ctx: &egui::Context) -> &[QrRegion] {
        let now = ctx.input(|i| i.time);
        self.regions_at(doc, info, page, now, ctx)
    }

    /// [`Self::regions`] at the time `now` (seconds, egui's clock).
    fn regions_at(&mut self, doc: &Document, info: &DocInfo, page: usize, now: f64, ctx: &egui::Context) -> &[QrRegion] {
        let Some(page_info) = info.pages.get(page) else { return &[] };
        let config = doc.render_config();
        let key = ScanKey { generation: doc.edit_generation(), layers: config.layers.clone(), hide_comments: config.hide_comments };
        self.regions_for(key, (doc.display.clone(), scan_config(config)), page_info, page, now, ctx)
    }

    /// One scan in flight at a time, for the current document state: a page asked for while
    /// another page's scan runs is scanned when asked for again after it. A change of state drops
    /// the pending entry, but its worker still finishes in the background, holding that state's
    /// bytes until it does.
    fn regions_for(
        &mut self,
        key: ScanKey,
        source: (Arc<Vec<u8>>, RenderConfig),
        info: &PageInfo,
        page: usize,
        now: f64,
        ctx: &egui::Context,
    ) -> &[QrRegion] {
        // Scans made for another document state are stale; a late result of one is dropped with its receiver.
        self.entries.retain(|_, entry| entry.key == key);
        self.entries.values_mut().for_each(Entry::poll);
        if !self.entries.contains_key(&page) {
            let rested = self.rested(&key, page, now, ctx);
            if !rested || self.entries.values().any(Entry::is_pending) {
                return &[];
            }
            self.wait = None;
            let (bytes, config) = source;
            let state = self.start(bytes, config, info.clone(), page, ctx);
            self.entries.insert(page, Entry { key, state });
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

    /// Whether the pointer has rested on `page` for [`DWELL`] seconds at `now`. A rest starts
    /// again when another page or document state is asked for, or when the page was not asked
    /// for within [`DWELL_GAP`] (the pointer left it in between, or the clock went back). Until
    /// the rest is long enough, asks for a repaint when it will be.
    fn rested(&mut self, key: &ScanKey, page: usize, now: f64, ctx: &egui::Context) -> bool {
        let fresh = self.wait.as_ref().is_none_or(|w| w.page != page || w.key != *key || !(0.0..=DWELL_GAP).contains(&(now - w.last)));
        if fresh {
            self.wait = Some(Wait { page, key: key.clone(), since: now, last: now });
        }
        let Some(wait) = self.wait.as_mut() else { return false };
        wait.last = now;
        let waited = now - wait.since;
        if waited >= DWELL {
            return true;
        }
        let left = Duration::try_from_secs_f64(DWELL - waited).unwrap_or(Duration::from_secs_f64(DWELL));
        ctx.request_repaint_after(left);
        false
    }

    /// Start scanning `page`: on a worker thread natively, right here on the web.
    fn start(&self, bytes: Arc<Vec<u8>>, config: RenderConfig, info: PageInfo, page: usize, ctx: &egui::Context) -> State {
        let (sender, receiver) = mpsc::channel();
        let scanner = self.scanner.clone();
        let wake = RepaintOnDrop(ctx.clone());
        let work = move || {
            let _wake = wake;
            // Declared after the guard so it drops first: the UI is woken with the channel closed.
            let sender = sender;
            let _ = sender.send(scanner(bytes, config, page, info));
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
/// it is the one the pointer is most specifically over. A NaN point is over none, and a region
/// with a non-finite corner is under none.
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
        .filter(|r| r.rect_user.iter().all(|v| v.is_finite()))
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

/// The hover tip for a QR code carrying `content`: the payload on one line (without characters
/// that hide or reorder text, cut at a character boundary) and what a right-click offers for it.
pub(crate) fn tip_for(content: &str) -> String {
    let preview = pdfcraft_engine::qr::preview_text(content, TIP_CHARS);
    if matches!(menu_for(content), QrMenu::Link(_)) {
        let what = crate::i18n::fmt(tl!("QR Code link: {url}"), &[("url", &preview)]);
        format!("{what}\n({})", tl!("Right-click to open in a browser or copy"))
    } else {
        let what = crate::i18n::fmt(tl!("QR Code text: {text}"), &[("text", &preview)]);
        format!("{what}\n({})", tl!("Right-click to copy or check"))
    }
}

/// What the page's right-click menu offers for a QR code.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum QrMenu {
    /// A web address, ready for a browser (`www.` gets `https://`): open it or copy it.
    Link(String),
    /// Any other payload, unchanged: copy it.
    Text(String),
}

/// The right-click menu choice for a QR code carrying `content`: a link only for a web address
/// that passes the check every address from a document passes (`links::document_url`: no hidden
/// or bidi characters, no controls, not overlong, a well-formed web or email address), so what
/// the menu shows is what opens. Anything else can only be copied.
pub(crate) fn menu_for(content: &str) -> QrMenu {
    let link = pdfcraft_engine::qr::is_url(content)
        .then(|| pdfcraft_engine::links::document_url(&pdfcraft_engine::qr::normalize_url(content)).ok())
        .flatten();
    match link {
        Some(url) => QrMenu::Link(url),
        None => QrMenu::Text(content.to_string()),
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

    fn key(generation: u64) -> ScanKey {
        ScanKey { generation, layers: Arc::default(), hide_comments: false }
    }

    /// Ask for `page`'s regions at `now` (seconds).
    fn ask(cache: &mut QrCache, key: ScanKey, page: usize, now: f64, ctx: &egui::Context) -> Vec<QrRegion> {
        cache.regions_for(key, (Arc::new(Vec::new()), RenderConfig::default()), &page_info(), page, now, ctx).to_vec()
    }

    /// Rest on the page, then ask for the regions until the scan has finished (or two seconds
    /// have passed). The clock steps a tenth of a second an ask, as a pointer at rest is asked.
    fn settled_key(cache: &mut QrCache, key: ScanKey, page: usize, ctx: &egui::Context) -> Vec<QrRegion> {
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut now = 0.0;
        loop {
            let found = ask(cache, key.clone(), page, now, ctx);
            let started = cache.entries.contains_key(&page);
            if (started && !cache.is_pending(page)) || Instant::now() >= deadline {
                return found;
            }
            now += 0.1;
            if started {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    }

    fn settled(cache: &mut QrCache, generation: u64, page: usize, ctx: &egui::Context) -> Vec<QrRegion> {
        settled_key(cache, key(generation), page, ctx)
    }

    /// Whether `page` has a scan (running or finished), and how many scans ran, once any running one is done.
    fn scans(cache: &mut QrCache, calls: &AtomicUsize, page: usize) -> (bool, usize) {
        let deadline = Instant::now() + Duration::from_secs(2);
        while cache.is_pending(page) && Instant::now() < deadline {
            cache.entries.values_mut().for_each(Entry::poll);
            std::thread::sleep(Duration::from_millis(5));
        }
        (cache.entries.contains_key(&page), calls.load(Ordering::SeqCst))
    }

    #[test]
    fn page_is_scanned_only_after_the_pointer_rests_on_it() {
        let ctx = egui::Context::default();
        let (mut cache, calls) = counting_cache(|| Ok(Vec::new()));
        // Passing over the page: asked once, nothing starts.
        assert!(ask(&mut cache, key(1), 0, 10.0, &ctx).is_empty());
        assert_eq!(scans(&mut cache, &calls, 0), (false, 0));
        assert!(ask(&mut cache, key(1), 0, 10.2, &ctx).is_empty());
        assert_eq!(scans(&mut cache, &calls, 0), (false, 0));
        // Rested long enough: exactly one scan.
        ask(&mut cache, key(1), 0, 10.45, &ctx);
        assert_eq!(scans(&mut cache, &calls, 0), (true, 1));
        ask(&mut cache, key(1), 0, 10.6, &ctx);
        assert_eq!(scans(&mut cache, &calls, 0), (true, 1));

        // Scrolling on to another page starts the rest again.
        let (mut cache, calls) = counting_cache(|| Ok(Vec::new()));
        ask(&mut cache, key(1), 0, 0.0, &ctx);
        ask(&mut cache, key(1), 1, 0.3, &ctx);
        ask(&mut cache, key(1), 0, 0.5, &ctx);
        assert_eq!(scans(&mut cache, &calls, 0), (false, 0));
        ask(&mut cache, key(1), 0, 0.95, &ctx);
        assert_eq!(scans(&mut cache, &calls, 0), (true, 1));
        assert_eq!(scans(&mut cache, &calls, 1), (false, 1));

        // Leaving the page and coming back long after starts it again too.
        let (mut cache, calls) = counting_cache(|| Ok(Vec::new()));
        ask(&mut cache, key(1), 0, 0.0, &ctx);
        ask(&mut cache, key(1), 0, 2.0, &ctx);
        assert_eq!(scans(&mut cache, &calls, 0), (false, 0));
        ask(&mut cache, key(1), 0, 2.45, &ctx);
        assert_eq!(scans(&mut cache, &calls, 0), (true, 1));
    }

    #[test]
    fn scan_uses_the_documents_view_settings_but_its_own_colours() {
        use pdfcraft_render::ReadingColors;
        let mut app = crate::PdfCraftApp::default();
        app.open_bytes("one.pdf", None, crate::canvas::tests::text_pages_pdf(&["Alpha"])).expect("opens");
        let id = app.views[0].id;
        assert!(app.session.set_hide_comments(id, true));
        assert!(app.session.set_reading_colors(id, ReadingColors::WHITE_ON_BLACK));
        let doc = app.session.get(id).expect("open");
        let seen = Arc::new(std::sync::Mutex::new(None::<RenderConfig>));
        let sink = seen.clone();
        let mut cache = QrCache::with_scanner(Arc::new(move |_bytes, config, _page, _info| {
            if let Ok(mut s) = sink.lock() {
                *s = Some(config);
            }
            Ok(Vec::new())
        }));
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut now = 0.0;
        while seen.lock().map(|s| s.is_none()).unwrap_or(false) && Instant::now() < deadline {
            cache.regions_at(doc, &doc.info, 0, now, &ctx);
            now += 0.1;
            std::thread::sleep(Duration::from_millis(5));
        }
        let config = seen.lock().ok().and_then(|s| s.clone()).expect("scanned");
        assert!(config.hide_comments, "hidden comments stay hidden for the scan");
        assert_eq!(config.reading_colors, ReadingColors::default(), "the scan sees the document's own colours");
        assert!(!config.system_fonts);
    }

    #[test]
    fn scan_config_keeps_password_and_layers() {
        let doc_config = RenderConfig {
            password: Some(Arc::from("secret")),
            layers: Arc::new(vec![(12, 0, false)]),
            hide_comments: true,
            reading_colors: pdfcraft_render::ReadingColors::YELLOW_ON_BLACK,
            system_fonts: true,
            ..RenderConfig::default()
        };
        let c = scan_config(&doc_config);
        assert_eq!(c.password.as_deref(), Some("secret"));
        assert_eq!(*c.layers, vec![(12, 0, false)]);
        assert!(c.hide_comments && !c.system_fonts && !c.transparent);
        assert_eq!(c.reading_colors, pdfcraft_render::ReadingColors::default());
    }

    #[test]
    fn hidden_characters_never_make_an_open_link() {
        for payload in ["https://good.example\u{202E}gpj.exe", "https://a.example\nb", "https://a.example/\u{200B}x"] {
            assert_eq!(menu_for(payload), QrMenu::Text(payload.to_string()), "{payload:?}");
            let tip = tip_for(payload);
            assert_eq!(tip.lines().count(), 2, "{tip:?}");
            let first = tip.lines().next().unwrap_or_default();
            assert!(!first.contains(['\u{202E}', '\u{200B}']), "{first:?}");
            assert!(first.contains("Right-click") || tip.contains("Right-click to copy or check"), "{tip:?}");
        }
        assert!(tip_for("https://a.example\nb").lines().next().is_some_and(|l| l.contains("https://a.example b")));
        // A trailing newline is only whitespace around the address.
        assert_eq!(menu_for("https://a.example/\n"), QrMenu::Link("https://a.example/".into()));
    }

    #[test]
    fn qr_link_goes_through_the_document_link_prompt() {
        let mut app = crate::PdfCraftApp::default();
        app.request_document_url("https://qr.test/2", crate::LinkOrigin::QrCode);
        assert_eq!(app.pending_link.as_ref().map(|p| (p.url.as_str(), p.origin)), Some(("https://qr.test/2", crate::LinkOrigin::QrCode)));
        let th = crate::i18n::Lang::from_code("th").expect("th registered");
        assert!(crate::i18n::has(th, crate::LinkOrigin::QrCode.noun()));
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
    fn region_at_ignores_non_finite_regions() {
        let regions = [region("inf", [0.0, 0.0, f64::INFINITY, 100.0]), region("nan", [0.0, f64::NAN, 100.0, 100.0])];
        assert!(region_at(&regions, [50.0, 50.0]).is_none());
        // A finite region next to them is still found.
        let regions = [region("inf", [f64::NEG_INFINITY, 0.0, 100.0, 100.0]), region("ok", [40.0, 40.0, 60.0, 60.0])];
        assert_eq!(region_at(&regions, [5.0, 5.0]).map(|r| r.content.as_str()), None);
        assert_eq!(region_at(&regions, [50.0, 50.0]).map(|r| r.content.as_str()), Some("ok"));
    }

    #[test]
    fn menu_choice_for_url_and_text() {
        assert_eq!(menu_for("https://example.com/menu?table=7"), QrMenu::Link("https://example.com/menu?table=7".into()));
        assert_eq!(menu_for("www.example.com"), QrMenu::Link("https://www.example.com".into()));
        assert_eq!(menu_for("สวัสดีครับ"), QrMenu::Text("สวัสดีครับ".into()));
        assert_eq!(menu_for("table 7"), QrMenu::Text("table 7".into()));
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
            assert_eq!(ask(&mut cache, key(1), 0, 5.0, &ctx).len(), 1);
        }
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        // An edit makes the regions stale: one new scan.
        let found = settled(&mut cache, 2, 0, &ctx);
        assert_eq!(found.len(), 1);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        settled(&mut cache, 2, 0, &ctx);
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        // Turning a layer off changes what the page shows without a new generation: one new scan.
        let hidden_layer = ScanKey { layers: Arc::new(vec![(12, 0, false)]), ..key(2) };
        settled_key(&mut cache, hidden_layer.clone(), 0, &ctx);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        settled_key(&mut cache, hidden_layer, 0, &ctx);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
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
        ask(&mut cache, key(1), 0, 0.0, &ctx);
        assert!(ask(&mut cache, key(1), 0, 0.5, &ctx).is_empty());
        assert!(cache.is_pending(0));
        // Another page waits for the first scan rather than starting a second one.
        assert!(ask(&mut cache, key(1), 1, 0.6, &ctx).is_empty());
        assert!(ask(&mut cache, key(1), 1, 1.1, &ctx).is_empty());
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
