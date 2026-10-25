//! Scan & OCR ▸ Recognize text: render pages, read the words in them and add an invisible text
//! layer ([`Edit::AddOcrText`]), making scanned pages searchable and selectable.
//!
//! Recognition is slow (about a second a page), so it is split from the edit: [`Session::ocr_job`]
//! captures what it needs, [`OcrJob::run`] works anywhere (the UI runs it on a worker thread) and
//! [`Session::apply_ocr`] applies the result as one undoable step.

use std::sync::{Arc, Mutex};

use pdfcraft_render::{PageInfo, PageRenderer, RenderConfig, RenderRequest, RequestKind, text::unreadable};

pub use pdfcraft_ocr::{LANGUAGES, Models, Ocr, OcrError, PlacedWord};

use crate::{DocId, Edit, EditError, Session};

/// How recognised digits are written into the OCR text layer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Digits {
    /// Keep whatever numeral script the recogniser read.
    #[default]
    AsRead,
    /// Write ASCII digits `0..=9` as Thai digits `๐..=๙` (`U+0E50..=U+0E59`).
    Thai,
    /// Write Thai digits `๐..=๙` as ASCII digits `0..=9`.
    Arabic,
}

impl Digits {
    pub fn from_code(code: &str) -> Option<Self> {
        match code {
            "as_read" => Some(Self::AsRead),
            "thai" => Some(Self::Thai),
            "arabic" => Some(Self::Arabic),
            _ => None,
        }
    }

    pub fn code(self) -> &'static str {
        match self {
            Self::AsRead => "as_read",
            Self::Thai => "thai",
            Self::Arabic => "arabic",
        }
    }

    pub fn apply(self, text: &str) -> String {
        match self {
            Self::AsRead => text.to_string(),
            Self::Thai => {
                text.chars().map(|c| if c.is_ascii_digit() { char::from_u32(0x0E50 + (c as u32 - b'0' as u32)).unwrap_or(c) } else { c }).collect()
            }
            Self::Arabic => text
                .chars()
                .map(|c| if ('\u{0E50}'..='\u{0E59}').contains(&c) { char::from_u32(b'0' as u32 + (c as u32 - 0x0E50)).unwrap_or(c) } else { c })
                .collect(),
        }
    }
}

/// Recognize Text settings.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrSettings {
    /// Resolution pages are rendered at for recognition (Acrobat's "Downsample to" choices).
    pub dpi: f32,
    /// Language code from [`LANGUAGES`]: `"en"` is read with the Latin (ocrs) models and `"th"` with the Thai recognizer ([`engine_for`]).
    pub language: String,
    /// Leave pages that already have text alone (Acrobat reports "page contains renderable
    /// text" and skips them). When `false`, such pages are read too (a picture on a page that
    /// also has text, #1028) and only the words that aren't already there are added.
    pub skip_text_pages: bool,
    /// Read pages whose own text is unusable ([`OcrPage::unusable_text`]) even while
    /// `skip_text_pages` leaves the others alone: the user's confirmation of the suggestion
    /// (SRS FR-17), which never reaches a page whose text is fine.
    pub read_unusable_text: bool,
    /// How recognised digits are written into the text layer.
    pub digits: Digits,
    /// What counts as the page's text when `skip_text_pages` is on: only its own (page content
    /// and form fields). Comments and text boxes are left out, and are not drawn into the picture
    /// that is read, so a scan with a few comments on it is still a scan. `false`: everything
    /// the page shows.
    pub native_text_only: bool,
}

/// A recognised word whose box is at least this much covered by the page's own text is that
/// text read again, not a word in a picture.
const ALREADY_ON_PAGE: f32 = 0.5;

/// Why a page whose own text is unusable was skipped.
pub const UNUSABLE_TEXT: &str =
    "the page's text is unreadable (private-use or unmapped characters); recognise it with skip_text_pages off to make it searchable";

/// The long side of an A4 sheet in points (a letter page's, 792, is shorter): "Downsample to"
/// is a resolution on paper this size or smaller.
const SHEET: f32 = 842.0;

/// How much of `word` the `text` boxes cover, from 0 to 1. Boxes are `[x0, y0, x1, y1]` in page
/// view space. Overlaps between the text boxes are counted twice (glyphs rarely overlap), so the
/// sum is capped at the whole word.
fn covered(word: [f32; 4], text: &[[f32; 4]]) -> f32 {
    // `max(0.0)` also turns a NaN side into 0: a degenerate box covers nothing.
    let area = |[x0, y0, x1, y1]: [f32; 4]| (x1 - x0).max(0.0) * (y1 - y0).max(0.0);
    let total = area(word);
    if !total.is_finite() || total <= 0.0 {
        return 0.0;
    }
    let inside: f32 =
        text.iter().map(|g| area([word[0].max(g[0]), word[1].max(g[1]), word[2].min(g[2]), word[3].min(g[3])])).filter(|a| a.is_finite()).sum();
    (inside / total).min(1.0)
}

impl Default for OcrSettings {
    fn default() -> Self {
        OcrSettings {
            dpi: 300.0,
            language: "en".into(),
            skip_text_pages: true,
            read_unusable_text: false,
            digits: Digits::AsRead,
            native_text_only: false,
        }
    }
}

/// What recognition found on one page.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrPage {
    pub page: usize,
    pub words: Vec<PlacedWord>,
    /// Words recognised but left out because the page's own text already has them there.
    pub already_on_page: usize,
    /// Why the page was not read, if it was skipped.
    pub skipped: Option<String>,
    /// The page's own text is unusable ([`pdfcraft_render::PageText::is_unusable`]) and was treated as no text.
    pub unusable_text: bool,
}

impl OcrPage {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

/// The recogniser for `lang` (`"en"`, `"th"`, …), loaded once per language and shared.
pub fn engine_for(lang: &str) -> Result<Arc<Ocr>, String> {
    static OCR: Mutex<Option<std::collections::HashMap<String, Arc<Ocr>>>> = Mutex::new(None);
    let key = match lang {
        "th" | "tha" => "th",
        "en" | "eng" | "" => "en",
        other => other,
    };
    let mut guard = OCR.lock().unwrap_or_else(|e| e.into_inner());
    let map = guard.get_or_insert_with(std::collections::HashMap::new);
    if let Some(o) = map.get(key) {
        return Ok(o.clone());
    }
    let o = Arc::new(Ocr::load_for(key).map_err(|e| e.to_string())?);
    map.insert(key.to_string(), o.clone());
    Ok(o)
}

/// The default (English/Latin) recogniser, loaded once and shared.
pub fn engine() -> Result<Arc<Ocr>, String> {
    engine_for("en")
}

/// Whether the recognition models for `lang` are installed.
pub fn available_for(lang: &str) -> bool {
    Models::find_for(lang).is_some()
}

/// Whether the default (English/Latin) recognition models are installed.
pub fn available() -> bool {
    available_for("en")
}

/// Everything recognition needs from a document, detached from the session.
pub struct OcrJob {
    bytes: Arc<Vec<u8>>,
    password: Option<String>,
    infos: Vec<PageInfo>,
    /// Per page: the render scale (pixels per point) that shows its images at their own
    /// resolution, if it has any.
    native: Vec<Option<f32>>,
    pub pages: Vec<usize>,
    pub settings: OcrSettings,
}

impl OcrJob {
    /// Read the pages. `progress(done, total)` is called before each page; returning `false`
    /// stops (the pages read so far are returned).
    pub fn run(self, ocr: &Ocr, mut progress: impl FnMut(usize, usize) -> bool) -> Vec<OcrPage> {
        let config =
            RenderConfig { password: self.password.as_deref().map(Arc::from), hide_comments: self.settings.native_text_only, ..Default::default() };
        let mut r = PageRenderer::new(self.bytes.clone(), config);
        let mut out = Vec::new();
        let total = self.pages.len();
        for (done, &page) in self.pages.iter().enumerate() {
            if !progress(done, total) {
                break;
            }
            let Some(info) = self.infos.get(page) else { continue };
            let own = r.render(RenderRequest { page, kind: RequestKind::Text, scale: 1.0, ..Default::default() }).text;
            let unusable_text = own.as_ref().is_some_and(|t| t.is_unusable());
            let skip = |why: &str| OcrPage { page, words: Vec::new(), already_on_page: 0, skipped: Some(why.into()), unusable_text };
            // The boxes of the page's own readable text (an earlier OCR layer's included), in view
            // space. Unreadable glyphs don't count: their boxes would hide the words read over them.
            let existing: Vec<[f32; 4]> = own
                .map(|t| t.glyphs.iter().filter(|g| g.text.chars().any(|c| !c.is_whitespace() && !unreadable(c))).map(|g| g.rect).collect())
                .unwrap_or_default();
            // The user confirms replacing unusable text (SRS D11) by reading the page again with
            // `skip_text_pages` off, or with `read_unusable_text`, which reads only such pages.
            let read_over = unusable_text && self.settings.read_unusable_text;
            if self.settings.skip_text_pages && unusable_text && !read_over {
                out.push(skip(UNUSABLE_TEXT));
                continue;
            }
            if self.settings.skip_text_pages && !read_over && !existing.is_empty() {
                out.push(skip("the page already contains text"));
                continue;
            }
            let shot = r.render(RenderRequest { page, scale: self.scale(page, info), ..Default::default() });
            if let Some(e) = shot.error {
                out.push(skip(&e));
                continue;
            }
            let lines = match ocr.recognize(&shot.rgba, shot.width, shot.height) {
                Ok(l) => l,
                Err(e) => {
                    out.push(skip(&e.to_string()));
                    continue;
                }
            };
            let scale = shot.width as f32 / info.width.max(1e-3);
            let to_user = |x: f32, y: f32| {
                let [u, v] = info.view_to_user(x / scale, y / scale);
                [u as f64, v as f64]
            };
            // Words the page's own text already has are not added a second time.
            let (fresh, already): (Vec<_>, Vec<_>) =
                lines.iter().flat_map(|l| &l.words).partition(|w| covered(w.rect.map(|v| v / scale), &existing) < ALREADY_ON_PAGE);
            let words = fresh
                .into_iter()
                .map(|w| {
                    let mut placed = PlacedWord::place(w, to_user);
                    placed.text = self.settings.digits.apply(&placed.text);
                    placed
                })
                .collect();
            out.push(OcrPage { page, words, already_on_page: already.len(), skipped: None, unusable_text });
        }
        progress(total, total);
        out
    }

    /// The render scale (pixels per point) `page` is read at. A scan is read at its own
    /// resolution (enlarging it only blurs the letters), at most the chosen one.
    fn scale(&self, page: usize, info: &PageInfo) -> f32 {
        // A NaN resolution (OcrSettings is public) made `wanted` NaN, and `native.clamp(fit, NaN)`
        // below panics: read it at the default resolution instead.
        let dpi = if self.settings.dpi.is_nan() { OcrSettings::default().dpi } else { self.settings.dpi };
        let wanted = dpi.clamp(72.0, 600.0) / 72.0;
        let Some(native) = self.native.get(page).copied().flatten() else { return wanted };
        // A scan whose page is larger than a sheet (often a box given in pixels, one point per
        // pixel, #1848) is measured as if shrunk to one: otherwise 1 px/pt is already its full
        // image, and no "Downsample to" choice could ask for less. Tolerate a page a hair over a
        // sheet (up to 1 %, e.g. 2493 x 3510 px at 300 dpi = 842.4 pt) at its own resolution.
        let long = info.width.max(info.height).max(1e-3);
        let fit = if long > SHEET * 1.01 { SHEET / long } else { 1.0 };
        native.clamp(fit, wanted * fit)
    }
}

impl Session {
    /// Capture what recognising `pages` (0-based; empty = all) of document `id` needs.
    pub fn ocr_job(&self, id: DocId, pages: &[usize], settings: OcrSettings) -> Option<OcrJob> {
        let doc = self.get(id)?;
        let all = doc.info.pages.len();
        let pages: Vec<usize> = if pages.is_empty() { (0..all).collect() } else { pages.iter().copied().filter(|p| *p < all).collect() };
        let native = (0..all)
            .map(|p| {
                if !pages.contains(&p) {
                    return None;
                }
                doc.page_images(p)
                    .iter()
                    .filter(|i| i.width > 1 && i.height > 1)
                    .map(|i| {
                        let m = i.matrix;
                        let (w, h) = (m[0].hypot(m[1]), m[2].hypot(m[3]));
                        (i.width as f64 / w.max(1e-6)).max(i.height as f64 / h.max(1e-6)) as f32
                    })
                    .reduce(f32::max)
            })
            .collect();
        Some(OcrJob { bytes: doc.bytes.clone(), password: doc.password.clone(), infos: doc.info.pages.clone(), native, pages, settings })
    }

    /// Add the words found by [`OcrJob::run`] as one undoable step. Returns the number of words.
    pub fn apply_ocr(&mut self, id: DocId, found: &[OcrPage]) -> Result<usize, EditError> {
        let edits: Vec<Edit> =
            found.iter().filter(|p| !p.words.is_empty()).map(|p| Edit::AddOcrText { page: p.page, words: p.words.clone() }).collect();
        let words = found.iter().map(|p| p.words.len()).sum();
        if !edits.is_empty() {
            self.apply(id, Edit::Batch { label: "Recognize text".into(), edits })?;
        }
        Ok(words)
    }

    /// Recognise text on `pages` and add it, in one call (the CLI and agents use this).
    pub fn recognize_text(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) -> Result<Vec<OcrPage>, String> {
        let lang = settings.language.clone();
        let job = self.ocr_job(id, pages, settings).ok_or("no such document")?;
        if let Some(why) = self.get(id).and_then(|d| d.read_only_reason.clone()) {
            return Err(why);
        }
        let ocr = engine_for(&lang)?;
        let found = job.run(&ocr, |_, _| true);
        self.apply_ocr(id, &found).map_err(|e| e.to_string())?;
        Ok(found)
    }
}

/// What [`recognize_file`] did to one file.
#[derive(Clone, Debug, PartialEq)]
pub struct FileResult {
    /// The new file (incrementally saved), or the original when nothing was recognised.
    pub bytes: Arc<Vec<u8>>,
    pub pages: Vec<OcrPage>,
}

impl FileResult {
    pub fn words(&self) -> usize {
        self.pages.iter().map(|p| p.words.len()).sum()
    }
}

/// Recognize text in multiple files: one PDF's bytes in, the searchable PDF out. `progress`
/// works as for [`OcrJob::run`].
pub fn recognize_file(
    name: &str,
    bytes: Arc<Vec<u8>>,
    password: Option<&str>,
    settings: OcrSettings,
    ocr: &Ocr,
    progress: impl FnMut(usize, usize) -> bool,
) -> Result<FileResult, String> {
    let mut s = Session::new();
    let id = s.open(name, None, bytes.clone(), password).map_err(|e| e.to_string())?;
    if let Some(why) = s.get(id).and_then(|d| d.read_only_reason.clone()) {
        return Err(why);
    }
    let job = s.ocr_job(id, &[], settings).ok_or("the document could not be read")?;
    let pages = job.run(ocr, progress);
    if s.apply_ocr(id, &pages).map_err(|e| e.to_string())? == 0 {
        return Ok(FileResult { bytes, pages });
    }
    // Checked like a save before the caller writes it (#2166).
    let bytes = s.save_bytes(id).map_err(|e| e.to_string())?;
    s.check_save(id, &bytes).map_err(|e| e.to_string())?;
    Ok(FileResult { bytes, pages })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::export;

    /// #1028: a recognised word over the page's own text is that text again; one in an empty
    /// part of the page (a picture) is new.
    #[test]
    fn words_over_existing_text_count_as_already_there() {
        // Glyph em boxes of "Total" at 12 pt, five letters side by side.
        let glyphs: Vec<[f32; 4]> = (0..5).map(|i| [100.0 + 7.0 * i as f32, 50.0, 107.0 + 7.0 * i as f32, 62.0]).collect();
        // OCR's tight box around the same ink.
        assert!(covered([100.5, 52.0, 134.0, 61.0], &glyphs) >= ALREADY_ON_PAGE);
        // A word in a picture further down, and one beside the text on the same line.
        assert_eq!(covered([100.0, 200.0, 140.0, 212.0], &glyphs), 0.0);
        assert_eq!(covered([140.0, 52.0, 170.0, 61.0], &glyphs), 0.0);
        // Touching the text's last letter is not enough.
        assert!(covered([131.0, 52.0, 170.0, 61.0], &glyphs) < ALREADY_ON_PAGE);
        // A page without text has nothing to cover a word.
        assert_eq!(covered([100.5, 52.0, 134.0, 61.0], &[]), 0.0);
    }

    /// A resolution that is not a number reads at the default one instead of panicking in
    /// `f32::clamp`, on a scan (which has a native resolution) and on a page without images.
    #[test]
    fn a_nan_resolution_reads_at_the_default() {
        let mut s = Session::new();
        let png = export::encode_png(612, 792, &vec![255; 612 * 792 * 4]).unwrap();
        let bytes = s.create_from_images(&[("scan.png".into(), png)]).unwrap();
        let scan = s.open("scan.pdf", None, bytes, None).unwrap();
        let blank = s.create_blank(612.0, 792.0, 1).unwrap();
        let blank = s.open("blank.pdf", None, blank, None).unwrap();
        for id in [scan, blank] {
            let default = s.ocr_job(id, &[], OcrSettings::default()).unwrap();
            for dpi in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                let job = s.ocr_job(id, &[], OcrSettings { dpi, ..Default::default() }).unwrap();
                let scale = job.scale(0, &job.infos[0]);
                assert!(scale.is_finite() && scale > 0.0, "{dpi}: {scale}");
                if dpi.is_nan() {
                    assert_eq!(scale, default.scale(0, &default.infos[0]));
                }
            }
        }
    }

    /// #1848: an A4 scan at 300 dpi whose page box is its pixel size (2480 x 3508 pt, one pixel
    /// per point) gets a smaller bitmap for each lower "Downsample to" choice, and a low-resolution
    /// letter page is still never shrunk below one pixel per point.
    #[test]
    fn downsample_shrinks_a_page_box_given_in_pixels() {
        let mut s = Session::new();
        let page = |w: u32, h: u32| export::encode_png(w, h, &vec![255; (w * h * 4) as usize]).unwrap();
        let bytes = s.create_from_images(&[("a4.png".into(), page(2480, 3508)), ("letter.png".into(), page(612, 792))]).unwrap();
        let id = s.open("scan.pdf", None, bytes, None).unwrap();
        let mut r = PageRenderer::new(s.get(id).unwrap().bytes.clone(), RenderConfig::default());
        for (dpi, a4) in [(72.0, [595, 842]), (150.0, [1240, 1754]), (300.0, [2480, 3508]), (600.0, [2480, 3508])] {
            let job = s.ocr_job(id, &[], OcrSettings { dpi, ..Default::default() }).unwrap();
            for (page, want) in [(0, a4), (1, [612, 792])] {
                let scale = job.scale(page, &job.infos[page]);
                let shot = r.render(RenderRequest { page, scale, ..Default::default() });
                assert!(shot.error.is_none(), "{:?}", shot.error);
                let near = |got: u32, want: u32| got.abs_diff(want) <= 1;
                assert!(near(shot.width, want[0]) && near(shot.height, want[1]), "{dpi} dpi, page {page}: {}x{}", shot.width, shot.height);
            }
        }
    }

    /// A 300 dpi A4 scan whose page box is a hair taller than a sheet (2493 x 3510 px on
    /// 598.32 x 842.4 pt) is read at its own 300 dpi, not shrunk to 299.86 dpi (a 1 px smaller
    /// image that changed what the Thai recognizer read).
    #[test]
    fn a_page_a_hair_over_a_sheet_is_read_at_its_own_resolution() {
        let mut s = Session::new();
        let png = export::encode_png(2493, 3510, &vec![255; 2493 * 3510 * 4]).unwrap();
        let bytes = s.create_from_images_with_resolution(&[("a4.png".into(), png)], crate::ImageResolution::Dpi(300.0)).unwrap();
        let id = s.open("scan.pdf", None, bytes, None).unwrap();
        let job = s.ocr_job(id, &[], OcrSettings::default()).unwrap();
        let info = &job.infos[0];
        assert!((info.height - 842.4).abs() < 0.01, "page height {}", info.height);
        let scale = job.scale(0, info);
        assert!((scale - 300.0 / 72.0).abs() < 1e-4, "read at {} dpi", scale * 72.0);
    }

    #[test]
    fn coverage_of_overlapping_or_broken_boxes_stays_between_zero_and_one() {
        // A glyph drawn twice (fake bold) does not cover more than the whole word.
        let twice = [[0.0, 0.0, 10.0, 10.0], [0.0, 0.0, 10.0, 10.0]];
        assert_eq!(covered([0.0, 0.0, 10.0, 10.0], &twice), 1.0);
        // Empty, inverted and non-finite boxes cover nothing and are covered by nothing.
        for word in [[5.0, 5.0, 5.0, 9.0], [9.0, 0.0, 1.0, 9.0], [f32::NAN, 0.0, 9.0, 9.0], [0.0, 0.0, f32::INFINITY, 9.0]] {
            assert_eq!(covered(word, &twice), 0.0, "{word:?}");
        }
        let broken = [[f32::NAN; 4], [f32::NEG_INFINITY, f32::NEG_INFINITY, f32::INFINITY, f32::INFINITY]];
        let c = covered([0.0, 0.0, 10.0, 10.0], &broken);
        assert!((0.0..=1.0).contains(&c), "{c}");
    }

    #[test]
    fn digits_option_converts_between_thai_and_arabic_numerals() {
        for (code, d) in [("as_read", Digits::AsRead), ("thai", Digits::Thai), ("arabic", Digits::Arabic)] {
            assert_eq!(Digits::from_code(code), Some(d));
            assert_eq!(d.code(), code);
        }
        assert_eq!(Digits::from_code("roman"), None);
        assert_eq!(Digits::AsRead.apply("พ.ศ. 2569 / ๒๕๖๙ (0123456789)"), "พ.ศ. 2569 / ๒๕๖๙ (0123456789)");
        assert_eq!(Digits::Thai.apply("พ.ศ. 2569 / ๒๕๖๙ (0123456789)"), "พ.ศ. ๒๕๖๙ / ๒๕๖๙ (๐๑๒๓๔๕๖๗๘๙)");
        assert_eq!(Digits::Arabic.apply("พ.ศ. 2569 / ๒๕๖๙ (๐๑๒๓๔๕๖๗๘๙)"), "พ.ศ. 2569 / 2569 (0123456789)");
    }
}
