//! Scan & OCR ▸ Recognize text: the Recognize Text dialog (pages, language, output, resolution)
//! and the background run with its progress. Recognition happens on a worker thread; the result
//! is applied as one undoable edit when it is done.

use std::sync::{Arc, Mutex};

use egui::{Align, Layout};
use pdfcraft_engine::DocId;
use pdfcraft_engine::ocr::{Digits, LANGUAGES, OcrError, OcrPage, OcrSettings};

use crate::theme::{self, Tokens};
use crate::{PdfCraftApp, widgets};

/// Which pages to read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OcrPages {
    All,
    Current,
    Range,
}

/// The dialog's choices (kept between runs, as in Acrobat).
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
            skip_text_pages: !self.text_pages,
            read_unusable_text: self.read_unusable_text,
            digits: self.digits,
        }
    }
}

/// What to tell the user when `language` has no models installed, in the interface language
/// (`None` when it has). The sentence for a named language is [`OcrError::NoLanguageModel`]'s own
/// wording with `{language}` where it names the language: that is the catalogs' key.
fn missing_models(language: &str) -> Option<String> {
    if pdfcraft_engine::ocr::available_for(language) {
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
}

/// Recognize text in multiple files: files done, total, the summary once finished, and a cancel
/// request.
#[derive(Default)]
pub struct BatchProgress {
    pub done: usize,
    pub total: usize,
    pub message: Option<String>,
    pub cancel: bool,
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

/// Recognize `files` one at a time and write each searchable copy (into `dir` on the desktop),
/// returning the summary. Cancel in `p` stops between files and inside one; a file cut short is
/// not written, so every file written is complete.
fn recognize_batch(
    files: Vec<(String, Vec<u8>)>,
    settings: &OcrSettings,
    #[cfg(not(target_arch = "wasm32"))] dir: &std::path::Path,
    p: &Mutex<BatchProgress>,
) -> String {
    // A poisoned lock means the UI side is gone: stop as if cancelled.
    let cancelled = || p.lock().map_or(true, |s| s.cancel);
    let (mut ok, mut words, mut unreadable, mut failed, mut stopped) = (0, 0, 0, Vec::new(), false);
    match pdfcraft_engine::ocr::engine_for(&settings.language) {
        Err(e) => failed.push(e),
        Ok(ocr) => {
            for (i, (name, bytes)) in files.into_iter().enumerate() {
                if cancelled() {
                    stopped = true;
                    break;
                }
                if let Ok(mut s) = p.lock() {
                    s.done = i;
                }
                let r = pdfcraft_engine::ocr::recognize_file(&name, Arc::new(bytes), None, settings.clone(), &ocr, |_, _| !cancelled());
                if cancelled() {
                    stopped = true;
                    break;
                }
                let saved = r.and_then(|r| {
                    #[cfg(not(target_arch = "wasm32"))]
                    crate::editing::write_atomically(&dir.join(&name).to_string_lossy(), &r.bytes).map_err(|e| e.to_string())?;
                    #[cfg(target_arch = "wasm32")]
                    crate::editing::download(&name, &r.bytes)?;
                    Ok((r.words(), unreadable_pages(&r.pages)))
                });
                match saved {
                    Ok((n, u)) => {
                        ok += 1;
                        words += n;
                        unreadable += u;
                    }
                    Err(e) => failed.push(format!("{name}: {}", e)),
                }
            }
        }
    }
    let mut msg = if stopped && ok == 0 {
        tl!("Cancelled").to_string()
    } else if ok == 1 {
        crate::i18n::fmt(tl!("Recognized {w} words in 1 file"), &[("w", &words.to_string())])
    } else {
        crate::i18n::fmt(tl!("Recognized {w} words in {n} files"), &[("w", &words.to_string()), ("n", &ok.to_string())])
    };
    if stopped && ok > 0 {
        msg.push_str("; ");
        msg.push_str(tl!("Cancelled"));
    }
    if unreadable > 0 {
        msg.push_str(&unreadable_note());
    }
    if !failed.is_empty() {
        msg.push_str(&crate::i18n::fmt(tl!("; failed: {list}"), &[("list", &failed.join("; "))]));
    }
    msg
}

pub struct OcrRun {
    pub doc: DocId,
    pub progress: Arc<Mutex<OcrProgress>>,
    /// The run's settings, for the offer to read pages it skipped for unusable text.
    pub settings: OcrSettings,
}

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
            egui::ComboBox::from_id_salt("ocr-language").selected_text(tl!(name)).width(220.0).show_ui(ui, |ui| {
                for (code, name) in LANGUAGES {
                    ui.selectable_value(&mut language, (*code).to_string(), tl!(*name));
                }
            });
            ui.end_row();
            ui.label(tl!("Output"));
            egui::ComboBox::from_id_salt("ocr-output").selected_text(tl!("Searchable Image (Exact)")).width(220.0).show_ui(ui, |ui| {
                let _ = ui
                    .selectable_label(true, tl!("Searchable Image (Exact)"))
                    .on_hover_text(tl!("Adds invisible text over each word; the page image is not changed"));
            });
            ui.end_row();
            ui.label(tl!("Downsample to"));
            egui::ComboBox::from_id_salt("ocr-dpi").selected_text(format!("{} dpi", d.dpi)).width(220.0).show_ui(ui, |ui| {
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
            egui::ComboBox::from_id_salt("ocr-digits").selected_text(digits_label).width(220.0).show_ui(ui, |ui| {
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
        ui.add_enabled(!d.text_pages, crate::widgets::checkbox(&mut d.read_unusable_text, tl!("Also recognize pages whose text is unusable")));
    });
    if language != app.ocr_draft.language() {
        app.ocr_draft.language = Some(language.clone());
    }
    let missing = missing_models(&language);
    if let Some(why) = &missing {
        ui.label(egui::RichText::new(why).small().color(t.text_muted));
        ui.add_space(6.0);
    }
    let available = missing.is_none();
    ui.add_space(6.0);
    let (mut go, mut cancel) = (false, false);
    ui.horizontal(|ui| {
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add_enabled_ui(available, |ui| widgets::pill_button(ui, tl!("Recognize text"), true)).inner.clicked() {
                go = true;
            }
            if widgets::pill_button(ui, tl!("Cancel"), false).clicked() {
                cancel = true;
            }
        })
    });
    (go, cancel)
}

impl PdfCraftApp {
    /// Recognize text on the pages chosen in the dialog, in the background.
    pub fn start_ocr(&mut self) {
        // Reading over unusable text is confirmed for one run only (SRS N9).
        let read_unusable_text = std::mem::take(&mut self.ocr_draft.read_unusable_text);
        let Some((vi, id)) = self.active_ids() else { return };
        if self.ocr_run.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        if let Some(why) = self.session.get(id).and_then(|d| d.read_only_reason.clone()) {
            self.notify(why);
            return;
        }
        let d = &self.ocr_draft;
        let pages: Vec<usize> = match d.pages {
            OcrPages::All => Vec::new(),
            OcrPages::Current => vec![self.views[vi].current],
            OcrPages::Range => (d.from.saturating_sub(1)..d.to).collect(),
        };
        let settings = OcrSettings { read_unusable_text, ..d.settings() };
        self.run_ocr(id, &pages, settings);
    }

    /// Recognize text on `pages` of `id` (all pages when empty) in the background.
    fn run_ocr(&mut self, id: DocId, pages: &[usize], settings: OcrSettings) {
        if self.ocr_run.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        if let Some(why) = missing_models(&settings.language) {
            self.notify(why);
            return;
        }
        // A new run replaces an offer still showing.
        self.ocr_offer = None;
        let language = settings.language.clone();
        let run_settings = settings.clone();
        let Some(job) = self.session.ocr_job(id, pages, settings) else { return };
        let progress = Arc::new(Mutex::new(OcrProgress { total: job.pages.len(), ..Default::default() }));
        let p = progress.clone();
        let work = move || {
            let result = pdfcraft_engine::ocr::engine_for(&language).map(|ocr| {
                job.run(&ocr, |done, total| {
                    let Ok(mut s) = p.lock() else { return false };
                    s.done = done;
                    s.total = total;
                    !s.cancel
                })
            });
            if let Ok(mut s) = p.lock() {
                s.result = Some(result);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else {
            std::thread::Builder::new().name("pdfcraft-ocr".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_run = Some(OcrRun { doc: id, progress, settings: run_settings });
        self.poll_ocr();
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
        self.run_ocr(offer.doc, &offer.pages, settings);
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
        egui::Area::new(egui::Id::new("ocr-unreadable-offer"))
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
    pub fn ocr_files(&mut self, files: Vec<(String, Vec<u8>)>) {
        if self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        // The settings showing now, not when the folder arrives.
        let settings = self.ocr_draft.settings();
        if let Some(why) = missing_models(&settings.language) {
            self.notify(why);
            return;
        }
        #[cfg(not(target_arch = "wasm32"))]
        match self.export_dir_override.clone() {
            Some(d) => self.ocr_files_into(files, settings, d.into()),
            None => {
                let dialog = rfd::AsyncFileDialog::new().set_title(tl!("Choose a folder for the searchable files").to_string());
                self.ask_one(crate::pickers::Ask::Folder(dialog), None, move |app, dir| app.ocr_files_into(files, settings, dir));
            }
        }
        #[cfg(target_arch = "wasm32")]
        self.ocr_files_into(files, settings);
    }

    /// [`Self::ocr_files`] once the folder for the searchable files is known.
    fn ocr_files_into(&mut self, files: Vec<(String, Vec<u8>)>, settings: OcrSettings, #[cfg(not(target_arch = "wasm32"))] dir: std::path::PathBuf) {
        // Another batch may have started while the folder picker was open.
        if self.ocr_batch.is_some() {
            self.notify_tr("Text recognition is already running");
            return;
        }
        let progress = Arc::new(Mutex::new(BatchProgress { total: files.len(), ..Default::default() }));
        let p = progress.clone();
        // The summary is written on the worker thread: draw it in the UI's language.
        let lang = crate::i18n::current();
        let work = move || {
            crate::i18n::set_current(lang);
            #[cfg(not(target_arch = "wasm32"))]
            let msg = recognize_batch(files, &settings, &dir, &p);
            #[cfg(target_arch = "wasm32")]
            let msg = recognize_batch(files, &settings, &p);
            if let Ok(mut s) = p.lock() {
                s.done = s.total;
                s.message = Some(msg);
            }
        };
        #[cfg(not(target_arch = "wasm32"))]
        if self.run_inline {
            work();
        } else {
            std::thread::Builder::new().name("pdfcraft-ocr-files".into()).spawn(work).ok();
        }
        #[cfg(target_arch = "wasm32")]
        work();
        self.ocr_batch = Some(progress);
        self.poll_ocr();
    }

    /// Stop the running recognition: a document is left as it was (SRS §8.2), and a batch keeps
    /// only the files already written.
    pub fn cancel_ocr(&mut self) {
        if let Some(r) = &self.ocr_run
            && let Ok(mut s) = r.progress.lock()
        {
            s.cancel = true;
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

    /// Show progress; apply the result once the worker is done.
    pub(crate) fn poll_ocr(&mut self) {
        if let Some(b) = self.ocr_batch.clone() {
            let msg = b.lock().ok().map(|mut s| s.message.take().ok_or((s.done, s.total, s.cancel)));
            match msg {
                Some(Ok(m)) => {
                    self.ocr_batch = None;
                    self.hide_ocr_progress();
                    self.notify(m);
                }
                Some(Err((done, total, cancelling))) => {
                    let label = crate::i18n::fmt(
                        tl!("Recognizing text… file {d} of {t}"),
                        &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
                    );
                    self.show_ocr_progress(label, done, total, cancelling);
                }
                None => {
                    self.ocr_batch = None;
                    self.hide_ocr_progress();
                }
            }
        }
        let Some(run) = self.ocr_run.as_ref() else { return };
        let (doc, progress, settings) = (run.doc, run.progress.clone(), run.settings.clone());
        let Ok(mut s) = progress.lock() else {
            // The worker panicked while holding the lock: it will never report back.
            self.ocr_run = None;
            self.hide_ocr_progress();
            return;
        };
        let cancelled = s.cancel;
        let Some(result) = s.result.take() else {
            let (done, total) = (s.done, s.total);
            drop(s);
            let label = crate::i18n::fmt(
                tl!("Recognizing text… page {d} of {t}"),
                &[("d", &(done + 1).min(total.max(1)).to_string()), ("t", &total.max(1).to_string())],
            );
            self.show_ocr_progress(label, done, total, cancelled);
            return;
        };
        drop(s);
        self.ocr_run = None;
        self.hide_ocr_progress();
        // SRS §8.2: Cancel leaves the document as it was, the pages read so far included.
        if cancelled {
            self.notify_tr("Cancelled");
            return;
        }
        let found = match result {
            Ok(f) => f,
            Err(e) => {
                self.notify_error(e);
                return;
            }
        };
        let read = found.iter().filter(|p| p.skipped.is_none()).count();
        let unreadable = unreadable_pages(&found);
        let skipped = found.len().saturating_sub(read).saturating_sub(unreadable);
        match self.session.apply_ocr(doc, &found) {
            Ok(words) => {
                if let Some(info) = self.session.get(doc).map(|d| d.info.clone())
                    && let Some(view) = self.views.iter_mut().find(|v| v.id == doc)
                {
                    view.document_changed(&info);
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
                self.notify(msg);
                // SRS FR-17: suggest reading the pages with unreadable text; only the user's
                // confirmation starts it.
                self.ocr_offer = UnreadableOffer::from_run(doc, &found, &settings);
            }
            Err(e) => self.notify_error(e),
        }
    }
}

#[cfg(test)]
mod tests {
    use egui_kittest::Harness;
    use egui_kittest::kittest::Queryable;
    use pdfcraft_engine::ocr::{PlacedWord, UNUSABLE_TEXT};

    use super::*;

    fn page(page: usize, skipped: Option<&str>, unusable_text: bool) -> OcrPage {
        let words = if skipped.is_some() {
            Vec::new()
        } else {
            vec![PlacedWord { text: "word".into(), origin: [72.0, 700.0], across: [40.0, 0.0], up: [0.0, 12.0] }]
        };
        OcrPage { page, words, already_on_page: 0, skipped: skipped.map(Into::into), unusable_text }
    }

    /// A blank three-page document with `progress` as its running recognition.
    fn running(progress: Arc<Mutex<OcrProgress>>) -> Harness<'static, PdfCraftApp> {
        Harness::builder().with_size(egui::vec2(1200.0, 800.0)).build_eframe(move |_cc| {
            let mut app = PdfCraftApp::new();
            app.set_option("language", "en").unwrap();
            let blank = pdfcraft_engine::Session::new().create_blank(612.0, 792.0, 3).unwrap().to_vec();
            app.open_bytes("scan.pdf", None, blank).unwrap();
            let (_, doc) = app.active_ids().unwrap();
            app.ocr_run = Some(OcrRun { doc, progress, settings: OcrSettings::default() });
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

    /// A batch cancelled before its next file writes nothing more.
    #[test]
    fn a_cancelled_batch_writes_no_more_files() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-batch-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let blank = pdfcraft_engine::Session::new().create_blank(612.0, 792.0, 1).unwrap().to_vec();
        let p = Mutex::new(BatchProgress { total: 2, cancel: true, ..Default::default() });
        let settings = OcrSettings::default();
        let msg = recognize_batch(vec![("one.pdf".into(), blank.clone()), ("two.pdf".into(), blank)], &settings, &dir, &p);
        if pdfcraft_engine::ocr::available() {
            assert_eq!(msg, "Cancelled");
        } else {
            assert!(msg.contains("failed"), "{msg}");
        }
        assert_eq!(std::fs::read_dir(&dir).unwrap().count(), 0, "no file was written");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
