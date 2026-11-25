//! Scan & OCR ▸ Recognize text: the Recognize Text dialog (pages, language, output, resolution)
//! and the background run with its progress. Recognition happens on a worker thread; the result
//! is applied as one undoable edit when it is done.

use std::sync::Arc;

use craft::pages::ui::{PageRangeChoices, PageRangeOptions};
use craft::pages::{PageRange, PageScope};
use craft::ui::buttons::Button;
use craft::ui::contract::{Bind, Show as _, clicked};
use craft::ui::display::Text;
use craft::ui::patterns::form_dialogs::{DialogFields, HostedParamDialog, Param, ParamDialog, ParamKind};
use craft::ui::runtime::env::Label;
use pdfcraft_engine::DocId;
<<<<<<< ours
use pdfcraft_engine::ocr::{DeskewMode, Digits, EngineChoice, LANGUAGES, MergeStrategy, OcrError, OcrPage, OcrSettings, OutputMode};
=======
use pdfcraft_engine::ocr::{EngineChoice, LANGUAGES, MergeStrategy, OcrGuard, OcrPage, OcrSettings, OutputMode};
>>>>>>> theirs

use super::job_count::JobCount;
// PLAN: assumed `crate::notices::{set_progress, end_progress, ProgressKey, ProgressNotice}` from pdf-chrome-w19 (free functions taking the app).
use crate::PdfCraftApp;
use crate::notices::{self, ProgressKey, ProgressNotice};

/// Which pages to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrPages {
    All,
    Current,
    Range,
}

/// The dialog's choices (kept between runs, as in Acrobat). The Recognize Text dialog, the OCR
/// Verify screen and the batch options all read and write this one draft, and it is what the
/// preferences persist: language, output mode, the four preprocessing flags, engine, strategy
/// and the three skip flags — never review edits, verified flags or the selection.
#[derive(Clone, Debug, PartialEq)]
pub struct OcrDraft {
    pub pages: OcrPages,
    pub from: usize,
    pub to: usize,
    /// The language the user picked; `None` follows the interface language ([`Self::language`]).
    pub language: Option<String>,
    /// "Downsample to" resolution.
    pub dpi: u32,
    /// Also recognize pages that already have text (pictures on them; their text isn't added again).
    pub text_pages: bool,
    /// Whether recognized digits are kept as read or converted to Thai or Arabic numerals.
    pub digits: Digits,
    /// Also read pages whose own text is unusable (SRS FR-17): the user's tick, or the browsing
    /// notice's for such a page. Used for one run only, so it never carries over to the next.
    pub read_unusable_text: bool,
    /// Which engine reads the pages.
    pub engine: EngineChoice,
    /// How two engines' readings combine.
    pub strategy: MergeStrategy,
    /// Preprocessing flags: orientation check and deskew are on by default (every OCR run checks them first).
    pub auto_rotate: bool,
    pub deskew: bool,
    /// Also turn the scan on the page in the document (one undo step) before reading, instead of
    /// straightening only the image that is read.
    pub deskew_in_document: bool,
    pub denoise: bool,
    pub binarize: bool,
    /// What Accept produces.
    pub output_mode: OutputMode,
    /// Batch: leave files that already contain text alone.
    pub skip_text_files: bool,
    /// Batch: leave pages that already contain text alone.
    pub skip_text_pages: bool,
    /// Batch: read everything, overriding both skips.
    pub force_ocr: bool,
    /// Batch: where the searchable copies go (None: ask each time). Remembered from the last
    /// batch's folder pick, so a repeated export into the same place does not re-ask.
    pub output_folder: Option<String>,
}

impl Default for OcrDraft {
    fn default() -> Self {
        OcrDraft {
            pages: OcrPages::All,
            from: 1,
            to: 1,
            language: None,
            dpi: 300,
            text_pages: false,
            engine: EngineChoice::default(),
            strategy: MergeStrategy::default(),
            auto_rotate: true,
            deskew: true,
            deskew_in_document: false,
            denoise: false,
            binarize: false,
            output_mode: OutputMode::default(),
            skip_text_files: false,
            skip_text_pages: true,
            force_ocr: false,
            output_folder: None,
            digits: Digits::AsRead,
            read_unusable_text: false,
        }
    }
}

impl OcrDraft {
    /// The document language: the user's pick, else the interface language when text recognition
    /// reads it (Thai in a Thai interface), else English.
    pub fn language(&self) -> &str {
        self.language.as_deref().unwrap_or_else(|| {
            let ui = crate::i18n::current().code();
            LANGUAGES.iter().find(|l| l.0 == ui).map_or("en", |l| l.0)
        })
    }

    /// The recognition settings these choices make.
    pub fn settings(&self) -> OcrSettings {
        OcrSettings {
            dpi: self.dpi as f32,
            language: self.language().to_string(),
            digits: self.digits,
            read_unusable_text: self.read_unusable_text,
            engine: self.engine,
            strategy: self.strategy,
            auto_rotate: self.auto_rotate,
            deskew: self.deskew,
            deskew_mode: if self.deskew_in_document { DeskewMode::Persistent } else { DeskewMode::Transient },
            denoise: self.denoise,
            binarize: self.binarize,
            output_mode: self.output_mode,
            skip_text_files: self.skip_text_files,
            skip_text_pages: self.skip_text_pages && !self.text_pages,
            force_ocr: self.force_ocr,
            ..Default::default()
        }
    }

    /// Restore from the preferences JSON (`ocr` object); unknown or malformed values keep the
    /// defaults, like every other restored setting.
    pub fn apply_pref(&mut self, v: &serde_json::Value) {
        if let Some(l) = v["language"].as_str().filter(|l| LANGUAGES.iter().any(|x| x.0 == *l)) {
            self.language = Some(l.to_string());
        }
        if let Some(e) = v["engine"].as_str() {
            self.engine = EngineChoice::parse(e);
        }
        if let Some(s) = v["strategy"].as_str() {
            self.strategy = parse_strategy(s);
        } else if self.engine == EngineChoice::Auto {
            // No strategy was saved with "Automatic": it reads the way Automatic reads by
            // default, not as "Primary only" (which would never bring in the second engine).
            self.strategy = self.engine.default_strategy();
        }
        if let Some(m) = v["output_mode"].as_str() {
            self.output_mode = OutputMode::parse(m);
        }
        for (key, flag) in [
            ("auto_rotate", &mut self.auto_rotate),
            ("deskew", &mut self.deskew),
            ("deskew_in_document", &mut self.deskew_in_document),
            ("denoise", &mut self.denoise),
            ("binarize", &mut self.binarize),
        ] {
            if let Some(on) = v[key].as_bool() {
                *flag = on;
            }
        }
        for (key, flag) in
            [("skip_text_files", &mut self.skip_text_files), ("skip_text_pages", &mut self.skip_text_pages), ("force_ocr", &mut self.force_ocr)]
        {
            if let Some(on) = v[key].as_bool() {
                *flag = on;
            }
        }
        if let Some(f) = v["output_folder"].as_str().filter(|f| !f.is_empty()) {
            self.output_folder = Some(f.to_string());
        }
    }

    /// The preferences JSON for `persist`.
    pub fn to_pref(&self) -> serde_json::Value {
        serde_json::json!({
            "language": self.language(),
            "engine": self.engine.name(),
            "strategy": strategy_id(self.strategy),
            "output_mode": self.output_mode.name(),
            "auto_rotate": self.auto_rotate,
            "deskew": self.deskew,
            "deskew_in_document": self.deskew_in_document,
            "denoise": self.denoise,
            "binarize": self.binarize,
            "skip_text_files": self.skip_text_files,
            "skip_text_pages": self.skip_text_pages,
            "force_ocr": self.force_ocr,
            "output_folder": self.output_folder,
        })
    }
}

<<<<<<< ours
/// What to tell the user when `language` has no models installed, in the interface language
/// (`None` when it has). The sentence for a named language is [`OcrError::NoLanguageModel`]'s own
/// wording with `{language}` where it names the language: that is the catalogs' key.
fn missing_models(language: &str) -> Option<String> {
    // The web build downloads the models on first use (the dialog says so): English always, and
    // Thai when `models/` holds its files. They are fetched together, so until the English models
    // are loaded Thai is still on its way.
    #[cfg(target_arch = "wasm32")]
    let installed = language == "en"
        || pdfcraft_engine::ocr::available_for(language)
        || (language == "th" && pdfcraft_engine::ocr::engine_unavailable_reason(EngineChoice::Ocrs).is_some());
    #[cfg(not(target_arch = "wasm32"))]
    let installed = pdfcraft_engine::ocr::available_for(language);
    if installed {
        return None;
    }
    if language == "en" {
        return Some(tl!("Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them.").to_string());
    }
    let name = LANGUAGES.iter().find(|l| l.0 == language).map_or(language, |l| l.1);
    let key = OcrError::NoLanguageModel("{language}".into()).to_string();
    Some(crate::i18n::fmt(crate::i18n::t(&key), &[("language", name)]))
}

/// Progress of a run: pages done, total, the result once finished, and a cancel request.
#[derive(Default)]
pub struct OcrProgress {
    pub done: usize,
    pub total: usize,
    pub result: Option<Result<Vec<OcrPage>, String>>,
    pub cancel: bool,
=======
/// The id a strategy is saved and chosen under.
fn strategy_id(strategy: MergeStrategy) -> &'static str {
    match strategy {
        MergeStrategy::PrimaryOnly => "primary",
        MergeStrategy::ConfidenceWeighted => "confidence",
        MergeStrategy::RoverVote => "rover",
    }
>>>>>>> theirs
}

/// The strategy a saved or chosen id names; an unknown id reads alone.
fn parse_strategy(id: &str) -> MergeStrategy {
    match id.trim().to_ascii_lowercase().as_str() {
        "confidence" | "confidence-weighted" | "confidence_weighted" => MergeStrategy::ConfidenceWeighted,
        "rover" | "rover-vote" | "vote" => MergeStrategy::RoverVote,
        _ => MergeStrategy::PrimaryOnly,
    }
}

/// The summary's note when pages were skipped because their own text can't be read (SRS FR-17):
/// it names the dialog option that reads them, which is the user's confirmation.
fn unreadable_note() -> String {
    crate::i18n::fmt(
        tl!("; some pages have unreadable text (turn on “{option}” to read them)"),
        &[("option", tl!("Also recognize pages that already have text"))],
    )
}

/// Pages skipped because their own text is unusable.
fn unreadable_pages(pages: &[OcrPage]) -> usize {
    pages.iter().filter(|p| p.unusable_text && p.skipped.is_some()).count()
}

/// Pages of `doc` that a run skipped because their own text is unusable, offered for
/// recognition (SRS FR-17, N9, S-09): nothing reads them until the user confirms, and the run
/// the confirmation starts reads only pages whose text is still unusable.
#[derive(Clone, Debug, PartialEq)]
pub struct UnreadableOffer {
    pub doc: DocId,
    /// Page indices (0-based), never empty.
    pub pages: Vec<usize>,
    /// The settings of the run that found them.
    pub settings: OcrSettings,
}

impl UnreadableOffer {
    /// The offer for a finished run, if it skipped pages for unusable text.
    fn from_run(doc: DocId, found: &[OcrPage], settings: &OcrSettings) -> Option<UnreadableOffer> {
        let pages: Vec<usize> = found.iter().filter(|p| p.unusable_text && p.skipped.is_some()).map(|p| p.page).collect();
        (!pages.is_empty()).then(|| UnreadableOffer { doc, pages, settings: settings.clone() })
    }

    /// The run the user confirms: these pages, reading over unusable text but still skipping a
    /// page whose text is fine (should the pages have moved since), as one undoable step.
    fn confirmed(&self) -> OcrSettings {
        OcrSettings { skip_text_pages: true, read_unusable_text: true, ..self.settings.clone() }
    }

    fn message(&self) -> String {
        if self.pages.len() == 1 {
            tl!("1 page has unreadable text. Recognize text on it? Its text stays, and you can undo this in one step.").to_string()
        } else {
            crate::i18n::fmt(
                tl!("{n} pages have unreadable text. Recognize text on just those pages? Their text stays, and you can undo this in one step."),
                &[("n", &self.pages.len().to_string())],
            )
        }
    }
}

/// A page Edit should read now (see [`PdfCraftApp::scan_to_read`]).
struct ScanToRead {
    view: usize,
    doc: DocId,
    page: usize,
    /// The document's edit generation when it was judged.
    generation: u64,
}

/// Files to recognize, by name, and the settings they were chosen with.
#[cfg(target_arch = "wasm32")]
pub type WaitingFiles = (Vec<(String, Vec<u8>)>, OcrSettings);

pub struct OcrRun {
    pub doc: DocId,
<<<<<<< ours
    pub progress: Arc<Mutex<OcrProgress>>,
    /// The run's settings, for the offer to read pages it skipped for unusable text.
    pub settings: OcrSettings,
=======
    /// Pages done and total, and the cancel request the job reads between pages.
    pub count: Arc<JobCount>,
>>>>>>> theirs
    /// What the document looked like when the job started: a result is applied only while it
    /// still does (the dialog closes on Go, so the user can edit, or redact, while this runs).
    pub guard: pdfcraft_engine::ocr::OcrGuard,
    /// Set for a run Edit started on a scanned page, not asked for: it shows no progress, says nothing
    /// when it found no words (a page with text of its own is skipped, and Edit must not make noise),
    /// and its result waits for Read mode and dialogs and is dropped if the document changed.
    pub automatic: Option<AutoOcr>,
    /// The dialog promised an editable document: the result goes to the OCR Verify screen
    /// (whose Accept produces the editable document), not to an invisible layer.
    pub editable: bool,
    /// The web build reads the pages in Web Workers, pumped by [`PdfCraftApp::poll_ocr`].
    #[cfg(target_arch = "wasm32")]
    pub web: Option<crate::ocr_web::Pages>,
    /// What the web workers read, kept until [`PdfCraftApp::poll_ocr`] takes it (an automatic run
    /// holds it back while Read mode or a dialog is up).
    #[cfg(target_arch = "wasm32")]
    pub web_result: Option<Result<Vec<OcrPage>, String>>,
}

/// What an Edit-started run needs to know to refuse a result that no longer fits its document.
#[derive(Clone, Copy, Debug)]
pub struct AutoOcr {
    /// The page it reads (0-based), when it was started.
    pub page: usize,
    /// The document's edit generation when it was started: a result is for those bytes only, and
    /// any later change (a page deleted or moved, say) may put a different page at that index.
    pub generation: u64,
}

/// The Tesseract probe runs in the background (the interface never waits on a process): while
/// it runs, an engine choice reads as "still checking", so look again soon.
pub(crate) fn keep_asking_while_probing(ctx: &egui::Context) {
    if pdfcraft_engine::ocr::engine_probe_running() {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
}

<<<<<<< ours
/// The dialog's height outside its scrolling settings: the title, a notice of up to two lines,
/// the Cancel / Recognize text row and the modal's own margins.
const OCR_DIALOG_RESERVE: f32 = 220.0;
/// The least the settings scroll in, however short the window: small enough that the title and the
/// buttons still fit the smallest window at the largest interface size (173 pt), as Preferences.
const OCR_MIN_SCROLL: f32 = 40.0;

/// The Recognize Text dialog: the title, the settings groups (scrolling when the window is too
/// short for them), any engine notice, and the Cancel / Recognize text row. Returns
/// `(recognize, cancel)`, which of those buttons was clicked this frame.
pub(crate) fn body(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) -> (bool, bool) {
    let pages = app.active_ids().and_then(|(_, id)| app.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
    let d = &mut app.ocr_draft;
    let mut language = d.language().to_string();
    d.to = d.to.clamp(1, pages);
    d.from = d.from.clamp(1, d.to);
    ui.label(egui::RichText::new(tl!("Recognize Text")).font(theme::semibold(18.0)));
    ui.add_space(8.0);
    let group = |ui: &mut egui::Ui, title: &str, body: &mut dyn FnMut(&mut egui::Ui)| {
        ui.label(egui::RichText::new(tl!(title)).font(theme::semibold(13.0)));
        egui::Frame::new().fill(t.hover).corner_radius(egui::CornerRadius::same(6)).inner_margin(egui::Margin::same(10)).show(ui, |ui| {
            ui.set_width(ui.available_width());
            body(ui);
        });
        ui.add_space(8.0);
    };
    // The settings outgrow a short window or a large display scale (#6599): they scroll, and
    // the notices and the Cancel / Recognize text row stay below the scroll, always in reach.
    let max = (ui.ctx().content_rect().height() - OCR_DIALOG_RESERVE).max(OCR_MIN_SCROLL);
    egui::ScrollArea::vertical()
        .id_salt("ocr-dialog-scroll")
        .max_height(max)
        // Only applies when the settings overflow: the dialog then opens at its full height
        // instead of growing frame by frame, as Preferences does.
        .min_scrolled_height(max)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            group(ui, "Pages", &mut |ui| {
                widgets::radio_value(ui, &mut d.pages, OcrPages::All, tl!("All pages"));
                widgets::radio_value(ui, &mut d.pages, OcrPages::Current, tl!("Current page"));
                ui.horizontal(|ui| {
                    widgets::radio_value(ui, &mut d.pages, OcrPages::Range, tl!("From"));
                    let on = d.pages == OcrPages::Range;
                    ui.add_enabled(on, egui::DragValue::new(&mut d.from).range(1..=pages));
                    ui.label(tl!("to"));
                    ui.add_enabled(on, egui::DragValue::new(&mut d.to).range(1..=pages));
                });
            });
            group(ui, "Settings", &mut |ui| {
                egui::Grid::new("ocr-settings").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label(tl!("Document language"));
                    let name = LANGUAGES.iter().find(|l| l.0 == language).map_or("English", |l| l.1);
                    craft_ui::ComboBox::from_id_salt("ocr-language").selected_text(tl!(name)).width(220.0).show_ui(ui, |ui| {
                        for (code, name) in LANGUAGES {
                            ui.selectable_value(&mut language, (*code).to_string(), tl!(*name));
                        }
                    });
                    ui.end_row();
                    ui.label(tl!("Engine"));
                    let engine_name = match d.engine {
                        EngineChoice::Auto => tl!("Automatic"),
                        EngineChoice::Ocrs => tl!("Built-in"),
                        EngineChoice::Tesseract => tl!("Tesseract"),
                    };
                    craft_ui::ComboBox::from_id_salt("ocr-engine").selected_text(engine_name).width(220.0).show_ui(ui, |ui| {
                        let choices: [(EngineChoice, &str); 3] =
                            [(EngineChoice::Auto, "Automatic"), (EngineChoice::Ocrs, "Built-in"), (EngineChoice::Tesseract, "Tesseract")];
                        for (choice, name) in choices {
                            ui.selectable_value(&mut d.engine, choice, tl!(name));
                        }
                    });
                    ui.end_row();
                    ui.label(tl!("Combine engines"));
                    let strategy_name = match d.strategy {
                        MergeStrategy::PrimaryOnly => tl!("Primary only"),
                        MergeStrategy::ConfidenceWeighted => tl!("Confidence weighted"),
                        MergeStrategy::RoverVote => tl!("ROVER vote"),
                    };
                    craft_ui::ComboBox::from_id_salt("ocr-strategy").selected_text(strategy_name).width(220.0).show_ui(ui, |ui| {
                        let choices: [(MergeStrategy, &str); 3] = [
                            (MergeStrategy::PrimaryOnly, "Primary only"),
                            (MergeStrategy::ConfidenceWeighted, "Confidence weighted"),
                            (MergeStrategy::RoverVote, "ROVER vote"),
                        ];
                        for (choice, name) in choices {
                            ui.selectable_value(&mut d.strategy, choice, tl!(name));
                        }
                    });
                    ui.end_row();
                    ui.label(tl!("Output"));
                    let mode_name = match d.output_mode {
                        OutputMode::Searchable => tl!("Searchable Image (Exact)"),
                        OutputMode::EditableText => tl!("Editable text"),
                    };
                    craft_ui::ComboBox::from_id_salt("ocr-output").selected_text(mode_name).width(220.0).show_ui(ui, |ui| {
                        let s = ui
                            .selectable_label(d.output_mode == OutputMode::Searchable, tl!("Searchable Image (Exact)"))
                            .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
                        if s.clicked() {
                            d.output_mode = OutputMode::Searchable;
                        }
                        let e = ui
                            .selectable_label(d.output_mode == OutputMode::EditableText, tl!("Editable text"))
                            .on_hover_text(tl!("Writes a new document of visible text; the scan is not changed"));
                        if e.clicked() {
                            d.output_mode = OutputMode::EditableText;
                        }
                    });
                    ui.end_row();
                    ui.label(tl!("Downsample to"));
                    craft_ui::ComboBox::from_id_salt("ocr-dpi").selected_text(format!("{} dpi", d.dpi)).width(220.0).show_ui(ui, |ui| {
                        for v in [600, 300, 150, 72] {
                            ui.selectable_value(&mut d.dpi, v, format!("{v} dpi"));
                        }
                    });
                    ui.end_row();
                    ui.label(tl!("Digits"));
                    let digits_label = match d.digits {
                        Digits::AsRead => tl!("As recognized"),
                        Digits::Thai => tl!("Thai digits"),
                        Digits::Arabic => tl!("Arabic digits"),
                    };
                    craft_ui::ComboBox::from_id_salt("ocr-digits").selected_text(digits_label).width(220.0).show_ui(ui, |ui| {
                        ui.selectable_value(&mut d.digits, Digits::AsRead, tl!("As recognized"));
                        ui.selectable_value(&mut d.digits, Digits::Thai, tl!("Thai digits"));
                        ui.selectable_value(&mut d.digits, Digits::Arabic, tl!("Arabic digits"));
                    });
                    ui.end_row();
                });
                ui.add_space(4.0);
                ui.add(crate::widgets::checkbox(&mut d.text_pages, tl!("Also recognize pages that already have text")))
                    .on_hover_text(tl!("Pictures on those pages are recognized too; words already in the page's text are not added again"));
                // Pages that already have text include these, so the choice only matters without it.
                ui.add_enabled(
                    !d.text_pages,
                    crate::widgets::checkbox(&mut d.read_unusable_text, tl!("Also recognize pages whose text is unusable")),
                );
                ui.add(crate::widgets::checkbox(&mut d.auto_rotate, tl!("Straighten pages scanned sideways or upside down")));
                ui.add(crate::widgets::checkbox(&mut d.deskew, tl!("Straighten skewed text lines")));
                ui.add_enabled(d.deskew, crate::widgets::checkbox(&mut d.deskew_in_document, tl!("Also straighten the page in the document")))
                    .on_hover_text(tl!(
                        "The scan is turned on the page (one undo step) before it is read; otherwise only the image that is read is straightened"
                    ));
                ui.add(crate::widgets::checkbox(&mut d.denoise, tl!("Remove scan speckle")));
                ui.add(crate::widgets::checkbox(&mut d.binarize, tl!("Black and white (binarize)")));
            });
            group(ui, "Skip", &mut |ui| {
                ui.add(crate::widgets::checkbox(&mut d.skip_text_pages, tl!("Skip pages that already have text")))
                    .on_hover_text(tl!("Pages with renderable text are left alone and reported as skipped"));
                ui.add(crate::widgets::checkbox(&mut d.skip_text_files, tl!("Skip files that already have text")))
                    .on_hover_text(tl!("In multiple files: a file with any text is its own skipped bucket, and nothing is written for it"));
                ui.add(crate::widgets::checkbox(&mut d.force_ocr, tl!("Force OCR (override skip options)")))
                    .on_hover_text(tl!("Reads pages and files even where the skip options would leave them alone"));
            });
            group(ui, "Output folder", &mut |ui| {
                ui.horizontal(|ui| match &d.output_folder {
                    Some(f) => {
                        ui.monospace(f).on_hover_text(tl!("The searchable copies are written here under the source names"));
                        if ui.small_button(tl!("Clear")).clicked() {
                            d.output_folder = None;
                        }
                    }
                    None => {
                        ui.label(tl!("Automatic")).on_hover_text(tl!("A folder is asked for when a multiple-file run starts"));
                    }
                });
            });
        });
    if language != d.language() {
        d.language = Some(language.clone());
    }
    let missing = missing_models(&language);
    if let Some(why) = &missing {
        ui.label(egui::RichText::new(why).small().color(t.text_muted));
        ui.add_space(6.0);
    }
    keep_asking_while_probing(ui.ctx());
    // The web build has no model files: it downloads the models the first time they're needed.
    #[cfg(target_arch = "wasm32")]
    let downloading = d.engine != EngineChoice::Tesseract && pdfcraft_engine::ocr::engine_unavailable_reason(EngineChoice::Ocrs).is_some();
    #[cfg(not(target_arch = "wasm32"))]
    let downloading = false;
    #[cfg(target_arch = "wasm32")]
    if downloading {
        match crate::ocr_web::state() {
            crate::ocr_web::State::Failed(e) => {
                ui.horizontal_wrapped(|ui| {
                    let msg = crate::i18n::fmt(tl!("Couldn't download the text recognition models: {e}"), &[("e", &e)]);
                    ui.label(egui::RichText::new(msg).small().color(t.text_muted));
                    if widgets::pill_button(ui, tl!("Download"), false).clicked() {
                        crate::ocr_web::start(ui.ctx().clone());
                    }
                });
            }
            state => {
                if state == crate::ocr_web::State::NotStarted {
                    crate::ocr_web::start(ui.ctx().clone());
                }
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(egui::RichText::new(tl!("Downloading the text recognition models…")).small().color(t.text_muted));
                });
=======
/// Every engine the dialog offers, in its order.
const ENGINES: [EngineChoice; 3] = [EngineChoice::Auto, EngineChoice::Ocrs, EngineChoice::Tesseract];

/// The resolutions "Downsample to" offers.
const DPI_CHOICES: [u32; 4] = [600, 300, 150, 72];

/// The action the Output folder row's Choose… button files.
const CHOOSE_FOLDER: &str = "output_folder.choose";

/// The folder pick for the searchable copies of a multiple-file run and for the dialog's Output folder.
#[cfg(not(target_arch = "wasm32"))]
fn output_folder_request() -> craft::host::PickRequest {
    craft::host::PickRequest::folder("ocr").title(tl!("Choose a folder for the searchable files").to_string())
}

/// The page range the draft stands for, in a document of `count` pages.
fn page_range_of(d: &OcrDraft, count: usize) -> PageRange {
    let scope = match d.pages {
        OcrPages::All => PageScope::All,
        OcrPages::Current => PageScope::Current,
        OcrPages::Range => PageScope::Range,
    };
    let to = d.to.clamp(1, count.max(1));
    PageRange { scope, from: d.from.clamp(1, to), to: Some(to), ..PageRange::default() }
}

/// Why `engine` cannot run, in the interface's language: the built-in engine's missing models get
/// the notice users know; Tesseract's reasons are catalog keys of their own.
fn engine_unavailable_text(engine: EngineChoice) -> Option<String> {
    let why = pdfcraft_engine::ocr::engine_unavailable_reason(engine)?;
    Some(match engine {
        EngineChoice::Tesseract => crate::i18n::t(&why).to_string(),
        _ => tl!(
            "Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them."
        )
        .to_string(),
    })
}

/// The values the Recognize Text dialog opens with: the draft's choices, the pages as a range.
fn fields_from(d: &OcrDraft, count: usize) -> DialogFields {
    let mut f = DialogFields::new();
    f.set("pages", PageRangeOptions::field_value(&page_range_of(d, count)));
    f.set("language", d.language.clone());
    f.set("engine", d.engine.name().to_string());
    f.set("strategy", strategy_id(d.strategy).to_string());
    f.set("output_mode", d.output_mode.name().to_string());
    f.set("dpi", d.dpi.to_string());
    f.set("text_pages", d.text_pages);
    f.set("auto_rotate", d.auto_rotate);
    f.set("deskew", d.deskew);
    f.set("denoise", d.denoise);
    f.set("binarize", d.binarize);
    f.set("skip_text_pages", d.skip_text_pages);
    f.set("skip_text_files", d.skip_text_files);
    f.set("force_ocr", d.force_ocr);
    f.set("output_folder", d.output_folder.clone().unwrap_or_default());
    f
}

/// Writes what the user confirmed in the dialog into the draft; a value the dialog does not hold
/// leaves the draft's as it is.
fn draft_from(d: &mut OcrDraft, fields: &DialogFields, count: usize) {
    if let Some(range) = PageRangeOptions::value(fields, "pages") {
        match range.scope {
            PageScope::All => d.pages = OcrPages::All,
            PageScope::Current => d.pages = OcrPages::Current,
            PageScope::Range => {
                d.pages = OcrPages::Range;
                d.from = range.from;
                d.to = range.to.unwrap_or(count);
>>>>>>> theirs
            }
            // The row offers no selected pages.
            _ => {}
        }
<<<<<<< ours
        ui.add_space(6.0);
    }
    if let Some(why) = pdfcraft_engine::ocr::engine_unavailable_reason(d.engine).filter(|_| !downloading) {
        // Said in the interface's language: the built-in engine's missing models get the
        // notice users know; Tesseract's reasons are catalog keys of their own.
        let text = match d.engine {
            pdfcraft_engine::ocr::EngineChoice::Tesseract => crate::i18n::t(&why).to_string(),
            _ => tl!("Text recognition isn't installed: its model files are missing. Reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them.")
                .to_string(),
        };
        ui.label(egui::RichText::new(text).small().color(t.text_muted));
        ui.add_space(6.0);
    }
    let available = missing.is_none();
    ui.add_space(6.0);
    let (mut go, mut cancel) = (false, false);
    let can_run = available && pdfcraft_engine::ocr::engine_unavailable_reason(d.engine).is_none();
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(can_run, |ui| widgets::pill_button(ui, tl!("Recognize text"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
=======
    }
    if let Some(l) = fields.text("language").filter(|l| LANGUAGES.iter().any(|x| x.0 == *l)) {
        d.language = l.to_string();
    }
    if let Some(e) = fields.text("engine") {
        d.engine = EngineChoice::parse(e);
    }
    if let Some(s) = fields.text("strategy") {
        d.strategy = parse_strategy(s);
    }
    if let Some(m) = fields.text("output_mode") {
        d.output_mode = OutputMode::parse(m);
    }
    if let Some(dpi) = fields.text("dpi").and_then(|v| v.parse().ok()) {
        d.dpi = dpi;
    }
    for (key, flag) in [
        ("text_pages", &mut d.text_pages),
        ("auto_rotate", &mut d.auto_rotate),
        ("deskew", &mut d.deskew),
        ("denoise", &mut d.denoise),
        ("binarize", &mut d.binarize),
        ("skip_text_pages", &mut d.skip_text_pages),
        ("skip_text_files", &mut d.skip_text_files),
        ("force_ocr", &mut d.force_ocr),
    ] {
        if let Some(on) = fields.bool(key) {
            *flag = on;
        }
    }
    if let Some(folder) = fields.text("output_folder") {
        d.output_folder = (!folder.is_empty()).then(|| folder.to_string());
    }
}

/// The Recognize Text dialog over `fields`, in a document of `count` pages whose current page is
/// `current` (1-based): the pages, the settings, the skip options and the output folder, with the
/// engine notice under them and OK off while the chosen engine cannot run.
fn recognize_dialog(fields: &mut DialogFields, count: usize, current: usize) -> ParamDialog<'_> {
    let engine = EngineChoice::parse(fields.text("engine").unwrap_or_default());
    let reason = engine_unavailable_text(engine);

    let languages = LANGUAGES.iter().map(|(code, name)| ((*code).to_string(), tl!(*name).into())).collect();
    let engine_names = |choice: EngineChoice| match choice {
        EngineChoice::Auto => tl!("Automatic"),
        EngineChoice::Ocrs => tl!("Built-in"),
        EngineChoice::Tesseract => tl!("Tesseract"),
    };
    let mut engine_row = Param::new(
        "engine",
        tl!("Engine"),
        ParamKind::choice(ENGINES.iter().map(|&c| (c.name().to_string(), engine_names(c).into())).collect(), None),
    )
    .option_description(EngineChoice::Auto.name(), tl!("Both engines, combined as the Strategy says"));
    for choice in ENGINES {
        if let Some(why) = engine_unavailable_text(choice) {
            engine_row = engine_row.disabled_option(choice.name(), why);
        }
    }
    let strategies = [
        (MergeStrategy::PrimaryOnly, tl!("Primary only")),
        (MergeStrategy::ConfidenceWeighted, tl!("Confidence weighted")),
        (MergeStrategy::RoverVote, tl!("ROVER vote")),
    ];
    let outputs = [
        (OutputMode::Searchable, tl!("Searchable Image (Exact)"), tl!("Adds invisible text over each word; the page image is not changed")),
        (OutputMode::EditableText, tl!("Editable text"), tl!("Writes a new document of visible text; the scan is not changed")),
    ];
    let mut output_row = Param::new(
        "output_mode",
        tl!("Output"),
        ParamKind::choice(outputs.iter().map(|(mode, name, _)| (mode.name().to_string(), (*name).into())).collect(), None),
    );
    for (mode, _, about) in &outputs {
        output_row = output_row.option_description(mode.name(), *about);
    }
    let flag = |key: &str, label: &str, about: Option<&str>| {
        let row = Param::new(key, label.to_string(), ParamKind::bool(false));
        match about {
            Some(text) => row.tooltip(text.to_string()),
            None => row,
        }
    };

    let params = vec![
        PageRangeOptions::param("pages", tl!("Pages"), count, current, PageRangeChoices::CurrentFromTo),
        Param::section(tl!("Settings")),
        Param::new("language", tl!("Document language"), ParamKind::choice(languages, None)),
        engine_row,
        Param::new(
            "strategy",
            tl!("Combine engines"),
            ParamKind::choice(strategies.iter().map(|(s, name)| (strategy_id(*s).to_string(), (*name).into())).collect(), None),
        ),
        output_row,
        Param::new(
            "dpi",
            tl!("Downsample to"),
            ParamKind::choice(DPI_CHOICES.iter().map(|v| (v.to_string(), format!("{v} dpi").into())).collect(), None),
        ),
        flag(
            "text_pages",
            tl!("Also recognize pages that already have text"),
            Some(tl!("Pictures on those pages are recognized too; words already in the page's text are not added again")),
        ),
        flag("auto_rotate", tl!("Straighten pages scanned sideways or upside down"), None),
        flag("deskew", tl!("Straighten skewed text lines"), None),
        flag("denoise", tl!("Remove scan speckle"), None),
        flag("binarize", tl!("Black and white (binarize)"), None),
        Param::section(tl!("Skip")),
        flag(
            "skip_text_pages",
            tl!("Skip pages that already have text"),
            Some(tl!("Pages with renderable text are left alone and reported as skipped")),
        ),
        flag(
            "skip_text_files",
            tl!("Skip files that already have text"),
            Some(tl!("In multiple files: a file with any text is its own skipped bucket, and nothing is written for it")),
        ),
        flag(
            "force_ocr",
            tl!("Force OCR (override skip options)"),
            Some(tl!("Reads pages and files even where the skip options would leave them alone")),
        ),
        Param::custom("output_folder", tl!("Output folder"), |ui, fields, cx| {
            let folder = fields.text("output_folder").unwrap_or_default().to_string();
            ui.horizontal(|ui| {
                let shown = if folder.is_empty() { tl!("Automatic").to_string() } else { folder.clone() };
                let _ = Text::new(Label::from(shown)).show(ui);
                if clicked(&Button::new(Label::from(tl!("Choose…"))).show(ui)) {
                    cx.request_action(CHOOSE_FOLDER);
                }
                if !folder.is_empty() && clicked(&Button::new(Label::from(tl!("Clear"))).show(ui)) {
                    fields.set("output_folder", String::new());
                }
            })
            .response
        }),
    ];

    // The web build has no model files: it downloads the models the first time they're needed.
    #[cfg(target_arch = "wasm32")]
    let download_notice =
        (engine != EngineChoice::Tesseract && pdfcraft_engine::ocr::engine_unavailable_reason(EngineChoice::Ocrs).is_some()).then(|| {
            match crate::ocr_web::state() {
                crate::ocr_web::State::Failed(e) => crate::i18n::fmt(tl!("Couldn't download the text recognition models: {e}"), &[("e", &e)]),
                _ => tl!("Downloading the text recognition models…").to_string(),
>>>>>>> theirs
            }
        });
    #[cfg(not(target_arch = "wasm32"))]
    let download_notice: Option<String> = None;

    let mut dialog = ParamDialog::new("ocr.recognize", tl!("Recognize Text"), params)
        .ok_label(Label::from(tl!("Recognize text")))
        .ok_enabled(reason.is_none())
        .values(Bind::bound(fields));
    if let Some(note) = download_notice.or_else(|| reason.clone()) {
        dialog = dialog.footer_note(Label::from(note));
    }
    if let Some(why) = reason {
        dialog = dialog.ok_reason(Label::from(why));
    }
    dialog
}

impl PdfCraftApp {
<<<<<<< ours
    /// Recognize text on the pages chosen in the dialog, in the background.
    pub fn start_ocr(&mut self) {
        // Reading over unusable text is confirmed for one run only (SRS N9).
        let read_unusable_text = std::mem::take(&mut self.ocr_draft.read_unusable_text);
        let Some((vi, id)) = self.active_ids() else { return };
=======
    /// Why Recognize Text cannot start on document `id` now: a run is going, or the document is
    /// read-only. The one rule for opening the dialog and for confirming it.
    fn ocr_start_refusal(&self, id: DocId) -> Option<String> {
>>>>>>> theirs
        if self.ocr_run.is_some() {
            return Some(tl!("Text recognition is already running").to_string());
        }
        self.session.get(id).and_then(|d| d.read_only_reason.clone())
    }

    /// Opens the Recognize Text dialog on the active document, with the draft's choices. Its OK is
    /// held: [`Self::recognize_text_pass`] answers it once the run has started or been refused.
    pub fn open_recognize_text(&mut self, ctx: &egui::Context) {
        let Some((vi, id)) = self.active_ids() else { return };
        if let Some(why) = self.ocr_start_refusal(id) {
            self.notify(why);
            return;
        }
        let count = self.session.get(id).map_or(1, |d| d.info.pages.len().max(1));
        let current = self.views[vi].current() + 1;
        let fields = fields_from(&self.ocr_draft, count);
        let dialog = HostedParamDialog::new(ctx, "ocr.recognize", fields, move |f| recognize_dialog(f, count, current)).held_ok();
        // PLAN: assumed `crate::dialogs::host::open(app: &mut PdfCraftApp, dialog: HostedParamDialog)` from pdf-dialogs-w4.
        crate::dialogs::host::open(self, dialog);
    }

    /// One pass of the open Recognize Text dialog: keeps asking while the engine probe runs, and
    /// when the user confirmed, writes the draft, starts the run and answers the held OK (a refusal
    /// is the answer's reason, shown in the dialog).
    pub(crate) fn recognize_text_pass(&mut self, ctx: &egui::Context) {
        keep_asking_while_probing(ctx);
        let dialog_id = craft::ui::dialog::dialog_id("ocr.recognize");
        #[cfg(target_arch = "wasm32")]
        if let Some(open) = self.dialogs.get_by_id::<HostedParamDialog>(dialog_id) {
            let engine = EngineChoice::parse(open.fields().text("engine").unwrap_or_default());
            if engine != EngineChoice::Tesseract
                && pdfcraft_engine::ocr::engine_unavailable_reason(EngineChoice::Ocrs).is_some()
                && crate::ocr_web::state() == crate::ocr_web::State::NotStarted
            {
                crate::ocr_web::start(self.wake.clone());
            }
        }
        // The Output folder row's Choose… asks for a folder; the answer fills the row.
        #[cfg(not(target_arch = "wasm32"))]
        if self.dialogs.get_mut_by_id::<HostedParamDialog>(dialog_id).and_then(|d| d.take_action()).is_some_and(|action| action == CHOOSE_FOLDER) {
            self.pick_first(output_folder_request(), None, |app, _, dir| {
                if let Some(dialog) = app.dialogs.get_mut_by_id::<HostedParamDialog>(craft::ui::dialog::dialog_id("ocr.recognize")) {
                    dialog.answer_path("output_folder", dir);
                }
            });
        }
        let Some(confirmed) = self.dialogs.get_mut_by_id::<HostedParamDialog>(dialog_id).and_then(|d| d.take_ok()) else { return };
        let count = self.active_ids().and_then(|(_, id)| self.session.get(id)).map_or(1, |d| d.info.pages.len().max(1));
        draft_from(&mut self.ocr_draft, &confirmed.fields, count);
        let answer = self.start_ocr(ctx);
        if let Some(dialog) = self.dialogs.get_mut_by_id::<HostedParamDialog>(dialog_id) {
            dialog.answer_ok(answer.map_err(Into::into));
        }
    }

    /// Recognize text on the pages chosen in the dialog, in the background; the reason it could
    /// not start otherwise.
    fn start_ocr(&mut self, ctx: &egui::Context) -> Result<(), String> {
        let Some((vi, id)) = self.active_ids() else { return Ok(()) };
        if let Some(why) = self.ocr_start_refusal(id) {
            return Err(why);
        }
        #[cfg(target_arch = "wasm32")]
        if self.web_models_missing(self.ocr_draft.engine) {
            return Err(tl!("Downloading the text recognition models…").to_string());
        }
        let d = &self.ocr_draft;
        let pages: Vec<usize> = match d.pages {
            OcrPages::All => Vec::new(),
            OcrPages::Current => vec![self.views[vi].current()],
            OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
<<<<<<< ours
        let settings = OcrSettings { read_unusable_text, ..d.settings() };
        self.start_ocr_pages(id, &pages, settings, false);
=======
        let settings = d.settings();
        self.start_ocr_pages(ctx, id, &pages, settings, false);
        Ok(())
>>>>>>> theirs
    }

    /// Edit on a scanned page: like Acrobat's "Converting scanned page to Editable Text and
    /// Images", the page's words are recognized first, so that they open for editing (Edit text &
    /// images groups the recognised words into phrases). Runs once per page, for the page in view
    /// while Edit is on (its tab, or the Edit text & images or Add text tool). The page must be a
    /// picture (an image covering at least half of it) with no text of its own: comments and text
    /// boxes over it do not count as text, but real page text does, and such a page (a flyer with a
    /// background picture) is left alone, and is judged again only when the document changes. It
    /// is one undoable step, "Recognize text", and is not run again for that page after Undo.
    /// Without an engine the page waits for one (see [`Self::await_ocr_models`]).
    pub(crate) fn auto_ocr_scanned_page(&mut self, ctx: &egui::Context) {
        let Some(scan) = self.scan_to_read() else { return };
        // the engine this run will use, not always the built-in one
        let mut settings = self.ocr_draft.settings();
        if pdfcraft_engine::ocr::engine_unavailable_reason(settings.engine).is_some() {
            self.await_ocr_models(&scan);
            return;
        }
        self.views[scan.view].auto_ocr_tried.insert(scan.page);
        // Only a page with no text of its own is read. Comments and text boxes over the picture do
        // not count as its text (a scan with a few comments is still a scan, as in Acrobat), but
        // a page with real text and a background picture is left alone.
        settings.skip_text_pages = true;
        settings.native_text_only = true;
        self.start_ocr_pages(ctx, scan.doc, &[scan.page], settings, true);
    }

    /// Edit found a scan but no engine can read it. The page is set aside until one can (rather
    /// than judged again every frame), and the web build starts downloading the models, as
    /// Recognize Text does. After a failed download it doesn't try again by itself: Recognize
    /// Text does, and the page is read once the models are in.
    fn await_ocr_models(&mut self, scan: &ScanToRead) {
        self.views[scan.view].auto_ocr_awaiting_models.insert(scan.page, scan.generation);
        #[cfg(target_arch = "wasm32")]
        if crate::ocr_web::state() == crate::ocr_web::State::NotStarted {
            crate::ocr_web::start(self.wake.clone());
            self.notify(tl!("Downloading the text recognition models…"));
        }
    }

    /// The OCR models have just loaded: the scans Edit set aside for want of them may be read.
    /// Only the web build loads them while running.
    #[cfg_attr(not(target_arch = "wasm32"), allow(dead_code))]
    pub(crate) fn ocr_models_loaded(&mut self) {
        for view in &mut self.views {
            view.auto_ocr_awaiting_models.clear();
        }
    }

    /// The page Edit should read now, if there is one: Edit is on outside Read mode with no dialog
    /// open, nothing else is being recognized, the document may be changed, the page in view has
    /// not been read before (nor been found no scan since the document last changed), and an image
    /// covers at least half of it. Needs no OCR models, so the rules can be tested without them;
    /// the page's own text is judged by the run itself.
    fn scan_to_read(&mut self) -> Option<ScanToRead> {
        // Edit is on when its panel is open (the Edit tab) or Edit text & images or Add text is picked,
        // the same test the page view uses for editing content.
        let editing = (self.left_open() && self.left == crate::LeftPanel::Tool("edit"))
            || matches!(self.quick_tool, crate::QuickTool::EditText | crate::QuickTool::AddText);
        // Not in Read mode (which leaves the Edit tool picked but is not editing) and not beneath a
        // dialog or prompt: nothing may change the document under those. It is tried again once
        // they are gone.
        if !editing || self.mode == crate::Mode::Read || self.modal_open() || self.ocr_run.is_some() || self.ocr_batch_running() {
            return None;
        }
        let (view, doc) = self.active_ids()?;
        let page = self.views[view].current();
        if self.views[view].auto_ocr_tried.contains(&page) {
            return None;
        }
        let document = self.session.get(doc)?;
        if document.read_only_reason.is_some() || !document.allows_modification() {
            return None;
        }
        // A page found not to be a scan is judged again only once the document has changed, not
        // every frame (listing its images parses the page).
        let generation = document.edit_generation();
        if self.views[view].auto_ocr_skipped.get(&page) == Some(&generation)
            || self.views[view].auto_ocr_awaiting_models.get(&page) == Some(&generation)
        {
            return None;
        }
        let info = document.info.pages.get(page)?;
        let page_area = f64::from(info.width) * f64::from(info.height);
        let covered = document.page_images(page).iter().any(|i| {
            let [x0, y0, x1, y1] = i.visible_rect.unwrap_or(i.rect);
            page_area > 0.0 && ((x1 - x0) * (y1 - y0)).abs() >= 0.5 * page_area
        });
        if !covered {
            self.views[view].auto_ocr_skipped.insert(page, generation);
            return None;
        }
        Some(ScanToRead { view, doc, page, generation })
    }

    /// Recognize `pages` (0-based; empty = all) of document `id` in the background.
    fn start_ocr_pages(&mut self, ctx: &egui::Context, id: DocId, pages: &[usize], settings: OcrSettings, automatic: bool) {
        // What an automatic run's result must still fit when it arrives (see `AutoOcr`).
        let auto = if automatic {
            pages.first().map(|&page| AutoOcr { page, generation: self.session.get(id).map_or(0, |d| d.edit_generation()) })
        } else {
            None
        };
        if self.ocr_run.is_some() {
            if !automatic {
                self.notify_tr("Text recognition is already running");
            }
            return;
        }
        if let Some(why) = missing_models(&settings.language) {
            if !automatic {
                self.notify(why);
            }
            return;
        }
        // A new run replaces an offer still showing.
        self.ocr_offer = None;
        // Persistent deskew turns the scans in the document first (one undo step); the job, and the
        // guard that says the document has not changed since it started, come after.
        if settings.deskew
            && settings.deskew_mode == DeskewMode::Persistent
            && settings.output_mode == OutputMode::Searchable
            && let Err(e) = self.session.straighten_scans(id, pages, &settings)
        {
            if !automatic {
                self.notify(e.to_string());
            }
            return;
        }
        let Some(job) = self.session.ocr_job(id, pages, settings.clone()) else { return };
        let guard = job.guard.clone();
        let editable = settings.output_mode == OutputMode::EditableText;
        let count = Arc::new(JobCount::default());
        count.set(0, job.pages.len());
        #[cfg(target_arch = "wasm32")]
        {
            let web = Some(crate::ocr_web::Pages::new(job.steps()));
<<<<<<< ours
            self.ocr_run = Some(OcrRun { doc: id, progress, guard, settings, automatic: auto, editable, web });
            self.poll_ocr();
=======
            self.ocr_run = Some(OcrRun { doc: id, count, guard, automatic: auto, editable, web, web_result: None });
            self.poll_ocr(ctx);
>>>>>>> theirs
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.run_ocr_job(ctx, id, job, settings, count, guard, auto, editable);
    }

    /// Reads the pages on the host's job queue. A cancel keeps the pages read so far, so the
    /// result is delivered either way.
    #[cfg(not(target_arch = "wasm32"))]
    #[allow(clippy::too_many_arguments)]
    fn run_ocr_job(
        &mut self,
        ctx: &egui::Context,
        id: DocId,
        job: pdfcraft_engine::ocr::OcrJob,
        settings: OcrSettings,
        count: Arc<JobCount>,
        guard: OcrGuard,
        auto: Option<AutoOcr>,
        editable: bool,
    ) {
<<<<<<< ours
        let p = progress.clone();
        let run_settings = settings.clone();
=======
        let reading = count.clone();
        let wake = self.wake.clone();
>>>>>>> theirs
        // A failure is written on the worker thread: in the UI's language.
        let lang = crate::i18n::current();
        self.ocr_runs.submit(id, craft::host::queue::Priority::default(), move |_| {
            crate::i18n::set_current(lang);
            recognize_guarded(|| {
                pdfcraft_engine::ocr::recognizers(&settings).map(|r| {
                    job.run(&r, |done, total| {
                        reading.set(done, total);
                        wake.wake();
                        !reading.cancelled()
                    })
                })
<<<<<<< ours
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        if self.run_inline {
            work();
        } else {
            std::thread::Builder::new().name("pdfcraft-ocr".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_run = Some(OcrRun { doc: id, progress, guard, settings: run_settings, automatic: auto, editable });
        self.poll_ocr();
=======
            })
        });
        self.ocr_run = Some(OcrRun { doc: id, count, guard, automatic: auto, editable });
        self.poll_ocr(ctx);
>>>>>>> theirs
    }

    /// The user confirmed the offer: read its pages (see [`UnreadableOffer::confirmed`]).
    pub fn confirm_unreadable_offer(&mut self) {
        let Some(offer) = self.ocr_offer.take() else { return };
        if self.session.get(offer.doc).is_none() || offer.pages.is_empty() {
            return;
        }
        if let Some(why) = self.session.get(offer.doc).and_then(|d| d.read_only_reason.clone()) {
            self.notify(why);
            return;
        }
        let settings = offer.confirmed();
        self.start_ocr_pages(offer.doc, &offer.pages, settings, false);
    }

    /// Draw the offer to read pages with unreadable text at the top right of the document area
    /// (clear of the summary toast and the progress card at the bottom, and of the side rails),
    /// while no OCR job runs. It goes when its document closes.
    pub(crate) fn show_unreadable_offer(&mut self, ctx: &egui::Context) {
        let Some(offer) = &self.ocr_offer else { return };
        if self.session.get(offer.doc).is_none() {
            self.ocr_offer = None;
            return;
        }
        if self.ocr_run.is_some() {
            return;
        }
        let message = offer.message();
        let t = Tokens::get(ctx);
        let area = self.document_rect.filter(|r| r.is_positive()).unwrap_or_else(|| ctx.content_rect());
        let (mut go, mut later) = (false, false);
        egui::Area::new(egui::Id::unique("ocr-unreadable-offer"))
            .order(egui::Order::Foreground)
            .pivot(egui::Align2::RIGHT_TOP)
            .fixed_pos(area.right_top() + egui::vec2(-16.0, 16.0))
            .show(ctx, |ui| {
                egui::Frame::popup(ui.style()).fill(t.card).corner_radius(egui::CornerRadius::same(10)).inner_margin(egui::Margin::same(14)).show(
                    ui,
                    |ui| {
                        ui.set_width(340.0_f32.min(area.width() - 32.0).max(160.0));
                        ui.label(egui::RichText::new(message).color(t.text));
                        ui.add_space(8.0);
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            go = widgets::pill_button(ui, tl!("Recognize these pages"), true).clicked();
                            later = widgets::pill_button(ui, tl!("Not now"), false).clicked();
                        });
                    },
                );
            });
        if go {
            self.confirm_unreadable_offer();
        } else if later {
            self.ocr_offer = None;
        }
    }

    /// Recognize text in each file, writing the searchable copies into a folder the user picks
    /// (the export folder override in tests), under the same names.
    pub fn ocr_files(&mut self, ctx: &egui::Context, files: Vec<(String, Vec<u8>)>) {
        if self.ocr_batch_running() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        // The settings showing now, not when the folder arrives. A remembered folder (the last
        // batch's pick, persisted with the preferences) is reused; otherwise the user is asked.
        let settings = self.ocr_draft.settings();
        #[cfg(not(target_arch = "wasm32"))]
        match self.ocr_draft.output_folder.clone() {
            Some(d) => self.ocr_files_into(ctx, files, settings, d.into()),
            None => {
                self.pick_first(output_folder_request(), None, move |app, ctx, dir| app.ocr_files_into(ctx, files, settings, dir));
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.ocr_files_into(ctx, files, settings);
    }

    /// [`Self::ocr_files`] once the folder for the searchable files is known.
    fn ocr_files_into(
        &mut self,
        ctx: &egui::Context,
        files: Vec<(String, Vec<u8>)>,
        settings: OcrSettings,
        #[cfg(not(target_arch = "wasm32"))] dir: std::path::PathBuf,
    ) {
        // Another batch may have started while the folder picker was open.
        if self.ocr_batch_running() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        // The folder is remembered (the next batch reuses it; the dialog's Clear un-remembers).
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.ocr_draft.output_folder = Some(dir.to_string_lossy().into_owned());
        }
        // Two inputs of one file name (from different folders) would write the same output
        // (or download under the same name): refused before anything is read.
        let twice = pdfcraft_engine::ocr::duplicate_output_names(files.iter().map(|(name, _)| name.as_str()));
        if !twice.is_empty() {
            self.notify_error(crate::i18n::fmt(
                tl!("Several files have the same name, so their results would overwrite each other; nothing was written: {list}"),
                &[("list", &twice.join(", "))],
            ));
            return;
        }
        // Collision pre-flight: a searchable copy never silently overwrites. Every target is
        // checked before the first page is read; a collision stops the batch here.
        #[cfg(not(target_arch = "wasm32"))]
        {
            let hits: Vec<String> = files
                .iter()
                .filter_map(|(name, _)| {
                    let target = dir.join(name);
                    target.exists().then(|| target.to_string_lossy().into_owned())
                })
                .take(5)
                .collect();
            if !hits.is_empty() {
                self.notify_error(crate::i18n::fmt(
                    tl!("The output folder already holds files of these names; nothing was written: {list}"),
                    &[("list", &hits.join(", "))],
                ));
                return;
            }
        }
        // After the checks of the files themselves: a batch that can't run says why it can't.
        if let Some(why) = missing_models(&settings.language) {
            self.notify(why);
            return;
        }
        #[cfg(target_arch = "wasm32")]
        {
            // The files wait for the models rather than having to be chosen again.
            if self.web_models_missing(settings.engine) {
                self.ocr_web_waiting = Some((files, settings));
                return;
            }
            self.start_web_batch(files, settings);
            self.poll_ocr(ctx);
        }
        #[cfg(not(target_arch = "wasm32"))]
        self.run_ocr_files(ctx, files, settings, dir);
    }

    /// Whether Recognize Text in Multiple Files is running (on the web, or waiting for the
    /// models to download).
    fn ocr_batch_running(&self) -> bool {
        #[cfg(target_arch = "wasm32")]
        if self.ocr_web_waiting.is_some() {
            return true;
        }
        self.ocr_batch.is_some()
    }

    /// The web build: read the files in Web Workers (advanced by [`Self::poll_ocr`]).
    #[cfg(target_arch = "wasm32")]
    fn start_web_batch(&mut self, files: Vec<(String, Vec<u8>)>, settings: OcrSettings) {
        let count = Arc::new(JobCount::default());
        count.set(0, files.len());
        // Each file opens in its own session; it keeps this one's JavaScript preference.
        self.ocr_web_batch = Some(crate::ocr_web::Batch::new(files, settings, self.session.javascript()));
        self.ocr_batch = Some(count);
    }

    /// The web build: start the files that waited for the models once they are loaded, or give
    /// them up with the reason when the download failed.
    #[cfg(target_arch = "wasm32")]
    fn start_waiting_web_batch(&mut self) {
        let Some((_, settings)) = &self.ocr_web_waiting else { return };
        if pdfcraft_engine::ocr::engine_unavailable_reason(settings.engine).is_none() {
            if let Some((files, settings)) = self.ocr_web_waiting.take() {
                self.start_web_batch(files, settings);
            }
            return;
        }
        match crate::ocr_web::state() {
            crate::ocr_web::State::Loading => {}
            crate::ocr_web::State::Failed(e) => {
                self.ocr_web_waiting = None;
                self.notify(failed(&e));
            }
            // Not loading and not loaded (nothing has started the download): start it.
            crate::ocr_web::State::NotStarted => crate::ocr_web::start(self.wake.clone()),
        }
    }

    /// Recognize text in the files on the host's job queue.
    #[cfg(not(target_arch = "wasm32"))]
    fn run_ocr_files(&mut self, ctx: &egui::Context, files: Vec<(String, Vec<u8>)>, settings: OcrSettings, dir: std::path::PathBuf) {
        // The engines are built (and named if they cannot be) before anything starts.
        let recognizers = match pdfcraft_engine::ocr::recognizers(&settings) {
            Ok(r) => r,
            Err(why) => {
                self.notify_error(why);
                return;
            }
        };
        let count = Arc::new(JobCount::default());
        count.set(0, files.len());
        let counting = count.clone();
        let wake = self.wake.clone();
        let host = self.host.clone();
        // The summary is written on the worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        // Each file opens in its own session; it keeps this one's JavaScript preference.
        let javascript = self.session.javascript();
        self.ocr_batches.submit((), craft::host::queue::Priority::default(), move |_| {
            crate::i18n::set_current(lang);
            let write = |name: &str, r: &pdfcraft_engine::ocr::FileResult| {
                host.files().write(&dir.join(name), &r.bytes, craft::host::WriteIntent::SaveAs).map(|_| ()).map_err(|e| e.to_string())
            };
            // A panic in the recognizer still ends the batch with a summary (#3919).
            let msg = pdfcraft_engine::guard(|| run_batch_files(files, settings, &recognizers, javascript, &counting, &wake, write))
                .unwrap_or_else(|panic| failed(&panic));
            let (_, total) = counting.read();
            counting.set(total, total);
            msg
        });
        self.ocr_batch = Some(count);
        self.poll_ocr(ctx);
    }

    /// Ask the running batch to stop: the file in flight finishes, the rest are counted as not
    /// run.
    pub(crate) fn cancel_ocr_batch(&mut self) {
        if let Some(count) = &self.ocr_batch {
            count.cancel();
        }
    }

    /// Stop a running recognition (the pages read so far are kept).
    pub(crate) fn cancel_ocr(&mut self) {
        if let Some(run) = &self.ocr_run {
            run.count.cancel();
        }
        if let Some(b) = &self.ocr_batch
            && let Ok(mut s) = b.lock()
        {
            s.cancel = true;
        }
    }

    /// Show the OCR progress card, unless another job's card is showing.
    fn show_ocr_progress(&mut self, label: String, done: usize, total: usize, cancelling: bool) {
        if self.progress_notice.as_ref().is_some_and(|n| n.job != widgets::ProgressJob::Ocr) {
            return;
        }
        let label = if cancelling { tl!("Cancelling…").to_string() } else { label };
        let fraction = done as f32 / total.max(1) as f32;
        self.progress_notice = Some(widgets::ProgressNotice { label, fraction, cancellable: !cancelling, job: widgets::ProgressJob::Ocr });
        if let Some(ctx) = &self.ctx {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }
    }

    fn hide_ocr_progress(&mut self) {
        if self.progress_notice.as_ref().is_some_and(|n| n.job == widgets::ProgressJob::Ocr) {
            self.progress_notice = None;
        }
    }

    /// The web build downloads the models the first time they're needed: while `engine` can't
    /// run for want of them, start (or retry) the download, say so and return `true`.
    #[cfg(target_arch = "wasm32")]
    pub(crate) fn web_models_missing(&mut self, engine: EngineChoice) -> bool {
        if pdfcraft_engine::ocr::engine_unavailable_reason(engine).is_none() {
            return false;
        }
        if crate::ocr_web::state() != crate::ocr_web::State::Loading {
            crate::ocr_web::start(self.wake.clone());
        }
        self.notify(tl!("Downloading the text recognition models…"));
        true
    }

    /// The web build: advance the pages being read in Web Workers.
    #[cfg(target_arch = "wasm32")]
    fn pump_web_ocr(&mut self, ctx: &egui::Context) {
        self.start_waiting_web_batch();
        if self.views.iter().any(|v| !v.auto_ocr_awaiting_models.is_empty())
            && pdfcraft_engine::ocr::engine_unavailable_reason(EngineChoice::Ocrs).is_none()
        {
            self.ocr_models_loaded();
        }
        let mut busy = false;
        if let Some(run) = self.ocr_run.as_mut()
            && let Some(pages) = run.web.as_mut()
        {
            busy = true;
            let found = pages.pump(run.count.cancelled(), ctx);
            run.count.set(pages.done(), pages.total());
            if let Some(found) = found {
                run.web_result = Some(Ok(found));
                run.web = None;
            }
        }
        let mut summary = None;
        if let Some(batch) = self.ocr_web_batch.as_mut() {
            busy = true;
            let count = self.ocr_batch.clone();
            let finished = batch.pump(count.as_ref().is_some_and(|c| c.cancelled()), self.host.files(), ctx);
            if let Some(count) = &count {
                let (_, total) = count.read();
                count.set(batch.done(), total);
            }
            if finished {
                summary = Some(batch_summary(batch.ok, batch.words, batch.skipped, &batch.failed, &batch.notes, batch.not_run));
            }
        }
        if let Some(message) = summary {
            self.ocr_web_batch = None;
            self.ocr_batch = None;
            notices::end_progress(self, ProgressKey::Ocr);
            self.notify(message);
        }
        if busy {
            // The workers answer between frames: keep looking.
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        } else {
            crate::ocr_web::close_idle_workers(ctx);
        }
    }

    /// Show progress; apply the result once the job is done.
    pub(crate) fn poll_ocr(&mut self, ctx: &egui::Context) {
        #[cfg(target_arch = "wasm32")]
<<<<<<< ours
        self.pump_web_ocr();
        if let Some(b) = self.ocr_batch.clone() {
            let msg = b.lock().map(|mut s| s.message.take().ok_or((s.done, s.total, s.cancel))).map_err(|e| e.to_string());
            match msg {
                Ok(Ok(m)) => {
                    self.ocr_batch = None;
                    self.hide_ocr_progress();
                    self.notify(m);
                }
                Ok(Err((done, total, cancelling))) => {
                    let label = crate::i18n::fmt(
                        tl!("Recognizing text… file {d} of {t}"),
                        &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
                    );
                    self.show_ocr_progress(label, done, total, cancelling);
                }
                // The worker stopped while holding the lock: it will never report back (#3919).
                Err(e) => {
                    self.ocr_batch = None;
                    self.hide_ocr_progress();
                    self.notify(failed(&e));
                }
            }
=======
        self.pump_web_ocr(ctx);
        self.poll_ocr_batch();
        self.poll_ocr_run(ctx);
    }

    /// Posts progress on Recognize Text in Multiple Files while it moves, and its summary once the
    /// job has delivered it.
    fn poll_ocr_batch(&mut self) {
        #[cfg(not(target_arch = "wasm32"))]
        if let Some(finished) = self.ocr_batches.try_recv() {
            self.ocr_batch = None;
            notices::end_progress(self, ProgressKey::Ocr);
            self.notify(finished.result);
            return;
>>>>>>> theirs
        }
        let Some((done, total)) = self.ocr_batch.as_ref().and_then(|c| c.take_change()) else { return };
        let label = crate::i18n::fmt(
            tl!("Recognizing text… file {d} of {t}"),
            &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
        );
        notices::set_progress(self, ProgressKey::Ocr, ProgressNotice { label, fraction: ocr_fraction(done, total), cancellable: true });
    }

    /// Posts progress on a run the user asked for while it moves, and lands its result once the job
    /// has delivered it.
    fn poll_ocr_run(&mut self, ctx: &egui::Context) {
        let Some(run) = self.ocr_run.as_ref() else { return };
<<<<<<< ours
        let (doc, progress, settings, auto, guard, editable) =
            (run.doc, run.progress.clone(), run.settings.clone(), run.automatic, run.guard.clone(), run.editable);
        let automatic = auto.is_some();
        let mut s = match progress.lock() {
            Ok(s) => s,
            Err(e) => {
                // The worker stopped while holding the lock: it will never report back, and without
                // this the run never clears and refuses every later one (#3919).
                let e = e.to_string();
                self.ocr_run = None;
                self.hide_ocr_progress();
                self.notify(failed(&e));
                return;
            }
        };
=======
        let (doc, count, auto, guard, editable) = (run.doc, run.count.clone(), run.automatic, run.guard.clone(), run.editable);
        let automatic = auto.is_some();
>>>>>>> theirs
        // The result of a run Edit started waits while Read mode or a dialog is up: nothing may change
        // the document beneath them, and the run may have been started before they appeared. It stays
        // in the queue until then; a run the user asked for completes as before.
        if automatic && (self.mode == crate::Mode::Read || self.modal_open()) {
            return;
        }
<<<<<<< ours
        let cancelled = s.cancel;
        let Some(result) = s.result.take() else {
            let (done, total) = (s.done, s.total);
            drop(s);
            // An automatic run shows no progress: it must make no noise.
            if !automatic {
=======
        #[cfg(target_arch = "wasm32")]
        let finished = self.ocr_run.as_mut().and_then(|r| r.web_result.take());
        #[cfg(not(target_arch = "wasm32"))]
        let finished = self.ocr_runs.try_recv().map(|f| f.result);
        let Some(result) = finished else {
            if !automatic && let Some((done, total)) = count.take_change() {
>>>>>>> theirs
                let label = crate::i18n::fmt(
                    tl!("Recognizing text… page {d} of {t}"),
                    &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
                );
<<<<<<< ours
                self.show_ocr_progress(label, done, total, cancelled);
            }
            if let Some(ctx) = &self.ctx {
                ctx.request_repaint_after(std::time::Duration::from_millis(200));
=======
                notices::set_progress(self, ProgressKey::Ocr, ProgressNotice { label, fraction: ocr_fraction(done, total), cancellable: true });
>>>>>>> theirs
            }
            return;
        };
        if !automatic {
            notices::end_progress(self, ProgressKey::Ocr);
        }
        self.ocr_run = None;
<<<<<<< ours
        self.hide_ocr_progress();
=======
        self.land_ocr(ctx, doc, guard, auto, editable, result);
    }

    /// Applies a finished run's `result` to document `doc`: dropped for an automatic run whose
    /// document changed, handed to the OCR Verify screen for an editable output, else written as
    /// one undoable step (the guard refuses it when the user edited meanwhile).
    fn land_ocr(
        &mut self,
        ctx: &egui::Context,
        doc: DocId,
        guard: OcrGuard,
        auto: Option<AutoOcr>,
        editable: bool,
        result: Result<Vec<OcrPage>, String>,
    ) {
        let automatic = auto.is_some();
>>>>>>> theirs
        // The result is for the bytes the run started from. If the document has changed since (a
        // page deleted or moved, say) its page index may now name another page: drop it, and let
        // the page in view be judged again.
        if let Some(a) = auto
            && self.session.get(doc).map(|d| d.edit_generation()) != Some(a.generation)
        {
            if let Some(view) = self.views.iter_mut().find(|v| v.id == doc) {
                view.auto_ocr_tried.remove(&a.page);
            }
            return;
        }
        // SRS §8.2: Cancel leaves the document as it was, the pages read so far included.
        if cancelled {
            if !automatic {
                self.notify_tr("Cancelled");
            }
            return;
        }
        let found = match result {
            Ok(f) => f,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        // The dialog's "Editable text" choice is honored: the recognition is not written as an
        // invisible layer here. It opens in the OCR Verify screen, whose Accept produces the
        // editable document (a NEW file) the choice promises.
        if editable && !automatic {
            self.ocr_verify.load(doc, guard, found);
            self.ocr_verify.open = true;
            self.notify_tr("Review the recognition, then Accept to write the editable document");
            return;
        }
        let read = found.iter().filter(|p| p.skipped.is_none()).count();
        let unreadable = unreadable_pages(&found);
        let skipped = found.len().saturating_sub(read).saturating_sub(unreadable);
        // The guard refuses a result that no longer fits its document (the user edited, or
        // redacted, while the worker was out); its reason says so.
        match self.session.apply_ocr_checked(doc, &found, &guard) {
            Ok(words) => {
                if let Some(info) = self.session.get(doc).map(|d| d.info.clone())
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == doc)
                {
                    view.document_changed(ctx, &info);
                }
                // An automatic run that found nothing (a page that has text of its own) says nothing.
                if automatic && words == 0 {
                    return;
                }
                let words_part = if words == 1 {
                    crate::i18n::fmt(tl!("Recognized 1 word"), &[])
                } else {
                    crate::i18n::fmt(tl!("Recognized {n} words"), &[("n", &words.to_string())])
                };
                let pages_part = if read == 1 { tl!("1 page").to_string() } else { crate::i18n::fmt(tl!("{n} pages"), &[("n", &read.to_string())]) };
                let mut msg = crate::i18n::fmt(tl!("{words} on {pages}"), &[("words", &words_part), ("pages", &pages_part)]);
                if skipped > 0 {
                    msg.push_str(&if skipped == 1 {
                        crate::i18n::fmt(tl!("; 1 page already had text"), &[])
                    } else {
                        crate::i18n::fmt(tl!("; {n} pages already had text"), &[("n", &skipped.to_string())])
                    });
                }
                if unreadable > 0 {
                    msg.push_str(&unreadable_note());
                }
                // The pages' own notes (the editable output's unrenderable-character warning,
                // for one) are part of the result: said here, not dropped.
                for note in found.iter().flat_map(|p| p.notes.iter()) {
                    msg.push_str("; ");
                    msg.push_str(note);
                }
                self.notify(msg);
                // SRS FR-17: suggest reading the pages with unreadable text; only the user's
                // confirmation starts it.
                self.ocr_offer = UnreadableOffer::from_run(doc, &found, &settings);
            }
            Err(e) => self.notify_error(e),
        }
    }
}

/// The share of a job that is done, for a progress bar.
fn ocr_fraction(done: usize, total: usize) -> f32 {
    if total == 0 { 0.0 } else { (done as f32 / total as f32).clamp(0.0, 1.0) }
}

/// The batch's worker: read each file, hand the searchable copy to `write` (the output folder),
/// count the buckets, and compose the summary. A file the skip-files option passes over is its
/// own bucket and `write` is never called for it. A cancel (`count`'s) is asked for between
/// files: the file in flight finishes, the rest are counted as not run. Each file's start is
/// recorded in `count` and wakes the app through `wake`. `javascript` is the app session's
/// preference, for each file's own session.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn run_batch_files(
    files: Vec<(String, Vec<u8>)>,
    settings: OcrSettings,
    recognizers: &pdfcraft_engine::ocr::Recognizers,
    javascript: bool,
    count: &JobCount,
    wake: &craft::host::Wake,
    mut write: impl FnMut(&str, &pdfcraft_engine::ocr::FileResult) -> Result<(), String>,
) -> String {
    let total = files.len();
    let (mut ok, mut skipped, mut words, mut failed, mut notes) = (0usize, 0usize, 0usize, Vec::new(), Vec::new());
    let mut not_run: Option<usize> = None;
    for (i, (name, bytes)) in files.into_iter().enumerate() {
        if count.cancelled() {
            not_run = Some(total - i);
            break;
        }
        count.set(i, total);
        wake.wake();
        let r = pdfcraft_engine::ocr::recognize_file(&name, Arc::new(bytes), None, settings.clone(), recognizers, javascript, |_, _| true);
        match r {
            Ok(res) if res.skipped() => skipped += 1,
            Ok(res) => match write(&name, &res) {
                Ok(()) => {
                    ok += 1;
                    words += res.words();
                    if let Some(note) = low_confidence_note(&name, &res, settings.low_confidence_threshold) {
                        notes.push(note);
                    }
                }
                Err(e) => failed.push(format!("{name}: {e}")),
            },
            Err(e) => failed.push(format!("{name}: {e}")),
        }
    }
    batch_summary(ok, words, skipped, &failed, &notes, not_run)
}

/// The batch's low-confidence note for one file: its words at or under `threshold` (60 by
/// default), on 1-based sorted pages; `None` when it has none.
pub(crate) fn low_confidence_note(name: &str, res: &pdfcraft_engine::ocr::FileResult, threshold: f32) -> Option<String> {
    let (count, pages) = pdfcraft_engine::ocr::low_confidence_pages(&res.pages, threshold)?;
    let list = pages.iter().map(|p| p.to_string()).collect::<Vec<_>>().join(", ");
    Some(crate::i18n::fmt(tl!("{name}: {n} low-confidence word(s) on pages {pages}"), &[("name", name), ("n", &count.to_string()), ("pages", &list)]))
}

/// The batch summary: what was read, the skipped files as their own bucket, the failures, the
/// low-confidence notes and (on a cancel) how many files were not run.
fn batch_summary(ok: usize, words: usize, skipped: usize, failed: &[String], notes: &[String], not_run: Option<usize>) -> String {
    let mut msg = if ok == 1 {
        crate::i18n::fmt(tl!("Recognized {w} words in 1 file"), &[("w", &words.to_string())])
    } else {
        crate::i18n::fmt(tl!("Recognized {w} words in {n} files"), &[("w", &words.to_string()), ("n", &ok.to_string())])
    };
    if skipped > 0 {
        msg.push_str(&crate::i18n::fmt(tl!("; {n} files already had text"), &[("n", &skipped.to_string())]));
    }
    if !failed.is_empty() {
        msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
    }
    if let Some(rest) = not_run {
        msg.push_str(&crate::i18n::fmt(tl!("; cancelled — {n} files were not run"), &[("n", &rest.to_string())]));
    }
    for note in notes {
        msg.push_str("; ");
        msg.push_str(note);
    }
    msg
}

/// "Recognize text failed: {e}", in the current language.
fn failed(e: &str) -> String {
    crate::i18n::fmt(tl!("{label} failed: {e}"), &[("label", tl!("Recognize text")), ("e", e)])
}

/// `recognize`, with a panic in it (a damaged model, a page the recognizer can't take) turned into
/// the run's error: the run has to end either way, or it refuses every later one (#3919).
#[cfg(not(target_arch = "wasm32"))]
fn recognize_guarded<T>(recognize: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    pdfcraft_engine::guard(recognize).unwrap_or_else(|panic| Err(failed(&panic)))
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::ocr::{PlacedWord, UNUSABLE_TEXT};

    use super::*;
    use crate::{LeftPanel, Mode, QuickTool};
    use craft::host::{AppInfo, ManualJobs};
    use pdfcraft_engine::ocr::{Recognizer, Recognizers};
    use pdfcraft_engine::{Edit, Session};
    fn page(page: usize, skipped: Option<&str>, unusable_text: bool) -> OcrPage {
        let words = if skipped.is_some() {
            Vec::new()
        } else {
            vec![PlacedWord {
                text: "word".into(),
                origin: [72.0, 700.0],
                across: [40.0, 0.0],
                up: [0.0, 12.0],
                confidence: None,
                source: "ocrs".into(),
            }]
        };
        OcrPage { page, words, already_on_page: 0, skipped: skipped.map(Into::into), unusable_text, notes: Vec::new(), deskew: None }
    }

    /// A blank three-page document with `progress` as its running recognition.
    fn running(progress: Arc<Mutex<OcrProgress>>) -> Harness<'static, PdfCraftApp> {
        Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            let blank = pdfcraft_engine::Session::new().create_blank(612.0, 792.0, 3).unwrap().to_vec();
            app.open_bytes("scan.pdf", None, blank).unwrap();
            let (_, doc) = app.active_ids().unwrap();
            let guard = app.session.ocr_guard(doc).unwrap();
            app.ocr_run = Some(OcrRun { doc, progress, guard, settings: OcrSettings::default(), automatic: None, editable: false });
            app
        })
    }

    /// SRS FR-08 and §8.2: the run shows a card with Cancel, and a cancelled run leaves the
    /// document as it was, even with pages already read.
    #[test]
    fn cancel_stops_recognition_and_leaves_the_document_unchanged() {
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 3, ..Default::default() }));
        let mut h = running(progress.clone());
        h.run_steps(4);
        h.get_by_label("Recognizing text… page 2 of 3");
        let shown = h.state().progress_notice.clone().unwrap();
        assert!(shown.cancellable && shown.job == widgets::ProgressJob::Ocr, "{shown:?}");

        h.get_by_label("Cancel").click();
        h.run_steps(2);
        assert!(progress.lock().unwrap().cancel, "the worker is asked to stop");
        h.get_by_label("Cancelling…");
        assert!(h.query_by_label("Cancel").is_none(), "Cancel is not offered twice");

        progress.lock().unwrap().result = Some(Ok(vec![page(0, None, false), page(1, None, false)]));
        h.run_steps(2);
        let app = h.state();
        assert!(app.ocr_run.is_none() && app.progress_notice.is_none());
        assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Cancelled"));
        let (_, doc) = app.active_ids().unwrap();
        assert_eq!(app.session.get(doc).unwrap().can_undo(), None, "nothing was added");
    }

    /// SRS FR-17: pages skipped for unreadable text are told apart from pages that had text, and
    /// the summary names the option that reads them.
    #[test]
    fn the_summary_names_pages_with_unreadable_text() {
        let found = vec![page(0, None, false), page(1, Some("the page already contains text"), false), page(2, Some(UNUSABLE_TEXT), true)];
        let progress = Arc::new(Mutex::new(OcrProgress { done: 3, total: 3, result: Some(Ok(found)), ..Default::default() }));
        let mut h = running(progress);
        h.run_steps(2);
        let app = h.state();
        assert_eq!(
            app.toast.as_ref().map(|t| t.0.as_str()),
            Some(
                "Recognized 1 word on 1 page; 1 page already had text; some pages have unreadable text (turn on “Also recognize pages that already have text” to read them)"
            )
        );
        let (_, doc) = app.active_ids().unwrap();
        assert_eq!(app.session.get(doc).unwrap().can_undo(), Some("Recognize text"));
    }

    /// SRS FR-17 / N9: a run that skipped pages for unreadable text offers to read them and
    /// starts nothing by itself; "Not now" drops the offer.
    #[test]
    fn unreadable_pages_are_offered_and_wait_for_confirmation() {
        let found = vec![page(0, None, false), page(1, Some("the page already contains text"), false), page(2, Some(UNUSABLE_TEXT), true)];
        let progress = Arc::new(Mutex::new(OcrProgress { done: 3, total: 3, result: Some(Ok(found)), ..Default::default() }));
        let mut h = running(progress);
        h.run_steps(3);
        h.get_by_label("1 page has unreadable text. Recognize text on it? Its text stays, and you can undo this in one step.");
        {
            let app = h.state();
            assert_eq!(app.ocr_offer.as_ref().map(|o| o.pages.clone()), Some(vec![2]));
            assert!(app.ocr_run.is_none(), "nothing is read before the user confirms");
        }
        h.get_by_label("Not now").click();
        h.run_steps(2);
        assert!(h.state().ocr_offer.is_none());
        assert!(h.query_by_label("Recognize these pages").is_none());
        let (_, doc) = h.state().active_ids().unwrap();
        assert_eq!(h.state().session.get(doc).unwrap().can_undo(), Some("Recognize text"), "only the first run was applied");
    }

    /// A run without unreadable pages offers nothing.
    #[test]
    fn no_offer_without_unreadable_pages() {
        let found = vec![page(0, None, false), page(1, Some("the page already contains text"), false)];
        let progress = Arc::new(Mutex::new(OcrProgress { done: 2, total: 2, result: Some(Ok(found)), ..Default::default() }));
        let mut h = running(progress);
        h.run_steps(3);
        assert!(h.state().ocr_offer.is_none());
        assert!(h.query_by_label("Recognize these pages").is_none());
    }

    /// Confirming reads just the offered pages, skipping any page whose text is fine, as a new
    /// run with the first run's language and resolution.
    #[test]
    fn confirming_reads_only_the_unreadable_pages() {
        let settings = OcrSettings { dpi: 150.0, language: "th".into(), skip_text_pages: true, ..Default::default() };
        let found =
            vec![page(0, Some(UNUSABLE_TEXT), true), page(1, Some("the page already contains text"), false), page(2, Some(UNUSABLE_TEXT), true)];
        let doc = DocId(1);
        let offer = UnreadableOffer::from_run(doc, &found, &settings).unwrap();
        assert_eq!(offer.pages, vec![0, 2]);
        let confirmed = offer.confirmed();
        assert!(confirmed.skip_text_pages && confirmed.read_unusable_text, "good pages stay skipped: {confirmed:?}");
        assert_eq!((confirmed.dpi, confirmed.language.as_str()), (150.0, "th"));
        assert!(UnreadableOffer::from_run(doc, &found[1..2], &settings).is_none());

        let found = vec![page(0, None, false), page(1, Some(UNUSABLE_TEXT), true), page(2, Some(UNUSABLE_TEXT), true)];
        let progress = Arc::new(Mutex::new(OcrProgress { done: 3, total: 3, result: Some(Ok(found)), ..Default::default() }));
        let mut h = running(progress);
        h.run_steps(3);
        h.get_by_label("2 pages have unreadable text. Recognize text on just those pages? Their text stays, and you can undo this in one step.");
        // Inline, so the confirmed run finishes within the click's frame.
        h.state_mut().run_inline = true;
        h.get_by_label("Recognize these pages").click();
        h.run_steps(2);
        let app = h.state();
        assert!(app.ocr_offer.is_none(), "the offer is used up");
        if pdfcraft_engine::ocr::available() {
            // The blank pages have no text of their own, so both offered pages are read.
            assert_eq!(app.toast.as_ref().map(|t| t.0.as_str()), Some("Recognized 0 words on 2 pages"), "pages 2 and 3 only");
        } else {
            assert!(app.ocr_run.is_none());
            assert!(app.toast.as_ref().is_some_and(|t| t.0.contains("isn't installed")), "{:?}", app.toast);
        }
    }

    /// A one-page PDF that is one picture covering the page.
    fn scan() -> Vec<u8> {
        let img = craft_image_codecs::Image::from_u8(8, 8, craft_image_codecs::ChannelLayout::Rgba, [250, 250, 250, 255].repeat(64)).unwrap();
        let png = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
        Session::new().create_from_images(&[("scan.png".into(), png)]).unwrap().to_vec()
    }

    fn app_with(pdf: Vec<u8>) -> PdfCraftApp {
        let mut app = PdfCraftApp::for_unit_tests();
        app.open_bytes(&egui::Context::default(), "doc.pdf", None, pdf).unwrap();
        app.quick_tool = QuickTool::EditText;
        app
    }

    fn found(app: &mut PdfCraftApp) -> Option<usize> {
        app.scan_to_read().map(|s| s.page)
    }

    /// The app on a host whose jobs run only when the test runs them, with the scan open and Edit on.
    fn app_on_manual_jobs() -> (PdfCraftApp, Arc<ManualJobs>) {
        let jobs = Arc::new(ManualJobs::new());
        let host = craft::host::Host::builder(AppInfo { id: "pdfcraft-test", name: "PdfCraft", version: "0" }).jobs(jobs.clone()).build();
        let mut app = PdfCraftApp::for_unit_tests_on(Arc::new(host));
        app.open_bytes(&egui::Context::default(), "doc.pdf", None, scan()).unwrap();
        app.quick_tool = QuickTool::EditText;
        (app, jobs)
    }

    /// Closes every dialog the host holds, as the user closing them would.
    fn close_dialogs(app: &mut PdfCraftApp) {
        let ctx = egui::Context::default();
        let ids: Vec<_> = app.dialogs.open_dialogs().iter().map(|d| d.id).collect();
        for id in ids {
            app.dialogs.cancel_by_id(&ctx, id);
        }
        // The JavaScript console is a window beside the host; closing it drops it.
        app.js_dialog = None;
    }

    /// The text of the notices posted so far.
    fn posted(app: &mut PdfCraftApp) -> String {
        crate::notices::texts(app, &egui::Context::default()).join("; ")
    }

    /// A page of one confident word.
    fn one_word_page(confidence: Option<f32>, notes: Vec<String>) -> OcrPage {
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence,
            source: "ocrs".into(),
        };
        OcrPage { page: 0, words: vec![word], already_on_page: 0, skipped: None, notes }
    }

    /// A run on the active document, as the start of one leaves it.
    fn run_of(app: &PdfCraftApp, automatic: bool, editable: bool) -> OcrRun {
        let doc = app.active_ids().unwrap().1;
        let generation = app.session.get(doc).unwrap().edit_generation();
        OcrRun {
            doc,
            count: Arc::default(),
            guard: app.session.ocr_guard(doc).unwrap(),
            automatic: automatic.then_some(AutoOcr { page: 0, generation }),
            editable,
        }
    }

    /// Puts `result` in the runs' queue as the job would, by running the host's waiting jobs.
    fn deliver(app: &PdfCraftApp, jobs: &ManualJobs, doc: DocId, result: Result<Vec<OcrPage>, String>) {
        app.ocr_runs.submit(doc, craft::host::queue::Priority::default(), move |_| result);
        jobs.run_all();
        assert!(app.ocr_runs.counts().finished > 0, "the result reached the queue");
    }

    /// These rules need no OCR models (the run itself judges the page's own text), so they are
    /// tested everywhere, unlike the tests that recognize something.
    #[test]
    fn edit_reads_a_page_that_is_a_picture_and_no_other() {
        assert_eq!(found(&mut app_with(scan())), Some(0));
        let text = Session::new().create_from_text("t", "Real text, no picture").unwrap().to_vec();
        assert_eq!(found(&mut app_with(text)), None, "a page of text is no scan");
        let blank = Session::new().create_blank(595.0, 842.0, 1).unwrap().to_vec();
        assert_eq!(found(&mut app_with(blank)), None, "nor is a blank page");
    }

    #[test]
    fn only_edit_being_on_starts_it() {
        let mut app = app_with(scan());
        app.quick_tool = QuickTool::Select;
        assert_eq!(found(&mut app), None, "another tool");
        // The Edit tab's panel counts, as for the page view, with no tool picked.
        app.left = LeftPanel::Tool("edit");
        app.set_left_open(true);
        assert_eq!(found(&mut app), Some(0), "the Edit panel");
        app.set_left_open(false);
        assert_eq!(found(&mut app), None, "the Edit panel closed");
    }

    #[test]
    fn it_waits_for_read_mode_and_dialogs_to_end() {
        let mut app = app_with(scan());
        app.mode = Mode::Read;
        assert_eq!(found(&mut app), None, "Read mode");
        app.mode = Mode::Edit;
        crate::js_dialogs::open_console(&mut app);
        assert_eq!(found(&mut app), None, "beneath a dialog");
        close_dialogs(&mut app);
        assert_eq!(found(&mut app), Some(0), "tried again once they are gone");
    }

    #[test]
    fn a_page_is_read_once_and_a_page_found_no_scan_is_judged_again_after_a_change() {
        let mut app = app_with(scan());
        app.views[0].auto_ocr_tried.insert(0);
        assert_eq!(found(&mut app), None, "a page already read is not read again (Undo sticks)");

        // A blank page is no scan; it is judged again only once the document changes.
        let blank = Session::new().create_blank(595.0, 842.0, 1).unwrap().to_vec();
        let mut app = app_with(blank);
        assert_eq!(found(&mut app), None);
        assert!(app.views[0].auto_ocr_skipped.contains_key(&0), "remembered with the document's generation");
        let id = app.active_ids().unwrap().1;
        let img = craft_image_codecs::Image::from_u8(8, 8, craft_image_codecs::ChannelLayout::Rgba, [250, 250, 250, 255].repeat(64)).unwrap();
        let png = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
        let add = Edit::AddImage { page: 0, rect: Some([0.0, 0.0, 595.0, 842.0]), name: "scan.png".into(), bytes: Arc::new(png) };
        app.session.apply(id, add).unwrap();
        assert_eq!(found(&mut app), Some(0), "a picture over the page makes it a scan");
    }
    /// A scan found while no engine could read it waits for one, and is read once the models
    /// load, though the document has not changed. Before, it was taken for no scan and left unread.
    #[test]
    fn a_scan_found_without_the_models_is_read_once_they_load() {
        let mut app = app_with(scan());
        let scan = app.scan_to_read().unwrap();
        app.await_ocr_models(&scan);
        assert_eq!(found(&mut app), None, "it waits for the models, not judged every frame");
        app.ocr_models_loaded();
        assert_eq!(found(&mut app), Some(0), "read once they are in");
        assert!(app.views[0].auto_ocr_skipped.is_empty(), "never taken for no scan");
    }
    /// A finished run Edit started is applied only once Read mode and dialogs are gone, and waits
    /// in the queue until then; one the user asked for is applied at once. The result is made up,
    /// so no OCR models are needed.
    #[test]
    fn the_result_of_an_automatic_run_waits_for_read_mode_and_dialogs() {
<<<<<<< ours
        let word = || pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: None,
            source: "ocrs".into(),
        };
        let finished = |app: &PdfCraftApp, automatic: bool| {
            let generation = app.session.get(app.active_ids().unwrap().1).unwrap().edit_generation();
            let page = pdfcraft_engine::ocr::OcrPage {
                page: 0,
                words: vec![word()],
                already_on_page: 0,
                skipped: None,
                unusable_text: false,
                notes: Vec::new(),
                deskew: None,
            };
            let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
            let guard = app.session.ocr_guard(app.active_ids().unwrap().1).unwrap();
            OcrRun {
                doc: app.active_ids().unwrap().1,
                progress,
                guard,
                settings: OcrSettings::default(),
                automatic: automatic.then_some(AutoOcr { page: 0, generation }),
                editable: false,
            }
        };
=======
>>>>>>> theirs
        let label = |app: &PdfCraftApp| app.session.get(app.active_ids().unwrap().1).unwrap().can_undo().map(String::from);

        let (mut app, jobs) = app_on_manual_jobs();
        let doc = app.active_ids().unwrap().1;
        app.ocr_run = Some(run_of(&app, true, false));
        deliver(&app, &jobs, doc, Ok(vec![one_word_page(None, Vec::new())]));
        app.mode = Mode::Read;
        app.poll_ocr(&egui::Context::default());
        assert!(app.ocr_run.is_some() && label(&app).is_none(), "waits in Read mode");
        assert_eq!(app.ocr_runs.counts().finished, 1, "the result stays in the queue");
        app.mode = Mode::Edit;
        crate::js_dialogs::open_console(&mut app);
        app.poll_ocr(&egui::Context::default());
        assert!(app.ocr_run.is_some() && label(&app).is_none(), "waits beneath a dialog");
        close_dialogs(&mut app);
        app.poll_ocr(&egui::Context::default());
        assert!(app.ocr_run.is_none(), "taken once they are gone");
        assert!(label(&app).is_some(), "one undo step was added");

        // A run the user asked for is not held back.
        let (mut app, jobs) = app_on_manual_jobs();
        let doc = app.active_ids().unwrap().1;
        app.ocr_run = Some(run_of(&app, false, false));
        deliver(&app, &jobs, doc, Ok(vec![one_word_page(None, Vec::new())]));
        app.mode = Mode::Read;
        app.poll_ocr(&egui::Context::default());
        assert!(label(&app).is_some(), "one undo step was added");
    }
    /// A run's result is for the document as it was when the run started. Once the document has
    /// changed, a result of Edit's is dropped, not applied to whatever page now has its index, and
    /// the page may be judged again.
    #[test]
    fn the_result_of_an_automatic_run_is_dropped_once_the_document_has_changed() {
        let (mut app, jobs) = app_on_manual_jobs();
        let id = app.active_ids().unwrap().1;
<<<<<<< ours
        let generation = app.session.get(id).unwrap().edit_generation();
        let page = pdfcraft_engine::ocr::OcrPage {
            page: 0,
            words: vec![pdfcraft_engine::ocr::PlacedWord {
                text: "scanned".into(),
                origin: [72.0, 700.0],
                across: [60.0, 0.0],
                up: [0.0, 12.0],
                confidence: None,
                source: "ocrs".into(),
            }],
            already_on_page: 0,
            skipped: None,
            unusable_text: false,
            notes: Vec::new(),
            deskew: None,
        };
        let progress = Arc::new(Mutex::new(OcrProgress { done: 1, total: 1, result: Some(Ok(vec![page])), cancel: false }));
        let guard = app.session.ocr_guard(id).unwrap();
        app.ocr_run = Some(OcrRun {
            doc: id,
            progress,
            guard,
            settings: OcrSettings::default(),
            automatic: Some(AutoOcr { page: 0, generation }),
            editable: false,
        });
=======
        app.ocr_run = Some(run_of(&app, true, false));
        deliver(&app, &jobs, id, Ok(vec![one_word_page(None, Vec::new())]));
>>>>>>> theirs
        app.views[0].auto_ocr_tried.insert(0);
        // The document changes while the run is out.
        let img = craft_image_codecs::Image::from_u8(8, 8, craft_image_codecs::ChannelLayout::Rgba, [250, 250, 250, 255].repeat(64)).unwrap();
        let png = craft_image_codecs::encode(&img, craft_image_codecs::Format::Png, &craft_image_codecs::EncodeOptions::default()).unwrap();
        app.session.apply(id, Edit::AddImage { page: 0, rect: Some([10.0, 10.0, 50.0, 50.0]), name: "p.png".into(), bytes: Arc::new(png) }).unwrap();
        app.poll_ocr(&egui::Context::default());
        assert!(app.ocr_run.is_none(), "the run is over");
        assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Add image"), "its result was not applied");
        assert!(!app.views[0].auto_ocr_tried.contains(&0), "the page may be judged again");
    }

    #[test]
    fn a_panicking_recognizer_ends_the_run_with_an_error() {
        let error = recognize_guarded(|| -> Result<(), String> { panic!("the model file is damaged") }).unwrap_err();
        assert!(error.starts_with("Recognize text failed") && error.contains("the model file is damaged"), "{error}");
        assert_eq!(recognize_guarded(|| Ok::<_, String>(7)), Ok(7));
    }

<<<<<<< ours
    /// Poison `m` the way a worker that panics while it holds the lock does.
    fn poison<T: Send + 'static>(m: &Arc<Mutex<T>>) {
        let worker = m.clone();
        let _ = std::thread::spawn(move || {
            let _held = worker.lock();
            panic!("the worker died");
        })
        .join();
        assert!(m.is_poisoned());
    }

    #[test]
    fn a_run_whose_worker_died_ends_instead_of_waiting_forever() {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        let pdf = pdfcraft_engine::Session::new().create_from_text("t", "guard fixture").unwrap();
        app.open_bytes("a.pdf", None, pdf.to_vec()).unwrap();
        let id = app.active_ids().unwrap().1;
        let guard = app.session.ocr_guard(id).unwrap();
        let progress = Arc::new(Mutex::new(OcrProgress { total: 3, ..Default::default() }));
        poison(&progress);
        app.ocr_run = Some(OcrRun { doc: id, progress, guard, settings: OcrSettings::default(), automatic: None, editable: false });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the next run isn't refused as already running");
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.starts_with("Recognize text failed"), "{toast}");

        let batch = Arc::new(Mutex::new(BatchProgress { total: 2, ..Default::default() }));
        poison(&batch);
        app.ocr_batch = Some(batch);
        app.poll_ocr();
        assert!(app.ocr_batch.is_none());
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.starts_with("Recognize text failed"), "{toast}");
    }

=======
>>>>>>> theirs
    /// A fake engine: one confident word per call, no filesystem, no models needed.
    struct Fake {
        confidence: f32,
    }

    impl Recognizer for Fake {
        fn id(&self) -> &'static str {
            "fake"
        }
        fn available(&self) -> bool {
            true
        }
        fn languages(&self) -> Vec<(String, String)> {
            vec![("en".into(), "English".into())]
        }
        fn recognize(
            &self,
            _image: &pdfcraft_ocr::OcrImage,
            _options: &pdfcraft_ocr::RecognizeOptions,
        ) -> Result<Vec<pdfcraft_ocr::Line>, pdfcraft_engine::ocr::OcrError> {
            Ok(vec![pdfcraft_ocr::Line {
                words: vec![pdfcraft_ocr::Word::new("word", [1.0, 1.0, 9.0, 9.0], "fake").with_confidence(self.confidence)],
            }])
        }
    }

    /// A batch over two text files (the engine never runs on them: they are skipped files):
    /// the summary counts them in their own bucket, nothing is written, and a cancel before
    /// the first file leaves the rest not run.
    #[test]
    fn batch_counts_skipped_files_and_honours_a_cancel() {
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "Already typed here").expect("fixture");
        let files = vec![("a.pdf".into(), pdf.to_vec()), ("b.pdf".into(), pdf.to_vec())];
        let count = JobCount::default();
        count.cancel();
        let wake = craft::host::Wake::none();
        let recognizers = Recognizers { primary: Arc::new(Fake { confidence: 95.0 }), secondary: None };
        let writes = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let w = writes.clone();
        let msg =
            run_batch_files(files, OcrSettings { skip_text_files: true, ..Default::default() }, &recognizers, true, &count, &wake, move |_, _| {
                w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            });
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0, "nothing is written for skipped files, and the cancel came first");
        assert!(msg.contains("cancelled — 2 files were not run"), "{msg}");
        // Without the cancel, the skipped bucket is named and still nothing is written.
        let files = vec![("a.pdf".into(), pdf.to_vec()), ("b.pdf".into(), pdf.to_vec())];
        let count = JobCount::default();
        let w = writes.clone();
        let msg =
            run_batch_files(files, OcrSettings { skip_text_files: true, ..Default::default() }, &recognizers, true, &count, &wake, move |_, _| {
                w.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(())
            });
        assert_eq!(writes.load(std::sync::atomic::Ordering::SeqCst), 0);
        assert!(msg.contains("; 2 files already had text"), "{msg}");
        assert!(msg.contains("Recognized 0 words in 0 files"), "{msg}");
    }

    /// A batch over scanned files: the words are counted, and a low-confidence reading lands
    /// in the summary as a note with its 1-based sorted pages.
    #[test]
    fn batch_notes_low_confidence_pages() {
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "fake scan").expect("fixture");
        let files = vec![("scan.pdf".into(), pdf.to_vec()), ("clean.pdf".into(), pdf.to_vec())];
        let count = JobCount::default();
        let wake = craft::host::Wake::none();
        let recognizers = Recognizers { primary: Arc::new(Fake { confidence: 40.0 }), secondary: None };
        // The fixtures are text PDFs standing in for scans; the default skip-text-pages option
        // would (correctly) leave them alone, so the batch here reads everything.
        let settings = OcrSettings { skip_text_pages: false, ..Default::default() };
        let msg = run_batch_files(files, settings.clone(), &recognizers, true, &count, &wake, |_, _| Ok(()));
        assert!(msg.contains("Recognized 2 words in 2 files"), "{msg}");
        assert!(msg.contains("scan.pdf: 1 low-confidence word(s) on pages 1"), "{msg}");
        // The write failing is a per-file failure, named in the summary.
        let files = vec![("scan.pdf".into(), pdf.to_vec())];
        let count = JobCount::default();
        let msg = run_batch_files(files, settings, &recognizers, true, &count, &wake, |_, _| Err("disk full".into()));
        assert!(msg.contains("; failed: scan.pdf: disk full"), "{msg}");
    }

    /// The batch note follows the settings' low-confidence threshold (60 by default), not a
    /// number of its own: a 40% word is flagged under 60 and under 40, and not under 30.
    #[test]
    fn the_batch_note_uses_the_settings_threshold() {
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "fake scan").expect("fixture");
        let recognizers = Recognizers { primary: Arc::new(Fake { confidence: 40.0 }), secondary: None };
        let run = |threshold: f32| {
            let files = vec![("scan.pdf".into(), pdf.to_vec())];
            let count = JobCount::default();
            let settings = OcrSettings { skip_text_pages: false, low_confidence_threshold: threshold, ..Default::default() };
            run_batch_files(files, settings, &recognizers, true, &count, &craft::host::Wake::none(), |_, _| Ok(()))
        };
        assert!(run(OcrSettings::default().low_confidence_threshold).contains("1 low-confidence word(s)"), "the default flags a 40% word");
        assert!(run(40.0).contains("1 low-confidence word(s)"), "at the threshold counts");
        assert!(!run(30.0).contains("low-confidence"), "a lower threshold leaves the word alone: {}", run(30.0));
    }

    /// The remembered output folder survives the preferences round trip; an absent or empty
    /// value keeps the default (ask each time).
    #[test]
    fn the_output_folder_is_persisted_with_the_draft() {
        let mut d = OcrDraft::default();
        assert_eq!(d.output_folder, None, "by default the folder is asked for");
        d.output_folder = Some("out".into());
        let mut r = OcrDraft::default();
        r.apply_pref(&d.to_pref());
        assert_eq!(r.output_folder.as_deref(), Some("out"));
        let mut e = OcrDraft::default();
        e.apply_pref(&serde_json::json!({}));
        assert_eq!(e.output_folder, None, "an absent value keeps the default");
        e.output_folder = Some("kept".into());
        e.apply_pref(&serde_json::json!({ "output_folder": "" }));
        assert_eq!(e.output_folder.as_deref(), Some("kept"), "an empty value is not a folder");
    }

    /// "Automatic" saved without a strategy reads the way Automatic reads by default (the
    /// second engine joins when the first is unsure); an explicit choice always stands.
    #[test]
    fn an_automatic_engine_without_a_saved_strategy_is_confidence_weighted() {
        let mut d = OcrDraft::default();
        d.apply_pref(&serde_json::json!({ "engine": "auto" }));
        assert_eq!((d.engine, d.strategy), (EngineChoice::Auto, MergeStrategy::ConfidenceWeighted));
        let mut named = OcrDraft::default();
        named.apply_pref(&serde_json::json!({ "engine": "tesseract" }));
        assert_eq!(named.strategy, MergeStrategy::PrimaryOnly, "a named engine reads alone");
        let mut explicit = OcrDraft::default();
        explicit.apply_pref(&serde_json::json!({ "engine": "auto", "strategy": "primary" }));
        assert_eq!((explicit.engine, explicit.strategy), (EngineChoice::Auto, MergeStrategy::PrimaryOnly));
    }

    /// Two inputs of one file name (from different folders, or differing only in case) would
    /// write the same output: the batch stops before anything is read or written.
    #[test]
    fn inputs_that_share_a_file_name_are_refused_before_anything_is_read() {
        let mut app = PdfCraftApp::for_unit_tests();
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-same-name-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        app.ocr_draft.output_folder = Some(dir.to_string_lossy().into_owned());
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "scanned words here").expect("fixture");
        app.ocr_files(
            &egui::Context::default(),
            vec![("scan.pdf".into(), pdf.to_vec()), ("other.pdf".into(), pdf.to_vec()), ("Scan.PDF".into(), pdf.to_vec())],
        );
        assert!(app.ocr_batch.is_none(), "the batch never starts");
        assert_eq!(app.ocr_batches.counts().running + app.ocr_batches.counts().queued, 0, "nothing was submitted");
        let toast = posted(&mut app);
        assert!(toast.contains("same name") && toast.contains("Scan.PDF"), "{toast}");
        assert_eq!(std::fs::read_dir(&dir).expect("fixture").count(), 0, "nothing was written");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A remembered output folder is used without asking, is remembered again after the run,
    /// and its collision pre-flight still stops the batch before the first page is read.
    #[test]
    fn a_remembered_output_folder_is_used_and_still_preflights_collisions() {
        let mut app = PdfCraftApp::for_unit_tests();
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-folder-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("fixture");
        std::fs::write(dir.join("a.pdf"), b"would be overwritten").expect("fixture");
        app.ocr_draft.output_folder = Some(dir.to_string_lossy().into_owned());
        let s = pdfcraft_engine::Session::new();
        let pdf = s.create_from_text("t", "scanned words here").expect("fixture");
        app.ocr_files(&egui::Context::default(), vec![("a.pdf".into(), pdf.to_vec()), ("b.pdf".into(), pdf.to_vec())]);
        assert!(app.ocr_batch.is_none(), "the collision stops the batch before anything is read");
        let toast = posted(&mut app);
        assert!(toast.contains("already holds files of these names"), "{toast}");
        assert!(toast.contains("a.pdf"), "the colliding names are in the error: {toast}");
        assert_eq!(app.ocr_draft.output_folder.as_deref(), Some(dir.to_string_lossy().as_ref()), "the folder stays remembered for the next batch");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A result that arrives after the document changed (here: a page turned while the worker was
    /// reading; the same applies to a redaction applied meanwhile) is refused with the reason and
    /// writes nothing, so words read before a redaction cannot come back as invisible text.
    #[test]
    fn a_result_that_arrives_after_the_document_changed_is_refused() {
        let mut app = PdfCraftApp::for_unit_tests();
        let pdf = pdfcraft_engine::Session::new().create_from_text("t", "Typed text").expect("fixture");
        app.open_bytes(&egui::Context::default(), "a.pdf", None, pdf.to_vec()).expect("opens");
        let id = app.active_ids().expect("a document").1;
        let guard = app.session.ocr_guard(id).expect("a guard");
        app.session.apply(id, pdfcraft_engine::Edit::RotatePages { pages: vec![0], degrees: 90 }).expect("an edit while the worker runs");
        let after_edit = app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned));
<<<<<<< ours
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "123-45-6789".into(),
            origin: [40.0, 40.0],
            across: [50.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "fake".into(),
        };
        let page = OcrPage { unusable_text: false, page: 0, words: vec![word], already_on_page: 0, skipped: None, notes: Vec::new(), deskew: None };
        let progress = Arc::new(Mutex::new(OcrProgress { total: 1, done: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_run = Some(OcrRun { settings: OcrSettings::default(), doc: id, progress, guard, automatic: None, editable: false });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the run is over");
        let toast = app.toast.clone().map(|t| t.0).unwrap_or_default();
=======
        let page = one_word_page(Some(95.0), Vec::new());
        app.land_ocr(&egui::Context::default(), id, guard, None, false, Ok(vec![page]));
        let toast = posted(&mut app);
>>>>>>> theirs
        assert!(toast.contains("changed since recognition started"), "{toast}");
        assert_eq!(
            app.session.get(id).and_then(|d| d.can_undo().map(str::to_owned)),
            after_edit,
            "nothing was written: the last undoable step is still the edit"
        );
    }

    /// A page's notes survive to the result the user sees: the editable output's
    /// unrenderable-character warning ("N characters had no font … appear as ?") is said in
    /// the run's summary, not dropped. The result is fabricated, so no models are needed.
    #[test]
    fn the_result_summary_names_the_substituted_characters() {
        let mut app = app_with(scan());
        let id = app.active_ids().unwrap().1;
<<<<<<< ours
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "ocrs".into(),
        };
        let page = pdfcraft_engine::ocr::OcrPage {
            page: 0,
            words: vec![word],
            already_on_page: 0,
            skipped: None,
            unusable_text: false,
            notes: vec!["3 characters had no font that could show them and appear as ? in the output".into()],
            deskew: None,
        };
        let guard = app.session.ocr_guard(id).unwrap();
        let progress = Arc::new(Mutex::new(OcrProgress { total: 1, done: 1, result: Some(Ok(vec![page])), cancel: false }));
        app.ocr_run = Some(OcrRun { settings: OcrSettings::default(), doc: id, progress, guard, automatic: None, editable: false });
        app.poll_ocr();
        let toast = app.toast.take().map(|t| t.0).unwrap_or_default();
        assert!(toast.contains("3 characters had no font"), "{toast}");
=======
        let note = "3 characters had no font that could show them and appear as ? in the output";
        let page = one_word_page(Some(95.0), vec![note.into()]);
        let guard = app.session.ocr_guard(id).unwrap();
        app.land_ocr(&egui::Context::default(), id, guard, None, false, Ok(vec![page]));
        let toast = posted(&mut app);
        assert!(toast.contains(note), "{toast}");
>>>>>>> theirs
    }

    /// The dialog's "Editable text" output is honored: a run that promised an editable document
    /// does not quietly write the invisible searchable layer — its result opens in the OCR
    /// Verify screen, whose Accept produces the editable document. The result is fabricated,
    /// so no models are needed.
    #[test]
    fn an_editable_output_run_opens_the_review_instead_of_applying() {
        let mut app = app_with(scan());
        let id = app.active_ids().unwrap().1;
<<<<<<< ours
        let word = pdfcraft_engine::ocr::PlacedWord {
            text: "scanned".into(),
            origin: [72.0, 700.0],
            across: [60.0, 0.0],
            up: [0.0, 12.0],
            confidence: Some(95.0),
            source: "ocrs".into(),
        };
        let page = OcrPage { unusable_text: false, page: 0, words: vec![word], already_on_page: 0, skipped: None, notes: Vec::new(), deskew: None };
=======
        let page = one_word_page(Some(95.0), Vec::new());
>>>>>>> theirs
        let guard = app.session.ocr_guard(id).unwrap();
        app.ocr_draft.output_mode = pdfcraft_engine::ocr::OutputMode::EditableText;
<<<<<<< ours
        app.ocr_run = Some(OcrRun { settings: OcrSettings::default(), doc: id, progress, guard, automatic: None, editable: true });
        app.poll_ocr();
        assert!(app.ocr_run.is_none(), "the run is over");
=======
        app.land_ocr(&egui::Context::default(), id, guard, None, true, Ok(vec![page]));
>>>>>>> theirs
        assert_eq!(app.session.get(id).unwrap().can_undo(), None, "no invisible layer was written");
        assert!(app.ocr_verify.open, "the review opens, to accept as an editable document");
        assert_eq!(app.ocr_verify.phase, crate::workflows::ocr_verify::Phase::ReviewReady);
    }
}

#[cfg(test)]
mod deskew_choice_tests {
    use super::*;

    /// Straightening and the orientation check are on by default; correcting the document itself is
    /// a separate, off-by-default choice that reaches the engine as the persistent mode.
    #[test]
    fn straightening_choices_reach_the_engine_settings() {
        let mut d = OcrDraft::default();
        let s = d.settings();
        assert!(s.deskew && s.auto_rotate, "every OCR run checks the page first");
        assert_eq!(s.deskew_mode, DeskewMode::Transient);
        d.deskew_in_document = true;
        assert_eq!(d.settings().deskew_mode, DeskewMode::Persistent);
    }

    /// The choice is remembered with the other OCR preferences.
    #[test]
    fn the_document_choice_is_a_preference() {
        let d = OcrDraft { deskew_in_document: true, ..Default::default() };
        let pref = d.to_pref();
        assert_eq!(pref["deskew_in_document"], true);
        let mut back = OcrDraft::default();
        back.apply_pref(&pref);
        assert!(back.deskew_in_document);
    }
}
