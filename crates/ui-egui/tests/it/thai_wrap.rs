//! Thai labels wrap at U+200B, and rows never start with a mark or a following vowel or end with a
//! leading vowel (upstream's epaint line breaking; the earlier vendored patch is retired).

use egui::epaint::text::{Fonts, LayoutJob, TextOptions};
use egui::{Color32, FontId};
use pdfcraft_ui_egui::theme;

/// The rows `text` wraps into at `width` points, with the app's fonts and a Thai face: the
/// craft-fonts one built in, or else the installed one. `None` when this machine has neither.
fn rows(text: &str, width: f32) -> Option<Vec<String>> {
    let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
    if pdfcraft_fonts::ui_thai_fonts().is_empty() && !defs.font_data.contains_key(theme::SYSTEM_THAI_FALLBACK) {
        return None;
    }
    let mut fonts = Fonts::new(TextOptions::default(), defs);
    fonts.set_line_breaker(std::sync::Arc::new(pdfcraft_ui_egui::ThaiLineBreaker));
    let mut view = fonts.with_pixels_per_point(1.0);
    let mut job = LayoutJob::simple(text.to_owned(), FontId::proportional(14.0), Color32::WHITE, width);
    job.wrap.break_anywhere = false;
    let galley = view.layout_job(job);
    Some(galley.rows.iter().map(|r| r.row.glyphs.iter().map(|g| g.chr).collect()).collect())
}

fn is_mark(c: char) -> bool {
    matches!(c, '\u{0E31}' | '\u{0E34}'..='\u{0E3A}' | '\u{0E47}'..='\u{0E4E}')
}

/// Every width from `widths`, so a rule holds wherever the row happens to fill up, not at one
/// lucky width.
fn rows_at_widths(text: &str, widths: std::ops::RangeInclusive<u16>) -> Option<Vec<Vec<String>>> {
    widths.map(|w| rows(text, f32::from(w))).collect()
}

#[test]
fn epaint_breaks_at_zero_width_space() {
    // Wider than any one word, so every row can end at a U+200B.
    let Some(all) = rows_at_widths("ผม\u{200B}กิน\u{200B}ข้าว\u{200B}ทุก\u{200B}วัน", 40..=90) else {
        eprintln!("skipping epaint_breaks_at_zero_width_space: no Thai font (build with CRAFT_FONTS_DIR, or install one)");
        return;
    };
    assert!(all.iter().any(|rows| rows.len() > 1), "{all:?}");
    for rows in &all {
        for row in rows {
            let first = row.trim_start_matches('\u{200B}').chars().next();
            assert!(matches!(first, Some('ผ' | 'ก' | 'ข' | 'ท' | 'ว')), "row starts mid-word: {rows:?}");
        }
    }
}

#[test]
fn epaint_never_breaks_before_a_mark() {
    let Some(all) = rows_at_widths("น้ำน้ำน้ำน้ำน้ำน้ำน้ำน้ำน้ำน้ำ", 20..=80) else {
        eprintln!("skipping epaint_never_breaks_before_a_mark: no Thai font (build with CRAFT_FONTS_DIR, or install one)");
        return;
    };
    assert!(all.iter().any(|rows| rows.len() > 1), "{all:?}");
    for rows in &all {
        for row in rows {
            assert!(!row.chars().next().is_some_and(is_mark), "row starts with a mark: {rows:?}");
        }
    }
}

#[test]
fn epaint_never_strands_a_leading_vowel() {
    // เ is written before the consonant it follows in speech: a row must not end with it.
    let Some(all) = rows_at_widths("เกเกเกเกเกเกเกเกเกเกเกเกเกเก", 20..=80) else {
        eprintln!("skipping epaint_never_strands_a_leading_vowel: no Thai font (build with CRAFT_FONTS_DIR, or install one)");
        return;
    };
    assert!(all.iter().any(|rows| rows.len() > 1), "{all:?}");
    for rows in &all {
        for row in rows.iter().take(rows.len().saturating_sub(1)) {
            assert!(!row.ends_with('เ'), "row ends with a leading vowel: {rows:?}");
        }
    }
}

#[test]
fn epaint_never_starts_a_row_with_a_following_vowel() {
    // า is written after its consonant and belongs to its syllable: a row must not start with it.
    let Some(all) = rows_at_widths("กากากากากากากากากากากากากากา", 20..=80) else {
        eprintln!("skipping epaint_never_starts_a_row_with_a_following_vowel: no Thai font (build with CRAFT_FONTS_DIR, or install one)");
        return;
    };
    assert!(all.iter().any(|rows| rows.len() > 1), "{all:?}");
    for rows in &all {
        for row in rows {
            assert!(!row.starts_with('า'), "row starts with a following vowel: {rows:?}");
        }
    }
}
