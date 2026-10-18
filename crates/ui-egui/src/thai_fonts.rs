//! Thai faces for the interface and for Thai text.
//!
//! Noto Sans Thai Medium draws English and Thai interface text. Noto Sans Thai Looped is the
//! default face for Thai text alone. Sarabun, Mitr, Kanit and Athiti are always offered in
//! Preferences. The other families are optional. Siamreap is not offered.
//!
//! A face that ships in this application is labeled as such. A face read from the operating
//! system is labeled as such, drawn on screen only, and never written into a PDF (AGENTS.md §1.4).
//! Windows reads its Fonts folders. macOS reads the Library Fonts folders. Linux reads the shared
//! font directories and Fontconfig. `PDFCRAFT_SYSTEM_FONTS=0` leaves operating-system files unread.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use egui::{FontData, FontDefinitions, FontFamily, FontId, RichText};

use crate::PdfCraftApp;
use crate::theme::{self, Tokens};
use crate::widgets;

/// The Thai text face used until the user picks another.
pub const DEFAULT_THAI_FONT: &str = "Noto Sans Thai Looped";

/// Font data key for Noto Sans Thai Medium at the front of the interface families.
pub const UI_FACE: &str = "noto-sans-thai-medium";
/// Font data key and named family for the chosen Thai text face.
pub const TEXT_FACE: &str = "thai-text";

const MAX_BYTES: u64 = 32 << 20;
const MAX_FACES: u32 = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Role {
    /// English and Thai interface text. Not a choice in the list.
    Interface,
    /// Default face for Thai text.
    ThaiText,
    /// Always listed.
    Primary,
    /// Listed only when optional fonts are on, or when this one is already chosen.
    Secondary,
}

/// Where the face that would be drawn comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// Bytes embedded with this application.
    Application,
    /// A file already installed on this machine.
    OperatingSystem,
    /// Neither is present.
    Missing,
}

struct Face {
    family: &'static str,
    role: Role,
    /// File name under a font directory.
    file: &'static str,
    /// Style matched against an embedded face (`Medium`, `Regular`).
    style: &'static str,
}

const CATALOG: &[Face] = &[
    Face { family: "Noto Sans Thai", role: Role::Interface, file: "NotoSansThai-Medium.ttf", style: "Medium" },
    Face { family: "Noto Sans Thai Looped", role: Role::ThaiText, file: "NotoSansThaiLooped-Regular.ttf", style: "Regular" },
    Face { family: "Sarabun", role: Role::Primary, file: "Sarabun-Regular.ttf", style: "Regular" },
    Face { family: "Mitr", role: Role::Primary, file: "Mitr-Regular.ttf", style: "Regular" },
    Face { family: "Kanit", role: Role::Primary, file: "Kanit-Regular.ttf", style: "Regular" },
    Face { family: "Athiti", role: Role::Primary, file: "Athiti-Regular.ttf", style: "Regular" },
    Face { family: "Bai Jamjuree", role: Role::Secondary, file: "BaiJamjuree-Regular.ttf", style: "Regular" },
    Face { family: "Chakra Petch", role: Role::Secondary, file: "ChakraPetch-Regular.ttf", style: "Regular" },
    Face { family: "Charm", role: Role::Secondary, file: "Charm-Regular.ttf", style: "Regular" },
    Face { family: "Charmonman", role: Role::Secondary, file: "Charmonman-Regular.ttf", style: "Regular" },
    Face { family: "Chonburi", role: Role::Secondary, file: "Chonburi-Regular.ttf", style: "Regular" },
    Face { family: "Fahkwang", role: Role::Secondary, file: "Fahkwang-Regular.ttf", style: "Regular" },
    Face { family: "Itim", role: Role::Secondary, file: "Itim-Regular.ttf", style: "Regular" },
    Face { family: "K2D", role: Role::Secondary, file: "K2D-Regular.ttf", style: "Regular" },
    Face { family: "KoHo", role: Role::Secondary, file: "KoHo-Regular.ttf", style: "Regular" },
    Face { family: "Kodchasan", role: Role::Secondary, file: "Kodchasan-Regular.ttf", style: "Regular" },
    Face { family: "Krub", role: Role::Secondary, file: "Krub-Regular.ttf", style: "Regular" },
    Face { family: "Maitree", role: Role::Secondary, file: "Maitree-Regular.ttf", style: "Regular" },
    Face { family: "Mali", role: Role::Secondary, file: "Mali-Regular.ttf", style: "Regular" },
    Face { family: "Noto Serif Thai", role: Role::Secondary, file: "NotoSerifThai-Regular.ttf", style: "Regular" },
    Face { family: "Pattaya", role: Role::Secondary, file: "Pattaya-Regular.ttf", style: "Regular" },
    Face { family: "Pridi", role: Role::Secondary, file: "Pridi-Regular.ttf", style: "Regular" },
    Face { family: "Prompt", role: Role::Secondary, file: "Prompt-Regular.ttf", style: "Regular" },
    Face { family: "Srisakdi", role: Role::Secondary, file: "Srisakdi-Regular.ttf", style: "Regular" },
    Face { family: "Sriracha", role: Role::Secondary, file: "Sriracha-Regular.ttf", style: "Regular" },
    Face { family: "Taviraj", role: Role::Secondary, file: "Taviraj-Regular.ttf", style: "Regular" },
    Face { family: "Thasadith", role: Role::Secondary, file: "Thasadith-Regular.ttf", style: "Regular" },
    Face { family: "Trirong", role: Role::Secondary, file: "Trirong-Regular.ttf", style: "Regular" },
];

/// A catalog family the Thai text list may store. Interface-only and unknown names, including
/// Siamreap, are refused.
pub fn known_choice(name: &str) -> Option<&'static str> {
    CATALOG.iter().find(|face| face.role != Role::Interface && face.family == name).map(|face| face.family)
}

/// Put the interface face and the chosen Thai text face into `fonts`.
///
/// The interface face leads the proportional, medium and semibold families, so English and Thai
/// interface text use it. The Thai text face leads a named family copied from proportional, so
/// the fallbacks already on that stack stay in place. Monospace is left alone.
pub fn install(fonts: &mut FontDefinitions, thai_family: &str) {
    if let Some(data) = load_ui() {
        fonts.font_data.insert(UI_FACE.to_owned(), data);
        for family in [FontFamily::Proportional, FontFamily::Name("medium".into()), FontFamily::Name("semibold".into())] {
            if let Some(stack) = fonts.families.get_mut(&family) {
                stack.retain(|name| name != UI_FACE);
                stack.insert(0, UI_FACE.to_owned());
            }
        }
    }
    if let Some(data) = load_text(thai_family) {
        fonts.font_data.insert(TEXT_FACE.to_owned(), data);
        let mut stack = fonts.families.get(&FontFamily::Proportional).cloned().unwrap_or_default();
        stack.retain(|name| name != TEXT_FACE);
        stack.insert(0, TEXT_FACE.to_owned());
        fonts.families.insert(FontFamily::Name(TEXT_FACE.into()), stack);
    }
}

/// Preferences ▸ Fonts.
pub fn preferences_section(ui: &mut egui::Ui, app: &mut PdfCraftApp, t: &Tokens) {
    ui.add_space(8.0);
    ui.label(RichText::new(say("Fonts")).font(theme::semibold(13.0)));
    let interface = face_named("Noto Sans Thai");
    source_row(ui, say("Interface font"), "Noto Sans Thai Medium", source_of(interface));
    ui.label(RichText::new(say("Noto Sans Thai Medium draws the English and Thai interface.")).small().color(t.text_muted));
    let thai = face_named(DEFAULT_THAI_FONT);
    source_row(ui, say("Thai text"), DEFAULT_THAI_FONT, source_of(thai));
    ui.label(
        RichText::new(say("Noto Sans Thai Looped draws Thai only. English interface text stays on the interface font.")).small().color(t.text_muted),
    );
    ui.horizontal(|ui| {
        ui.label(say("Thai text font"));
        let selected = choice_label(&app.thai_font);
        egui::ComboBox::from_id_salt("thai-text-font").selected_text(selected).show_ui(ui, |ui| {
            for face in choices(app.thai_optional_fonts, &app.thai_font) {
                let label = choice_label(face.family);
                if ui.selectable_value(&mut app.thai_font, face.family.to_owned(), label).clicked() {
                    ui.close();
                }
            }
        });
    });
    ui.add(widgets::checkbox(&mut app.thai_optional_fonts, say("Show optional fonts")));
    ui.label(RichText::new("English และภาษาไทย").font(theme::regular(16.0)));
    ui.label(RichText::new("สวัสดี ภาษาไทย").font(FontId::new(16.0, FontFamily::Name(TEXT_FACE.into()))));
    ui.label(RichText::new(say("Operating system fonts are drawn on screen only. They are not written into a PDF.")).small().color(t.text_muted));
}

fn source_row(ui: &mut egui::Ui, title: &str, family: &str, source: Source) {
    ui.horizontal(|ui| {
        ui.label(title);
        ui.label(family);
        ui.label(RichText::new(say(source_word(source))).small());
    });
}

fn choices(optional: bool, selected: &str) -> Vec<&'static Face> {
    CATALOG
        .iter()
        .filter(|face| match face.role {
            Role::Interface => false,
            Role::ThaiText | Role::Primary => true,
            Role::Secondary => optional || face.family == selected,
        })
        .collect()
}

fn choice_label(family: &str) -> String {
    let Some(face) = CATALOG.iter().find(|face| face.family == family) else {
        return family.to_owned();
    };
    format!("{} · {} · {}", face.family, say(tier_word(face.role)), say(source_word(source_of(Some(face)))))
}

fn tier_word(role: Role) -> &'static str {
    match role {
        Role::Interface => "Interface font",
        Role::ThaiText => "Thai text",
        Role::Primary => "Primary",
        Role::Secondary => "Optional font",
    }
}

fn source_word(source: Source) -> &'static str {
    match source {
        Source::Application => "This application",
        Source::OperatingSystem => "Operating system",
        Source::Missing => "Not installed",
    }
}

fn say(english: &'static str) -> &'static str {
    crate::i18n::tr(crate::i18n::current(), english)
}

fn face_named(family: &str) -> Option<&'static Face> {
    CATALOG.iter().find(|face| face.family == family)
}

fn source_of(face: Option<&Face>) -> Source {
    let Some(face) = face else { return Source::Missing };
    if application_bytes(face).is_some() {
        Source::Application
    } else if os_path(face).is_some() {
        Source::OperatingSystem
    } else {
        Source::Missing
    }
}

fn load_ui() -> Option<Arc<FontData>> {
    let face = face_named("Noto Sans Thai")?;
    let data = load_face(face, 'A')?;
    covers(&data.font, data.index, '\u{0E01}').then_some(data)
}

fn load_text(family: &str) -> Option<Arc<FontData>> {
    let face = CATALOG.iter().find(|face| face.role != Role::Interface && face.family == family)?;
    load_face(face, '\u{0E01}')
}

fn load_face(face: &Face, probe: char) -> Option<Arc<FontData>> {
    if let Some(bytes) = application_bytes(face) {
        return owned_face(bytes.to_vec(), probe);
    }
    let path = os_path(face)?;
    if !system_fonts_enabled() {
        return None;
    }
    read_face(&path, probe)
}

fn application_bytes(face: &Face) -> Option<&'static [u8]> {
    pdfcraft_fonts::ui_thai_fonts()
        .into_iter()
        .find(|font| font.family.eq_ignore_ascii_case(face.family) && font.style.eq_ignore_ascii_case(face.style) && !font.bytes.is_empty())
        .map(|font| font.bytes)
}

fn os_path(face: &Face) -> Option<PathBuf> {
    // A file sitting directly in a font folder, which is how Windows stores faces. The join is
    // case-insensitive there, so it stays ahead of the walk below.
    font_roots().into_iter().map(|root| root.join(face.file)).find(|path| path.is_file()).or_else(|| nested_catalog_file(face.file))
}

/// A catalog file nested under a font folder. macOS and Linux install faces in subdirectories,
/// and Fontconfig may point at a store path that is not one of [`font_roots`].
fn nested_catalog_file(file: &str) -> Option<PathBuf> {
    catalog_files().get(file).cloned()
}

fn catalog_files() -> &'static HashMap<String, PathBuf> {
    static FILES: OnceLock<HashMap<String, PathBuf>> = OnceLock::new();
    FILES.get_or_init(|| {
        let names: HashSet<&str> = CATALOG.iter().map(|face| face.file).collect();
        let found = index_tree(&font_roots(), &names, 8, 8_192);
        #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
        let found = with_fontconfig(&names, found);
        found
    })
}

#[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
fn with_fontconfig(names: &HashSet<&str>, mut found: HashMap<String, PathBuf>) -> HashMap<String, PathBuf> {
    if found.len() < names.len() {
        for path in crate::system_fonts::paths_named(names) {
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                found.entry(name.to_string()).or_insert(path);
            }
        }
    }
    found
}

/// Catalog files under `roots`, at most `max_depth` directories down and `max_entries` directory
/// entries in all. Each directory is visited once, so a repeated path or a link cycle stops.
fn index_tree(roots: &[PathBuf], names: &HashSet<&str>, max_depth: usize, max_entries: usize) -> HashMap<String, PathBuf> {
    let mut found = HashMap::new();
    let mut seen = HashSet::new();
    let mut entries = 0usize;
    let mut pending: Vec<(PathBuf, usize)> = roots.iter().rev().cloned().map(|root| (root, 0)).collect();
    while let Some((dir, depth)) = pending.pop() {
        if found.len() == names.len() || entries >= max_entries {
            break;
        }
        let Ok(canonical) = dir.canonicalize() else { continue };
        if !seen.insert(canonical) {
            continue;
        }
        let Ok(read) = std::fs::read_dir(&dir) else { continue };
        for entry in read {
            if found.len() == names.len() || entries >= max_entries {
                break;
            }
            entries = entries.saturating_add(1);
            let Ok(entry) = entry else { continue };
            let path = entry.path();
            if path.is_dir() {
                if depth < max_depth {
                    pending.push((path, depth.saturating_add(1)));
                }
                continue;
            }
            let Some(name) = path.file_name().and_then(|name| name.to_str()) else { continue };
            if names.contains(name) {
                found.entry(name.to_string()).or_insert(path);
            }
        }
    }
    found
}

fn font_roots() -> Vec<PathBuf> {
    #[cfg(target_os = "windows")]
    {
        let mut roots = Vec::new();
        let windir =
            std::env::var_os("WINDIR").or_else(|| std::env::var_os("SystemRoot")).map_or_else(|| PathBuf::from(r"C:\Windows"), PathBuf::from);
        roots.push(windir.join("Fonts"));
        if let Some(local) = std::env::var_os("LOCALAPPDATA") {
            roots.push(PathBuf::from(local).join("Microsoft").join("Windows").join("Fonts"));
        }
        roots
    }
    #[cfg(target_os = "macos")]
    {
        let mut roots = Vec::new();
        if let Some(home) = std::env::var_os("HOME") {
            roots.push(PathBuf::from(home).join("Library").join("Fonts"));
        }
        roots.extend([
            PathBuf::from("/Library/Fonts"),
            PathBuf::from("/System/Library/Fonts"),
            PathBuf::from("/System/Library/Fonts/Supplemental"),
            PathBuf::from("/Network/Library/Fonts"),
        ]);
        roots
    }
    #[cfg(all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))]
    {
        let mut roots = vec![PathBuf::from("/usr/share/fonts"), PathBuf::from("/usr/local/share/fonts")];
        if let Some(home) = std::env::var_os("HOME") {
            let home = PathBuf::from(home);
            roots.push(home.join(".local").join("share").join("fonts"));
            roots.push(home.join(".fonts"));
        }
        roots
    }
    #[cfg(not(any(target_os = "windows", target_os = "macos", all(unix, not(target_os = "macos"), not(target_arch = "wasm32")))))]
    Vec::new()
}

fn system_fonts_enabled() -> bool {
    !std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|value| value == "0")
}

fn read_face(path: &Path, probe: char) -> Option<Arc<FontData>> {
    let meta = std::fs::metadata(path).ok()?;
    if !meta.is_file() || meta.len() == 0 || meta.len() > MAX_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    owned_face(bytes, probe)
}

fn owned_face(bytes: Vec<u8>, probe: char) -> Option<Arc<FontData>> {
    let index = face_index(&bytes, probe)?;
    let mut data = FontData::from_owned(bytes);
    data.index = index;
    Some(Arc::new(data))
}

fn face_index(bytes: &[u8], probe: char) -> Option<u32> {
    (0..MAX_FACES).find(|&index| covers(bytes, index, probe))
}

fn covers(bytes: &[u8], index: u32, probe: char) -> bool {
    use skrifa::MetadataProvider as _;
    skrifa::FontRef::from_index(bytes, index).is_ok_and(|font| font.charmap().map(probe).is_some())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Application bytes win over a file of the same family. A file wins over nothing.
    fn source_at(has_application: bool, roots: &[std::path::PathBuf], file: &str) -> Source {
        if has_application {
            Source::Application
        } else if roots.iter().any(|root| root.join(file).is_file()) {
            Source::OperatingSystem
        } else {
            Source::Missing
        }
    }

    #[test]
    fn siamreap_is_not_offered() {
        assert!(CATALOG.iter().all(|face| {
            let folded = face.family.to_ascii_lowercase();
            !folded.contains("siam")
                && !folded.contains("siem")
                && !face.file.to_ascii_lowercase().contains("siam")
                && !face.file.to_ascii_lowercase().contains("siem")
        }));
        assert!(known_choice("Siamreap").is_none());
        assert!(known_choice("Siemreap").is_none());
        assert!(known_choice("Siamreap-Regular").is_none());
        assert!(known_choice("../Sarabun").is_none());
        assert!(known_choice("Noto Sans Thai").is_none());
    }

    #[test]
    fn primaries_are_the_four_named_faces_and_looped_is_the_default() {
        let primaries: Vec<&str> = CATALOG.iter().filter(|face| face.role == Role::Primary).map(|face| face.family).collect();
        assert_eq!(primaries, ["Sarabun", "Mitr", "Kanit", "Athiti"]);
        assert_eq!(known_choice(DEFAULT_THAI_FONT), Some(DEFAULT_THAI_FONT));
        assert_eq!(CATALOG.iter().filter(|face| face.role == Role::Interface).count(), 1);
        assert_eq!(CATALOG.len(), 28);
    }

    #[test]
    fn optional_faces_stay_hidden_until_asked_except_the_one_already_chosen() {
        let shown = choices(false, DEFAULT_THAI_FONT);
        assert!(shown.iter().all(|face| face.role != Role::Secondary));
        assert_eq!(shown.len(), 5);
        let kept = choices(false, "Mali");
        assert!(kept.iter().any(|face| face.family == "Mali"));
        assert_eq!(kept.iter().filter(|face| face.role == Role::Secondary).count(), 1);
        assert_eq!(choices(true, DEFAULT_THAI_FONT).len(), 27);
    }

    #[test]
    fn an_application_face_wins_over_a_file_and_a_file_wins_over_nothing() {
        let dir = std::env::temp_dir().join("pdfcraft-thai-font-source");
        assert!(std::fs::create_dir_all(&dir).is_ok());
        let path = dir.join("Sarabun-Regular.ttf");
        assert!(std::fs::write(&path, b"x").is_ok());
        assert_eq!(source_at(true, &[dir.clone()], "Sarabun-Regular.ttf"), Source::Application);
        assert_eq!(source_at(false, &[dir.clone()], "Sarabun-Regular.ttf"), Source::OperatingSystem);
        assert_eq!(source_at(false, &[dir], "missing.ttf"), Source::Missing);
        assert_eq!(source_at(false, &[], "Sarabun-Regular.ttf"), Source::Missing);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn inter_covers_latin_and_not_thai() {
        let inter = pdfcraft_fonts::INTER_REGULAR;
        assert!(covers(inter, 0, 'A'));
        assert!(!covers(inter, 0, '\u{0E01}'));
        assert!(face_index(b"not a font", 'A').is_none());
    }

    #[test]
    fn the_choice_is_kept_and_an_unknown_name_is_ignored() {
        let mut app = crate::PdfCraftApp::default();
        assert_eq!(app.thai_font, DEFAULT_THAI_FONT);
        assert!(!app.thai_optional_fonts);
        app.thai_font = "Kanit".to_owned();
        app.thai_optional_fonts = true;
        let mut restored = crate::PdfCraftApp::default();
        restored.restore(&app.persist());
        assert_eq!(restored.thai_font, "Kanit");
        assert!(restored.thai_optional_fonts);
        restored.restore(r#"{"thai_font":"Siamreap-Regular","thai_optional_fonts":false}"#);
        assert_eq!(restored.thai_font, "Kanit");
        assert!(!restored.thai_optional_fonts);
    }

    #[test]
    fn the_interface_face_leads_proportional_when_it_loads() {
        let defs = theme::installed_font_definitions(theme::CjkPreference::Default);
        let leads = defs.families.get(&FontFamily::Proportional).and_then(|stack| stack.first()).is_some_and(|name| name == UI_FACE);
        assert_eq!(leads, load_ui().is_some());
        assert_eq!(defs.font_data.contains_key(TEXT_FACE), load_text(DEFAULT_THAI_FONT).is_some());
        if leads {
            for family in [FontFamily::Name("medium".into()), FontFamily::Name("semibold".into())] {
                assert_eq!(defs.families.get(&family).and_then(|stack| stack.first()).map(String::as_str), Some(UI_FACE));
            }
            let stack = &defs.families[&FontFamily::Name(TEXT_FACE.into())];
            assert_eq!(stack.first().map(String::as_str), Some(TEXT_FACE));
            assert!(stack.iter().any(|name| name == "Inter"));
        }
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn windows_noto_sans_thai_medium_covers_english_and_thai() {
        if std::env::var_os("PDFCRAFT_SYSTEM_FONTS").is_some_and(|value| value == "0") {
            return;
        }
        let Some(face) = face_named("Noto Sans Thai") else { return };
        let Some(path) = os_path(face) else { return };
        let Ok(bytes) = std::fs::read(&path) else { return };
        assert!(face_index(&bytes, 'A').is_some_and(|index| covers(&bytes, index, '\u{0E01}')), "{}", path.display());
    }

    #[test]
    fn a_nested_catalog_file_is_found_and_the_walk_stops() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-thai-walk-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let nested = dir.join("a").join("b");
        let too_deep = nested.join("c").join("d");
        assert!(std::fs::create_dir_all(&too_deep).is_ok());
        let sarabun = nested.join("Sarabun-Regular.ttf");
        let mali = too_deep.join("Mali-Regular.ttf");
        assert!(std::fs::write(&sarabun, b"x").is_ok());
        assert!(std::fs::write(&mali, b"x").is_ok());
        let names: HashSet<&str> = ["Sarabun-Regular.ttf", "Mali-Regular.ttf", "Kanit-Regular.ttf"].into_iter().collect();
        let found = index_tree(&[dir.clone(), dir.clone()], &names, 2, 100);
        assert_eq!(found.get("Sarabun-Regular.ttf").and_then(|path| path.file_name()).and_then(|name| name.to_str()), Some("Sarabun-Regular.ttf"));
        assert!(!found.contains_key("Mali-Regular.ttf"));
        assert!(!found.contains_key("Kanit-Regular.ttf"));
        let flat = dir.join("flat");
        assert!(std::fs::create_dir_all(&flat).is_ok());
        assert!(std::fs::write(flat.join("Sarabun-Regular.ttf"), b"x").is_ok());
        assert!(std::fs::write(flat.join("Mali-Regular.ttf"), b"x").is_ok());
        assert_eq!(index_tree(&[flat], &names, 2, 1).len(), 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn each_desktop_has_a_font_directory() {
        let roots = font_roots();
        if cfg!(target_os = "windows") || cfg!(target_os = "macos") {
            assert!(roots.iter().any(|path| path.ends_with("Fonts")));
        }
        if cfg!(all(unix, not(target_os = "macos"), not(target_arch = "wasm32"))) {
            assert!(roots.iter().any(|path| path.ends_with("fonts")));
        }
        if cfg!(target_arch = "wasm32") {
            assert!(roots.is_empty());
        }
    }

    #[test]
    fn a_rejected_name_does_not_install_a_thai_text_face() {
        let mut fonts = FontDefinitions::default();
        install(&mut fonts, "Siamreap");
        assert!(!fonts.font_data.contains_key(TEXT_FACE));
        assert!(!fonts.families.contains_key(&FontFamily::Name(TEXT_FACE.into())));
    }
}
