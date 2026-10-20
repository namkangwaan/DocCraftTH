//! Scan & OCR ▸ Recognize text in the real shell (egui_kittest): the dialog, the run and its
//! result (skipped without the OCR models: `cargo xtask models`).

use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use pdfcraft_engine::{AddedText, Edit, Session, export};
use pdfcraft_ui_egui::PdfCraftApp;

/// A one-page PDF that is only a picture of a sentence.
fn scan() -> Vec<u8> {
    let mut s = Session::new();
    let text = s.create_from_text("t", "Recognize these scanned words").unwrap();
    let id = s.open("text.pdf", None, text, None).unwrap();
    let png = export::Exporter::new(s.get(id).unwrap()).png(0, 150.0).unwrap();
    s.create_from_images(&[("scan.png".into(), png)]).unwrap().to_vec()
}

#[test]
fn recognize_text_dialog_adds_searchable_text() {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.recognize"));
    h.run_steps(2);
    h.get_by_label("Document language");
    h.get_by_label("Output");
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    h.get_by_label("Recognize text").click();
    h.run_steps(3);
    assert!(h.state().ocr_run.is_none(), "finished");
    let toast = h.state().toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && toast.contains("on 1 page"), "{toast}");
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    assert_eq!(app.session.get(id).unwrap().can_undo(), Some("Recognize text"));
}

/// Opens Recognize Text on a scan with the interface set to `ui`.
fn recognize_dialog(ui: &'static str) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", ui).unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.recognize"));
    h.run_steps(2);
    h
}

/// FR-03: the dialog starts on the interface language when text recognition reads it, and on
/// English otherwise. A choice made in the dialog is kept.
#[test]
fn recognize_text_dialog_defaults_to_thai_in_a_thai_interface() {
    let mut h = recognize_dialog("th");
    assert_eq!(h.state().ocr_draft.settings().language, "th");
    h.get_by_value("ไทย");
    h.state_mut().ocr_draft.language = Some("en".into());
    h.run_steps(2);
    assert_eq!(h.state().ocr_draft.settings().language, "en", "the user's choice wins over the interface language");
    let h = recognize_dialog("en");
    assert_eq!(h.state().ocr_draft.settings().language, "en");
    let h = recognize_dialog("de");
    assert_eq!(h.state().ocr_draft.settings().language, "en", "no German model: English");
}

#[test]
fn recognize_text_dialog_offers_digits_choice() {
    use pdfcraft_engine::ocr::Digits;
    let mut h = recognize_dialog("en");
    h.get_by_label("Digits");
    assert_eq!(h.state().ocr_draft.settings().digits, Digits::AsRead);
    h.state_mut().ocr_draft.digits = Digits::Thai;
    h.run_steps(2);
    assert_eq!(h.state().ocr_draft.settings().digits, Digits::Thai);
}

/// FR-05: a language without models is reported in the interface language, naming the language,
/// and nothing starts.
#[test]
fn a_missing_language_model_is_reported_in_the_interface_language() {
    let mut h = recognize_dialog("th");
    h.state_mut().ocr_draft.language = Some("xx".into());
    h.state_mut().start_ocr();
    // Thai interface text carries U+200B at word boundaries for wrapping (i18n/th_breaks.rs).
    let toast = h.state().toast.clone().unwrap().0.replace('\u{200B}', "");
    assert!(toast.contains("xx") && toast.contains("โมเดลการรู้จำข้อความ") && toast.contains("PDFCRAFT_MODELS"), "{toast}");
    assert!(h.state().ocr_run.is_none(), "nothing started");
}

#[test]
fn recognize_text_in_multiple_files_writes_searchable_copies() {
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-ui-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut app = PdfCraftApp::new();
    app.set_option("language", "en").unwrap();
    app.run_inline = true;
    app.export_dir_override = Some(dir.to_string_lossy().into_owned());
    app.use_files(pdfcraft_ui_egui::FilePurpose::Ocr, vec![("one.pdf".into(), scan()), ("two.pdf".into(), scan())]);
    assert!(app.ocr_batch.is_none(), "finished");
    let toast = app.toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && toast.ends_with("in 2 files"), "{toast}");
    for name in ["one.pdf", "two.pdf"] {
        let bytes = std::fs::read(dir.join(name)).unwrap();
        assert!(bytes.len() > scan().len(), "an incremental update was appended");
    }
}

/// #1028: "Also recognize pages that already have text" reads a picture on a page that has its own
/// text, without adding that text a second time.
#[test]
fn recognize_text_can_read_pictures_on_pages_with_text() {
    let hybrid = {
        let mut s = Session::new();
        let id = s.open("scan.pdf", None, std::sync::Arc::new(scan()), None).unwrap();
        let (w, h) = {
            let p = &s.get(id).unwrap().info.pages[0];
            (p.width as f64, p.height as f64)
        };
        let note =
            AddedText { rect: [w * 0.1, h * 0.75, w * 0.9, h * 0.85], text: "Invoice reference 4711".into(), size: 18.0, ..Default::default() };
        s.apply(id, Edit::AddText { page: 0, text: note }).unwrap();
        s.save_bytes(id).unwrap().to_vec()
    };
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("hybrid.pdf", None, hybrid).unwrap();
        app.run_inline = true;
        app
    });
    h.run_steps(4);
    assert!(h.state_mut().execute("ocr.recognize"));
    h.run_steps(2);
    assert!(h.state().ocr_draft.settings().skip_text_pages, "pages with text are skipped by default");
    h.get_by_label("Also recognize pages that already have text").click();
    h.run_steps(2);
    assert!(!h.state().ocr_draft.settings().skip_text_pages);
    if !pdfcraft_engine::ocr::available() {
        eprintln!("skipped: OCR models not installed");
        return;
    }
    h.get_by_label("Recognize text").click();
    h.run_steps(3);
    assert!(h.state().ocr_run.is_none(), "finished");
    let toast = h.state().toast.clone().unwrap().0;
    assert!(toast.starts_with("Recognized ") && toast.ends_with("on 1 page"), "{toast}");
    let app = h.state();
    let id = app.active_ids().unwrap().1;
    let doc = app.session.get(id).unwrap();
    assert_eq!(doc.can_undo(), Some("Recognize text"));
    let mut r = pdfcraft_render::PageRenderer::new(doc.bytes.clone(), Default::default());
    let text = r.render(pdfcraft_render::RenderRequest { page: 0, kind: pdfcraft_render::RequestKind::Text, scale: 1.0, ..Default::default() });
    let text = text.text.unwrap().plain_text().to_lowercase();
    assert_eq!((text.matches("invoice").count(), text.matches("scanned").count()), (1, 1), "{text}");
}

/// Edit text & images on a scanned page: an OCR word (invisible text over the scan) opens for
/// editing on a click, stays open when the pointer leaves it, and its editor hides the scanned
/// word instead of showing it through.
#[test]
fn an_ocr_word_opens_for_editing_and_stays_open() {
    use pdfcraft_engine::ocr::PlacedWord;
    let mut h = Harness::builder().with_size(egui::vec2(1400.0, 900.0)).build_eframe(|_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", "en").unwrap();
        app.open_bytes("scan.pdf", None, scan()).unwrap();
        let id = app.views[0].id;
        let ph = app.session.get(id).unwrap().info.pages[0].height as f64;
        let words = vec![PlacedWord { text: "Recognize".into(), origin: [72.0, ph - 100.0], across: [80.0, 0.0], up: [0.0, 14.0] }];
        app.session.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
        app.set_option("zoom", "100").unwrap();
        assert!(app.execute("edit.edit_text"));
        app
    });
    for _ in 0..60 {
        h.run_steps(2);
        if !h.state().render_pending() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(5));
    }
    let (r, k) = {
        let app = h.state();
        let d = app.session.get(app.views[0].id).unwrap();
        assert!(d.text_blocks(0).first().is_some_and(|b| b.invisible));
        let r = app.views[0].page_screen_rect(0).unwrap();
        (r, r.width() / d.info.pages[0].width)
    };
    // The word, 100 pt from the top of the page.
    let p = r.min + egui::vec2(100.0, 93.0) * k;
    h.hover_at(p);
    h.run_steps(2);
    for pressed in [true, false] {
        h.event(egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: Default::default() });
    }
    h.run_steps(4);
    let opened = |h: &Harness<'_, PdfCraftApp>| h.state().views[0].line_editor.as_ref().map(|e| e.text.clone());
    assert_eq!(opened(&h).as_deref(), Some("Recognize"));
    assert!(h.state().views[0].image_selection.is_none(), "the scan under the word isn't selected");
    h.hover_at(r.min + egui::vec2(300.0, 400.0) * k);
    h.run_steps(4);
    assert_eq!(opened(&h).as_deref(), Some("Recognize"), "moving the pointer away keeps the word open");
}

/// One page whose text maps to private-use code points, as a legacy Thai font's ToUnicode does
/// (SRS FR-17): rows of such words over the whole page, saved and reopened.
fn legacy_text_page() -> Vec<u8> {
    let mut s = Session::new();
    let blank = s.create_blank(612.0, 792.0, 1).unwrap();
    let id = s.open("legacy.pdf", None, blank, None).unwrap();
    let words = (0..10)
        .map(|i| pdfcraft_engine::ocr::PlacedWord {
            text: "\u{F700}\u{F701}\u{F702}\u{F703}".into(),
            origin: [0.0, 79.2 * f64::from(i)],
            across: [612.0, 0.0],
            up: [0.0, 79.2],
        })
        .collect();
    s.apply(id, Edit::AddOcrText { page: 0, words }).unwrap();
    s.save_bytes(id).unwrap().to_vec()
}

/// Opens `pdf` at `size` with the interface in `ui`, and runs frames until the current page's
/// text has been read.
fn browsing(pdf: Vec<u8>, ui: &'static str, size: egui::Vec2) -> Harness<'static, PdfCraftApp> {
    let mut h = Harness::builder().with_size(size).build_eframe(move |_cc| {
        let mut app = PdfCraftApp::new();
        app.set_option("language", ui).unwrap();
        app.open_bytes("page.pdf", None, pdf).unwrap();
        app.run_inline = true;
        app
    });
    for _ in 0..400 {
        h.run_steps(1);
        if h.state().current_page_text_unusable().is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    assert!(h.state().current_page_text_unusable().is_some(), "the page's text was read");
    h.run_steps(2);
    h
}

const NOTICE: &str = "This page has unusable text";
const OPEN: &str = "Recognize text on this page…";

/// SRS §5.5: browsing a page whose text is unusable shows the notice, and nothing is read.
#[test]
fn a_page_with_unusable_text_shows_the_notice() {
    let h = browsing(legacy_text_page(), "en", egui::vec2(1400.0, 900.0));
    assert_eq!(h.state().current_page_text_unusable(), Some(true));
    h.get_by_label(NOTICE);
    h.get_by_label(OPEN);
    let app = h.state();
    assert!(app.ocr_run.is_none() && app.dialog.is_none(), "the notice starts nothing by itself");
    assert_eq!(app.session.get(app.views[0].id).unwrap().can_undo(), None);
}

/// Pages with good text, or with no text at all, show no notice.
#[test]
fn pages_with_good_or_no_text_show_no_notice() {
    let s = Session::new();
    for (what, pdf) in [
        ("good text", s.create_from_text("t", "Ordinary searchable words").unwrap().to_vec()),
        ("no text", s.create_blank(612.0, 792.0, 1).unwrap().to_vec()),
    ] {
        let h = browsing(pdf, "en", egui::vec2(1400.0, 900.0));
        assert_eq!(h.state().current_page_text_unusable(), Some(false), "{what}");
        assert!(h.query_by_label(NOTICE).is_none(), "{what}");
    }
}

/// The button opens Recognize Text for this page with reading over its unusable text ticked,
/// and still reads nothing until the user confirms there; the tick lasts one run.
#[test]
fn the_notice_opens_recognize_text_for_the_page_without_reading_it() {
    let mut h = browsing(legacy_text_page(), "en", egui::vec2(1400.0, 900.0));
    h.get_by_label(OPEN).click();
    h.run_steps(3);
    {
        let app = h.state();
        assert!(matches!(app.dialog, Some(pdfcraft_ui_egui::Dialog::RecognizeText)));
        let d = &app.ocr_draft;
        assert_eq!((format!("{:?}", d.pages).as_str(), d.from, d.to), ("Range", 1, 1), "set to this page");
        let settings = d.settings();
        assert!(settings.read_unusable_text && settings.skip_text_pages, "only unusable text is read over: {settings:?}");
        assert!(app.ocr_run.is_none(), "nothing is read before the user confirms");
        assert_eq!(app.session.get(app.views[0].id).unwrap().can_undo(), None);
    }
    h.get_by_label("Also recognize pages whose text is unusable");
    assert!(h.query_by_label(NOTICE).is_none(), "the dialog covers the page, so the notice steps aside");

    // Confirming uses the tick for this run only.
    if pdfcraft_engine::ocr::available() {
        h.get_by_label("Recognize text").click();
        h.run_steps(3);
        let toast = h.state().toast.clone().unwrap().0;
        assert!(toast.starts_with("Recognized ") && toast.contains("on 1 page"), "the page was read, not skipped: {toast}");
    } else {
        h.state_mut().start_ocr();
    }
    assert!(!h.state().ocr_draft.read_unusable_text, "the next run skips unusable pages again unless asked");
}

/// "Not now" hides the notice for the document.
#[test]
fn not_now_dismisses_the_notice() {
    let mut h = browsing(legacy_text_page(), "en", egui::vec2(1400.0, 900.0));
    h.get_by_label("Not now").click();
    h.run_steps(3);
    assert!(h.query_by_label(NOTICE).is_none());
    assert!(h.state().dialog.is_none() && h.state().ocr_run.is_none());
}

/// The notice in Thai (SRS §5.5 wording).
#[test]
fn the_notice_reads_in_thai() {
    let h = browsing(legacy_text_page(), "th", egui::vec2(1400.0, 900.0));
    // Thai is drawn with zero-width spaces between words, for line breaking; a label and its
    // text node both carry it.
    let shows =
        |text: &str| h.query_all_by(|n| [n.label(), n.value()].into_iter().flatten().any(|l| l.replace('\u{200b}', "") == text)).next().is_some();
    assert!(shows("หน้านี้มีข้อความที่ใช้ไม่ได้"));
    assert!(shows("รู้จำข้อความในหน้านี้…"));
}
