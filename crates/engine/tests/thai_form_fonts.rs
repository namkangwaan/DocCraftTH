//! Integration tests for bundled THSarabun style resolution and FontsGOV catalog registration.

use std::path::PathBuf;

use pdfcraft_engine::thai_catalog::{Source, entries_in_files};
use pdfcraft_fonts::source_shaping::shape_validated_thai;
use pdfcraft_fonts::thai_faces;
use pdfcraft_forms::FieldFont;

#[test]
fn thsarabun_styles_resolve_and_fontsgov_is_not_substituted() {
    if pdfcraft_fonts::CRAFT_FONTS.is_empty() {
        eprintln!("skipping thsarabun_styles_resolve_and_fontsgov_is_not_substituted: built without craft-fonts");
        return;
    }

    let reg = thai_faces::by_family("THSarabun").expect("THSarabun must be bundled");
    let bold = thai_faces::resolve_style(reg, true, false);
    let italic = thai_faces::resolve_style(reg, false, true);
    let bold_italic = thai_faces::resolve_style(reg, true, true);
    assert_eq!(thai_faces::resolve_style(reg, false, false), reg);
    assert_ne!(bold, reg);
    assert_ne!(italic, reg);
    assert_ne!(bold_italic, reg);
    assert_ne!(bold, italic);
    assert_ne!(bold, bold_italic);
    assert_ne!(italic, bold_italic);
    assert_ne!(thai_faces::bytes(bold), thai_faces::bytes(reg));

    let Some(craft_dir) = std::env::var_os("CRAFT_FONTS_DIR").map(PathBuf::from) else {
        return;
    };
    let gov_dir = craft_dir.join("fonts").join("FontsGOV");
    if !gov_dir.is_dir() {
        return;
    }

    let krub_files = ["TH Krub.ttf", "TH Krub Bold.ttf", "TH Krub Italic.ttf", "TH Krub Bold Italic.ttf"]
        .into_iter()
        .map(|name| gov_dir.join(name))
        .collect::<Vec<_>>();
    let entries = entries_in_files(krub_files);
    let krub = entries
        .into_iter()
        .find(|e| e.source == Source::Installed && e.family == "TH Krub")
        .expect("TH Krub should be listed as installed");
    assert!(!krub.substituted, "FontsGOV TH Krub must not be substituted");
    let FieldFont::Thai(krub_id) = krub.font else {
        panic!("expected FieldFont::Thai for TH Krub");
    };
    let krub_bold = thai_faces::resolve_style(krub_id, true, false);
    assert_ne!(krub_bold, krub_id, "TH Krub Bold should resolve to its own FaceId");
    let shaped = shape_validated_thai(thai_faces::bytes(krub_id), "กิ่ น้ำ").expect("GPOS-false FontsGOV TH Krub should shape cleanly");
    assert!(shaped.advance > 0.0);
}
