//! The Font row on the Field Properties ▸ Appearance tab: standard PDF fonts, bundled Thai faces,
//! and Thai faces found on this machine, plus an inline notice when a non-embeddable face is
//! substituted by Sarabun or when the build carries no Thai font.

use craft::theme::variant::Tone;
use craft::ui::contract::{Bind, Entry, Props as _, Show as _, ShowContainer as _};
use craft::ui::form::Field;
use craft::ui::notices::{Notice, NoticeLayout};
use craft::ui::pickers::Select;
use pdfcraft_engine::FieldFace;
use pdfcraft_engine::thai_catalog::CatalogEntry;

use super::FieldFont;
use super::draft::FieldDraft;

/// The Thai faces available to the Font dropdown: on native, scans the system and app font
/// catalogue in the background and caches the result once ready; on web, returns the bundled faces.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn catalog_entries() -> Vec<CatalogEntry> {
    use std::sync::{Arc, OnceLock};
    static CATALOG: OnceLock<Arc<craft::font_catalog::Catalog>> = OnceLock::new();
    static ENTRIES: OnceLock<Vec<CatalogEntry>> = OnceLock::new();
    if let Some(entries) = ENTRIES.get() {
        return entries.clone();
    }
    let catalog = CATALOG.get_or_init(pdfcraft_engine::thai_catalog::installed_catalog);
    if catalog.is_ready() {
        ENTRIES.get_or_init(|| pdfcraft_engine::thai_catalog::thai_entries(catalog)).clone()
    } else {
        catalog.scan_in_background();
        pdfcraft_engine::thai_catalog::bundled_entries()
    }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn catalog_entries() -> Vec<CatalogEntry> {
    pdfcraft_engine::thai_catalog::bundled_entries()
}

/// The embeddable Thai faces (excluding substituted ones), for Shared Field Properties.
pub(crate) fn embeddable_entries() -> Vec<CatalogEntry> {
    catalog_entries().into_iter().filter(|e| !e.substituted).collect()
}

/// Draws the Font row and any inline notice below it using the active catalogue entries.
pub(super) fn show(ui: &mut egui::Ui, d: &mut FieldDraft) {
    let entries = catalog_entries();
    show_with_entries(ui, d, &entries);
}

/// Draws the Font row and any inline notice below it for an explicit list of Thai `entries`.
pub fn show_with_entries(ui: &mut egui::Ui, d: &mut FieldDraft, thai: &[CatalogEntry]) {
    let Some(l) = d.look.as_mut() else { return };
    let sub_id = egui::Id::unique(("thai-font-sub", &d.field));
    let mut chosen_sub: Option<String> = ui.data_mut(|data| data.get_temp(sub_id)).flatten();
    if chosen_sub.as_ref().is_some_and(|fam| !thai.iter().any(|e| e.substituted && e.family == *fam && e.font == l.font)) {
        chosen_sub = None;
        ui.data_mut(|data| data.remove::<Option<String>>(sub_id));
    }

    let mut choices: Vec<Entry<i64>> = FieldFont::ALL.iter().enumerate().map(|(i, f)| Entry::new(i as i64, tl!(f.label()).to_string())).collect();
    let thai_base = choices.len();
    for (i, entry) in thai.iter().enumerate() {
        let label = if entry.substituted { format!("{} ({})", entry.family, tl!("shown as Sarabun")) } else { entry.family.clone() };
        choices.push(Entry::new((thai_base + i) as i64, label));
    }
    let unknown_idx = if let Some(FieldFace::Unknown(ref name)) = d.face {
        let idx = choices.len() as i64;
        choices.push(Entry::new(idx, format!("{name} ({})", tl!("kept as is"))));
        Some(idx)
    } else {
        None
    };

    let current: i64 = if let Some(idx) = unknown_idx {
        idx
    } else if let Some(ref fam) = chosen_sub
        && let Some(pos) = thai.iter().position(|e| e.substituted && e.family == *fam)
    {
        (thai_base + pos) as i64
    } else if let Some(pos) = thai.iter().position(|e| !e.substituted && e.font == l.font) {
        (thai_base + pos) as i64
    } else {
        FieldFont::ALL.iter().position(|f| *f == l.font).unwrap_or(0) as i64
    };

    let mut picked = current;
    let label = tl!("Font:");
    Field::new(label.to_owned()).id_salt("font").show(ui, |ui, _| {
        Select::new(label.to_owned(), Bind::bound(&mut picked), &choices).show(ui);
    });

    if picked != current {
        let idx = picked as usize;
        if idx < FieldFont::ALL.len() {
            l.font = FieldFont::ALL[idx];
            d.face = Some(FieldFace::Standard);
            chosen_sub = None;
            ui.data_mut(|data| data.remove::<Option<String>>(sub_id));
        } else if let Some(entry) = thai.get(idx - thai_base) {
            l.font = entry.font;
            d.face = match entry.font {
                FieldFont::Thai(id) => Some(FieldFace::Thai(id)),
                _ => Some(FieldFace::Standard),
            };
            if entry.substituted {
                chosen_sub = Some(entry.family.clone());
                ui.data_mut(|data| data.insert_temp(sub_id, Some(entry.family.clone())));
            } else {
                chosen_sub = None;
                ui.data_mut(|data| data.remove::<Option<String>>(sub_id));
            }
        }
    }

    if let Some(fam) = chosen_sub {
        let msg = crate::i18n::fmt(tl!("{name} can't be embedded in a form. The field will use Sarabun."), &[("name", &fam)]);
        Notice::new(Tone::Info, msg).layout(NoticeLayout::Inline).show(ui);
    } else if thai.is_empty() {
        Notice::new(Tone::Info, tl!("No Thai font is built in. Thai text uses a font installed on this computer on screen only."))
            .layout(NoticeLayout::Inline)
            .show(ui);
    }
}
