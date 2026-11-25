//! The Thai faces a form field's Font list offers: the bundled ones that a form can embed, then the
//! Thai-capable faces found on this machine and in the app's own font folder.
//!
//! An installed face a form may carry (an openly licensed, static font: see
//! [`pdfcraft_fonts::thai_faces::check`]) is registered and embedded as it is. Any other Thai face
//! is listed too, marked `substituted`: choosing it uses Sarabun, which the form embeds, so the form
//! reads the same on every machine.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;

use craft_font_catalog::{Catalog, FaceInfo};
use pdfcraft_fonts::thai_faces::{self, FaceId};
use pdfcraft_forms::FieldFont;

/// Where a listed face comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Embedded in this application.
    Bundled,
    /// Installed on this machine, or in the app's font folder.
    Installed,
}

/// One Thai face in the Font list.
#[derive(Clone, Debug, PartialEq)]
pub struct CatalogEntry {
    /// The name the list shows.
    pub family: String,
    /// What choosing it sets (for a `substituted` face, Sarabun).
    pub font: FieldFont,
    pub source: Source,
    /// The face cannot be embedded in a form, so `font` is Sarabun.
    pub substituted: bool,
}

/// OS/2 `ulCodePageRange1` bit 16: the Thai code page (874).
const THAI_CODE_PAGE: u32 = 1 << 16;

/// `%LOCALAPPDATA%\PdfCraft\Fonts` on Windows (`$XDG_DATA_HOME/pdfcraft/fonts` elsewhere): fonts put
/// there are found without being installed.
pub fn app_font_dir() -> Option<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty()).map(PathBuf::from);
    if cfg!(windows) {
        var("LOCALAPPDATA").map(|d| d.join("PdfCraft").join("Fonts"))
    } else {
        var("XDG_DATA_HOME").or_else(|| var("HOME").map(|h| h.join(".local").join("share"))).map(|d| d.join("pdfcraft").join("fonts"))
    }
}

/// The catalogue of this machine's fonts the Font list reads: the system catalogue with the app's
/// font folder added, or, with `PDFCRAFT_SYSTEM_FONTS=0`, only the files in that folder.
pub fn installed_catalog() -> Arc<Catalog> {
    let dir = app_font_dir();
    if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|v| v == "0") {
        let files = dir
            .and_then(|d| std::fs::read_dir(d).ok())
            .map(|read| {
                read.filter_map(Result::ok)
                    .map(|e| e.path())
                    .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ttf") || x.eq_ignore_ascii_case("otf")))
                    .collect()
            })
            .unwrap_or_default();
        return Arc::new(Catalog::of_files(files));
    }
    let catalog = Catalog::global();
    catalog.set_app_dirs(dir.into_iter().collect());
    Arc::clone(catalog)
}

fn is_thai_regular(face: &FaceInfo) -> bool {
    face.code_pages[0] & THAI_CODE_PAGE != 0 && !face.italic && face.variations.is_empty() && face.style.eq_ignore_ascii_case("Regular")
}

/// The bundled faces a form can embed (used on web and while the system catalogue scans).
pub fn bundled_entries() -> Vec<CatalogEntry> {
    thai_faces::bundled()
        .into_iter()
        .map(|id| CatalogEntry { family: thai_faces::family(id).to_string(), font: FieldFont::Thai(id), source: Source::Bundled, substituted: false })
        .collect()
}

/// The bundled faces a form can embed, then the Thai faces `catalog` lists (Regular style, by
/// family). Reads each candidate's file, so call it when the Font list opens, not every frame.
pub fn thai_entries(catalog: &Catalog) -> Vec<CatalogEntry> {
    let mut out = bundled_entries();
    let mut seen: HashSet<FaceId> = thai_faces::bundled().into_iter().collect();
    let mut seen_substituted = HashSet::new();
    let sarabun = thai_faces::sarabun();
    let mut faces: Vec<FaceInfo> = catalog.faces().iter().filter(|f| is_thai_regular(f)).cloned().collect();
    faces.sort_by(|a, b| a.family.cmp(&b.family));
    for face in faces {
        let Ok(data) = catalog.load(&face) else { continue };
        match thai_faces::check(&data.bytes).and_then(|()| thai_faces::register(data.bytes.to_vec())) {
            Ok(id) => {
                if seen.insert(id) {
                    out.push(CatalogEntry { family: face.family, font: FieldFont::Thai(id), source: Source::Installed, substituted: false });
                }
            }
            Err(_) => {
                if let Some(sarabun) = sarabun
                    && seen_substituted.insert(face.family.clone())
                {
                    out.push(CatalogEntry { family: face.family, font: FieldFont::Thai(sarabun), source: Source::Installed, substituted: true });
                }
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A copy of `bytes` that differs from it (an unreferenced trailer changes no table), written
    /// to a fresh folder, and a catalog of just that file.
    fn catalog_of(name: &str, bytes: &[u8]) -> (std::path::PathBuf, Catalog) {
        let dir = std::env::temp_dir().join(format!("pdfcraft-thai-catalog-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let mut copy = bytes.to_vec();
        copy.extend_from_slice(&7u32.to_le_bytes());
        let file = dir.join(format!("{name}.ttf"));
        std::fs::write(&file, copy).unwrap();
        (dir, Catalog::of_files(vec![file]))
    }

    fn installed(entries: Vec<CatalogEntry>) -> Vec<CatalogEntry> {
        entries.into_iter().filter(|e| e.source == Source::Installed).collect()
    }

    #[test]
    fn a_licensed_thai_face_in_a_folder_is_listed_and_embeddable() {
        let Some(sarabun) = pdfcraft_fonts::thai_faces::sarabun() else { return };
        let (dir, catalog) = catalog_of("licensed", pdfcraft_fonts::thai_faces::bytes(sarabun));
        let found = installed(thai_entries(&catalog));
        std::fs::remove_dir_all(dir).unwrap();
        let [entry] = &found[..] else { panic!("one installed face: {found:?}") };
        assert!(!entry.substituted, "{entry:?}");
        assert_ne!(entry.font, FieldFont::Thai(sarabun), "a different file is a face of its own");
    }

    #[test]
    fn a_latin_only_face_is_not_listed() {
        let inter = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Inter-Regular.ttf")).unwrap();
        let (dir, catalog) = catalog_of("latin", &inter);
        let found = installed(thai_entries(&catalog));
        std::fs::remove_dir_all(dir).unwrap();
        assert!(found.is_empty(), "{found:?}");
    }

    #[test]
    fn a_face_the_embedding_policy_refuses_is_listed_as_substituted() {
        let (Some(sarabun), Some(variable)) = (pdfcraft_fonts::thai_faces::sarabun(), pdfcraft_fonts::document_thai_font()) else { return };
        // The bundled Noto Sans Thai is a variable font, which a form cannot carry.
        let (dir, catalog) = catalog_of("variable", variable.bytes);
        let found = installed(thai_entries(&catalog));
        std::fs::remove_dir_all(dir).unwrap();
        let [entry] = &found[..] else { panic!("one installed face: {found:?}") };
        assert!(entry.substituted, "{entry:?}");
        assert_eq!(entry.font, FieldFont::Thai(sarabun), "it shows as Sarabun");
    }

    #[test]
    fn bundled_faces_come_first_and_are_embeddable() {
        let all = thai_entries(&Catalog::of_files(Vec::new()));
        let bundled: Vec<_> = all.iter().filter(|e| e.source == Source::Bundled).collect();
        assert_eq!(bundled.len(), pdfcraft_fonts::thai_faces::bundled().len());
        assert!(bundled.iter().all(|e| !e.substituted));
        assert!(all.iter().take(bundled.len()).all(|e| e.source == Source::Bundled));
    }
}
