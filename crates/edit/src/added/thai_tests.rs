//! Existing Thai text drawn from the bundled Thai outlines (`thai_glyph_resources`): one Type 3
//! code per distinct glyph and cluster text, in fonts of `GLYPHS_PER_FONT` codes each. The
//! outline checks need the craft-fonts Thai face; the budget check runs without it.

use pdfcraft_cos::{Dict, Document};
use pdfcraft_fonts::source_shaping::{ShapedGlyph, ShapedRun};

use super::*;

/// `count` copies of `glyph`, each with cluster text of its own (so each needs a code of its own).
fn distinct(glyph: &ShapedGlyph, count: usize) -> ShapedRun {
    let glyphs = (0..count).map(|i| ShapedGlyph { position: [i as f64, 0.0], text: format!("{i}"), ..glyph.clone() }).collect();
    ShapedRun { glyphs, advance: count as f64 }
}

#[test]
fn thai_glyph_resources_refuse_a_row_past_the_glyph_budget() {
    let glyph = ShapedGlyph { id: Default::default(), position: [0.0, 0.0], text: String::new() };
    let over = ShapedRun { glyphs: vec![glyph; 65_535], advance: 0.0 };
    let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
    assert!(thai_glyph_resources(&mut doc, &mut fonts, &over).is_err());
    assert!(fonts.is_empty(), "a refused row adds no font");
}

/// Codes fill a font up to `GLYPHS_PER_FONT`, the next distinct cluster starts another font at
/// code 1, and the same glyph with the same text again reuses its code. One glyph drawn for two
/// cluster texts takes two codes, since each code's ToUnicode gives its own text.
#[test]
fn thai_glyph_resources_shard_distinct_clusters_at_the_font_boundary() {
    let Some(face) = pdfcraft_fonts::document_thai_font() else {
        eprintln!(
            "skipping thai_glyph_resources_shard_distinct_clusters_at_the_font_boundary: built without craft-fonts (set CRAFT_FONTS_DIR to run it)"
        );
        return;
    };
    let ko = pdfcraft_fonts::source_shaping::shape(face.bytes, "ก").unwrap().glyphs.remove(0);
    for (clusters, fonts_used) in [(GLYPHS_PER_FONT - 1, 1), (GLYPHS_PER_FONT, 1), (GLYPHS_PER_FONT + 1, 2)] {
        let mut shaped = distinct(&ko, clusters);
        shaped.glyphs.push(shaped.glyphs[0].clone());
        let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
        let placed = thai_glyph_resources(&mut doc, &mut fonts, &shaped).unwrap();
        assert_eq!((fonts.len(), placed.len()), (fonts_used, clusters + 1), "{clusters} clusters");
        assert_eq!(placed.last(), placed.first(), "a repeated glyph and text reuses its code");
        let codes: std::collections::HashSet<_> = placed.iter().collect();
        assert_eq!(codes.len(), clusters, "every distinct cluster has a code of its own");
        assert_eq!(placed[GLYPHS_PER_FONT - 2].1, [(GLYPHS_PER_FONT - 1) as u8]);
        if clusters > GLYPHS_PER_FONT {
            assert_eq!(placed[GLYPHS_PER_FONT - 1].1, [GLYPHS_PER_FONT as u8]);
            assert_eq!(placed[GLYPHS_PER_FONT].1, [1], "the next font starts at code 1");
            assert_ne!(placed[GLYPHS_PER_FONT].0, placed[0].0);
        }
    }
    // At the budget: one cluster repeated 65 534 times needs one code.
    let mut row = distinct(&ko, 1);
    row.glyphs = vec![row.glyphs[0].clone(); 65_534];
    let (mut doc, mut fonts) = (Document::new_empty(), Dict::new());
    let placed = thai_glyph_resources(&mut doc, &mut fonts, &row).unwrap();
    assert_eq!((fonts.len(), placed.len()), (1, 65_534));
}

fn blank_doc() -> Document {
    use std::sync::Arc;
    let objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 /MediaBox [0 0 612 792] >>",
        "<< /Type /Page /Parent 2 0 R >>",
    ];
    let mut out = b"%PDF-1.7\n".to_vec();
    let mut offs = Vec::new();
    for (i, o) in objs.iter().enumerate() {
        offs.push(out.len());
        out.extend_from_slice(format!("{} 0 obj\n{o}\nendobj\n", i + 1).as_bytes());
    }
    let x = out.len();
    out.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", objs.len() + 1).as_bytes());
    for o in offs {
        out.extend_from_slice(format!("{o:010} 00000 n \n").as_bytes());
    }
    out.extend_from_slice(format!("trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n", objs.len() + 1).as_bytes());
    Document::open(Arc::new(out)).unwrap()
}

fn reopen(doc: &Document) -> Document {
    use std::sync::Arc;
    let bytes = pdfcraft_cos::write_incremental(doc, &pdfcraft_cos::SaveOptions::default()).unwrap();
    Document::open(Arc::new(bytes)).unwrap()
}

fn page_font_dicts(doc: &Document) -> Vec<(String, Dict)> {
    let page = &pdfcraft_model::pages(doc)[0];
    let Some(res) = page.dict.get(b"Resources").and_then(|r| doc.resolve(r).as_dict().cloned()) else {
        return Vec::new();
    };
    let Some(fonts) = res.get(b"Font").and_then(|f| doc.resolve(f).as_dict().cloned()) else {
        return Vec::new();
    };
    fonts
        .iter()
        .filter_map(|(k, v)| doc.resolve(v).as_dict().cloned().map(|d| (String::from_utf8_lossy(k).into_owned(), d)))
        .collect()
}

#[test]
fn added_text_with_thsarabun_embeds_type0_for_thai_and_latin_and_roundtrips() {
    let Some(thsarabun_id) = pdfcraft_fonts::thai_faces::by_family("THSarabun") else {
        eprintln!(
            "skipping added_text_with_thsarabun_embeds_type0_for_thai_and_latin_and_roundtrips: built without craft-fonts (set CRAFT_FONTS_DIR to run it)"
        );
        return;
    };
    let mut doc = blank_doc();
    let item = AddedText {
        rect: [72.0, 600.0, 400.0, 720.0],
        text: "หนังสือราชการ 2569 ABC".into(),
        family: Family::Thai(thsarabun_id),
        bold: true,
        italic: false,
        size: 16.0,
        ..AddedText::default()
    };
    add_content(&mut doc, 0, &Content::Text(item.clone())).unwrap();

    let reloaded = reopen(&doc);
    let added = list_added(&reloaded);
    assert_eq!(added.len(), 1);
    let Content::Text(loaded) = &added[0].content else {
        panic!("expected text item");
    };
    assert_eq!(loaded.family, Family::Thai(thsarabun_id));
    assert!(loaded.bold);
    assert!(!loaded.italic);
    assert_eq!(loaded.text, item.text);

    let fonts = page_font_dicts(&reloaded);
    assert!(
        fonts.iter().any(|(_, d)| d.name(b"Subtype") == Some(b"Type0")),
        "expected Type0 TrueType font in page /Resources /Font: {fonts:?}"
    );
    assert!(
        !fonts.iter().any(|(_, d)| d.name(b"BaseFont") == Some(b"Helvetica") || d.name(b"Subtype") == Some(b"Type3")),
        "THSarabun must embed both Thai and Latin/digits in Type0, not Helvetica or Type3: {fonts:?}"
    );
    assert!(Family::Thai(thsarabun_id).width("2569 ABC", 16.0, true, false) > 0.0);
    assert!(text_width(&item) > 0.0);
}

#[test]
fn added_text_std14_helvetica_remains_unchanged() {
    if pdfcraft_fonts::document_thai_font().is_none() {
        eprintln!("skipping added_text_std14_helvetica_remains_unchanged: built without craft-fonts");
        return;
    }
    let mut doc = blank_doc();
    let item = AddedText {
        rect: [72.0, 600.0, 400.0, 720.0],
        text: "สวัสดี 2569 ABC".into(),
        family: Family::Helvetica,
        size: 14.0,
        ..AddedText::default()
    };
    add_content(&mut doc, 0, &Content::Text(item)).unwrap();

    let added = list_added(&doc);
    assert_eq!(added.len(), 1);
    let Content::Text(loaded) = &added[0].content else {
        panic!("expected text item");
    };
    assert_eq!(loaded.family, Family::Helvetica);

    let fonts = page_font_dicts(&doc);
    assert!(fonts.iter().any(|(_, d)| d.name(b"BaseFont") == Some(b"Helvetica")));
    assert!(fonts.iter().any(|(_, d)| d.name(b"Subtype") == Some(b"Type3")));
    assert!(!fonts.iter().any(|(_, d)| d.name(b"Subtype") == Some(b"Type0")));
}

#[test]
fn added_text_from_base_backwards_compatible_and_safe() {
    assert_eq!(Family::from_base(b"Helvetica"), (Family::Helvetica, false, false));
    assert_eq!(Family::from_base(b"Times-Bold"), (Family::Times, true, false));
    assert_eq!(Family::from_base(b"Courier-BoldOblique"), (Family::Courier, true, true));
    assert_eq!(Family::from_base(b"UnknownCustomFont"), (Family::Helvetica, false, false));
}
