//! Thai (`th`) catalog tests, kept apart from `mod.rs` so upstream merges don't conflict.

use super::*;

#[test]
fn thai_is_registered_and_persists() {
    let th = Lang::from_code("th").expect("th registered");
    assert_eq!(th.name(), "ไทย");
    assert!(!th.rtl());
    assert_eq!(normalize_pref("TH"), Some("th"));
    for tag in ["th", "th-TH", "th_TH.UTF-8", "TH-th"] {
        assert_eq!(lang_from_tag(tag), Some(th), "{tag}");
    }
    assert_eq!(tr(th, "File"), "ไฟล์");
    assert_eq!(tr(th, "Edit"), "แก้ไข");
    assert_eq!(tr(th, "Save"), "บันทึก");
    assert_eq!(tr(th, "Save as…"), "บันทึกเป็น…");
    assert_eq!(tr(th, "Interface language"), "ภาษาของหน้าจอ");
    for n in [0, 1, 2, 5] {
        assert_eq!(trn(th, n, "{n} page", "{n} pages"), format!("{n} หน้า"));
    }
    let mut app = crate::PdfCraftApp::default();
    app.set_option("language", "th").unwrap();
    assert_eq!(app.language, "th");
    let mut restored = crate::PdfCraftApp::default();
    restored.restore(&app.persist());
    assert_eq!(restored.language, "th");
}

#[test]
fn local_pdf_date_follows_language() {
    let th = Lang::from_code("th").expect("th registered");
    assert_eq!(local_pdf_date_in(th, "D:20260930104512-04'00'"), "2569-09-30 10:45");
    assert_eq!(local_pdf_date_in(th, "2026-09-30 10:45"), "2569-09-30 10:45", "already pretty");
    assert_eq!(local_pdf_date_in(Lang::EN, "D:20260930104512-04'00'"), "2026-09-30 10:45");
    assert_eq!(local_pdf_date_in(th, "yesterday"), "yesterday");
    assert_eq!(local_pdf_date_in(th, "๒๕๖๙"), "๒๕๖๙", "non-ASCII is left alone");
    assert_eq!(local_pdf_date_in(th, ""), "");
}
