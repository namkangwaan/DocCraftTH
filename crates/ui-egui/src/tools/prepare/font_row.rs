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
/// catalogue in the background (rebuilding whenever `catalog.generation()` advances) without
/// blocking the UI thread; on web, returns the bundled faces.
#[cfg(not(target_arch = "wasm32"))]
pub(crate) fn catalog_entries(ctx: Option<&egui::Context>) -> Vec<CatalogEntry> {
    use std::sync::{Arc, Mutex, OnceLock, PoisonError};
    struct Cache {
        generation: Option<u64>,
        entries: Vec<CatalogEntry>,
        building: Option<u64>,
    }
    static CATALOG: OnceLock<Arc<craft::font_catalog::Catalog>> = OnceLock::new();
    static CACHE: Mutex<Cache> = Mutex::new(Cache { generation: None, entries: Vec::new(), building: None });

    let catalog = CATALOG.get_or_init(pdfcraft_engine::thai_catalog::installed_catalog);
    catalog.rescan_if_changed();
    if !catalog.is_ready() {
        catalog.scan_in_background();
        if let Some(ctx) = ctx {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        return pdfcraft_engine::thai_catalog::bundled_entries();
    }
    let cur_gen = catalog.generation();
    let mut cache = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
    if cache.generation == Some(cur_gen) {
        return cache.entries.clone();
    }
    if cache.building != Some(cur_gen) {
        cache.building = Some(cur_gen);
        let cat = Arc::clone(catalog);
        let repaint_ctx = ctx.cloned();
        std::thread::spawn(move || {
            let entries = pdfcraft_engine::thai_catalog::thai_entries(&cat);
            let mut c = CACHE.lock().unwrap_or_else(PoisonError::into_inner);
            c.generation = Some(cur_gen);
            c.entries = entries;
            if c.building == Some(cur_gen) {
                c.building = None;
            }
            if let Some(ctx) = repaint_ctx {
                ctx.request_repaint();
            }
        });
    }
    if cache.entries.is_empty() { pdfcraft_engine::thai_catalog::bundled_entries() } else { cache.entries.clone() }
}

#[cfg(target_arch = "wasm32")]
pub(crate) fn catalog_entries(_ctx: Option<&egui::Context>) -> Vec<CatalogEntry> {
    pdfcraft_engine::thai_catalog::bundled_entries()
}

/// The embeddable Thai faces (excluding substituted ones), for Shared Field Properties.
pub(crate) fn embeddable_entries() -> Vec<CatalogEntry> {
    catalog_entries(None).into_iter().filter(|e| !e.substituted).collect()
}

/// Draws the Font row and any inline notice below it using the active catalogue entries.
pub(super) fn show(ui: &mut egui::Ui, d: &mut FieldDraft) {
    let entries = catalog_entries(Some(ui.ctx()));
    show_with_entries(ui, d, &entries);
}

/// Draws the Font row and any inline notice below it for an explicit list of Thai `entries`.
pub fn show_with_entries(ui: &mut egui::Ui, d: &mut FieldDraft, thai: &[CatalogEntry]) {
    let Some(l) = d.look.as_mut() else { return };
    if d.substituted_family.as_ref().is_some_and(|fam| !thai.iter().any(|e| e.substituted && e.family == *fam && e.font == l.font)) {
        d.substituted_family = None;
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
    } else if let Some(ref fam) = d.substituted_family
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
            d.substituted_family = None;
        } else if let Some(entry) = thai.get(idx - thai_base) {
            l.font = entry.font;
            d.face = match entry.font {
                FieldFont::Thai(id) => Some(FieldFace::Thai(id)),
                _ => Some(FieldFace::Standard),
            };
            d.substituted_family = entry.substituted.then(|| entry.family.clone());
        }
    }

    if let Some(ref fam) = d.substituted_family {
        let msg = crate::i18n::fmt(tl!("{name} can't be embedded in a form. The field will use Sarabun."), &[("name", fam)]);
        Notice::new(Tone::Info, msg).layout(NoticeLayout::Inline).show(ui);
    } else if thai.is_empty() {
        Notice::new(Tone::Info, tl!("No Thai font is built in. Thai text uses a font installed on this computer on screen only."))
            .layout(NoticeLayout::Inline)
            .show(ui);
    }
}
