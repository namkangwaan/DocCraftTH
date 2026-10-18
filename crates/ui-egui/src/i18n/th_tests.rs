//! Thai (`th`) catalog tests, kept apart from `mod.rs` so upstream merges don't conflict.

use super::*;

/// A translation as it is typed: without the display-only Thai soft breaks (U+200B).
fn plain(s: &str) -> String {
    super::th_breaks::without_soft_breaks(s).into_owned()
}

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
    assert_eq!(plain(tr(th, "Edit")), "แก้ไข");
    assert_eq!(plain(tr(th, "Save")), "บันทึก");
    assert_eq!(plain(tr(th, "Save as…")), "บันทึกเป็น…");
    assert_eq!(plain(tr(th, "Interface language")), "ภาษาของหน้าจอ");
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

#[test]
fn thai_covers_commands_and_tools() {
    let th = Lang::from_code("th").expect("th registered");
    let mut missing = std::collections::BTreeSet::new();
    for command in pdfcraft_engine::commands::COMMANDS {
        if !has(th, command.label) {
            missing.insert(command.label);
        }
        if let Some(menu) = command.menu
            && !has(th, menu)
        {
            missing.insert(menu);
        }
    }
    for group in pdfcraft_engine::catalog::TOOL_GROUPS {
        if !has(th, group.label) {
            missing.insert(group.label);
        }
        for section in group.sections {
            if !has(th, section.title) {
                missing.insert(section.title);
            }
            for item in section.items {
                if !has(th, item.label) {
                    missing.insert(item.label);
                }
            }
        }
    }
    assert!(missing.is_empty(), "Thai misses commands/tools: {missing:#?}");
}

#[test]
fn thai_covers_ui_literals() {
    let th = Lang::from_code("th").expect("th registered");
    let literals = super::tests::scan_ui_literals(true);
    let missing: Vec<_> = literals.iter().filter(|label| !has(th, label)).collect();
    assert!(missing.is_empty(), "Untranslated Thai UI labels: {missing:#?}");
}

/// FR-05: the catalog key is the engine's own wording with a placeholder where it names the
/// language (`ocr_ui::missing_models` builds it the same way).
#[test]
fn thai_names_the_missing_language_model() {
    let th = Lang::from_code("th").expect("th registered");
    let key = pdfcraft_engine::ocr::OcrError::NoLanguageModel("{language}".into()).to_string();
    assert!(has(th, &key));
    let thai = fmt(tr(th, &key), &[("language", "ไทย")]);
    assert!(thai.contains("ไทย") && thai.contains("PDFCRAFT_MODELS") && !thai.is_ascii(), "{thai}");
    assert!(!thai.contains("not installed"), "{thai}");
}

#[test]
fn thai_covers_bundled_catalog_keys() {
    let th = Lang::from_code("th").expect("th registered");
    let mut missing = std::collections::BTreeSet::new();
    for reference in LANGUAGES.iter().filter(|language| language.code != "th") {
        let (entries, errors) = catalog::parse_entries(reference.source, reference.plural_forms());
        assert!(errors.is_empty(), "{}: invalid reference catalog: {errors:?}", reference.code);
        for entry in entries {
            let translated = match entry.context.as_str() {
                "" => th.catalog().plain(&entry.source),
                "@id" => th.catalog().id(&entry.source),
                "@plural" => entry.source.split_once('|').and_then(|(one, other)| th.catalog().plural(one, other, 0)),
                context => th.catalog().contextual(context, &entry.source),
            };
            if translated.is_none() {
                missing.insert((entry.context, entry.source));
            }
        }
    }
    assert!(missing.is_empty(), "Thai misses bundled catalog keys: {missing:#?}");
}

#[test]
fn thai_months_and_contexts() {
    let th = Lang::from_code("th").expect("th registered");
    let months = ["มกราคม", "กุมภาพันธ์", "มีนาคม", "เมษายน", "พฤษภาคม", "มิถุนายน", "กรกฎาคม", "สิงหาคม", "กันยายน", "ตุลาคม", "พฤศจิกายน", "ธันวาคม"];
    let english = ["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
    for (en, thai) in english.iter().zip(months) {
        assert_eq!(plain(tr(th, en)), thai);
    }
    assert_ne!(tr_ctx(th, "signature pad", "Type"), "Type");
    assert_ne!(tr_ctx(th, "comment menu", "Edit"), "Edit");
}

#[test]
fn calendar_day_tooltip_uses_the_buddhist_era_in_thai() {
    let th = Lang::from_code("th").expect("th registered");
    assert_eq!(plain(&crate::forms_ui::day_tooltip_in(th, 2026, 10, 17)), "17 ตุลาคม 2569");
    assert_eq!(crate::forms_ui::day_tooltip_in(Lang::EN, 2026, 10, 17), "October 17, 2026");
}

#[test]
fn thai_keeps_self_signed_apart_from_signing_yourself() {
    let th = Lang::from_code("th").expect("th registered");
    let yourself = tr(th, "Sign yourself");
    assert_ne!(tr(th, "Self-signed"), yourself, "a certificate's trust is not the act of signing");
    assert_ne!(tr_ctx(th, "certificate", "Self-signed"), yourself);
}

#[test]
fn thai_strings_carry_soft_breaks() {
    let th = Lang::from_code("th").expect("th registered");
    let said = tr(th, "Only allow this if you trust the source document.");
    assert!(said.contains('\u{200B}'), "{said:?}");
    assert_eq!(super::th_breaks::without_soft_breaks(said), "อนุญาตเฉพาะเมื่อคุณเชื่อถือเอกสารต้นทาง");
    assert_eq!(tr(th, "File"), "ไฟล์", "one word, no break");
}

#[test]
fn soft_breaks_are_display_only() {
    for text in ["ทดสอบ เอกสาร น้ำใจ", "abc", ""] {
        let marked = super::th_breaks::soft_breaks(text);
        assert_eq!(super::th_breaks::without_soft_breaks(&marked), text);
    }
}
