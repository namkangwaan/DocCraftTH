//! The Thai faces a form field can be set to: the bundled Thai faces and up to sixteen openly
//! licensed faces registered at run time (found in a font folder). A [`FaceId`] is a small `Copy`
//! handle, so field looks that hold one stay `Copy`.
//!
//! A face is registered only when it passes the embedding checks ([`embedding::validate_document_input`]
//! and [`embedding::validate`]) and has a Thai glyph. A face that fails them is never registered,
//! so nothing here can put a restricted font into a PDF.

use std::sync::{Mutex, OnceLock, PoisonError};

use harfrust::ShaperData;
use skrifa::raw::TableProvider;
use skrifa::{FontRef, MetadataProvider};

use crate::{ShapedScript, embedding};

/// A Thai face: a bundled one or a registered one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaceId(u16);

/// Registered faces beyond the bundled ones, so a folder of fonts cannot grow memory without bound.
const MAX_CUSTOM: usize = 64;
/// Ids 0..=5 are bundled Thai faces (Noto Sans Thai, Sarabun, and THSarabun's 4 styles); registered faces follow.
const PRIMARY: u16 = 0;
const SARABUN: u16 = 1;
const TH_SARABUN_REGULAR: u16 = 2;
const TH_SARABUN_BOLD: u16 = 3;
const TH_SARABUN_ITALIC: u16 = 4;
const TH_SARABUN_BOLD_ITALIC: u16 = 5;
const FIRST_CUSTOM: u16 = 6;
const BUNDLED_CUSTOM_COUNT: usize = 4;

type Shaper = (FontRef<'static>, ShaperData);

struct Custom {
    family: &'static str,
    bold: bool,
    italic: bool,
    bytes: &'static [u8],
    shaper: &'static Shaper,
}

/// Registered faces. The bytes are kept for the life of the process (at most [`MAX_CUSTOM`]).
static CUSTOM: Mutex<Vec<Custom>> = Mutex::new(Vec::new());

fn custom<T>(f: impl FnOnce(&[Custom]) -> T) -> T {
    f(&CUSTOM.lock().unwrap_or_else(PoisonError::into_inner))
}

fn exact_thai_face(family: &str, style: &str) -> Option<&'static crate::EmbeddedFace> {
    crate::ui_thai_fonts()
        .into_iter()
        .find(|f| f.family.eq_ignore_ascii_case(family) && f.style.eq_ignore_ascii_case(style))
}

fn bundled_face(id: FaceId) -> Option<&'static crate::EmbeddedFace> {
    match id.0 {
        PRIMARY => ShapedScript::Thai.font(),
        SARABUN => ShapedScript::ThaiSarabun.font(),
        TH_SARABUN_REGULAR => exact_thai_face("THSarabun", "Regular"),
        TH_SARABUN_BOLD => exact_thai_face("THSarabun", "Bold"),
        TH_SARABUN_ITALIC => exact_thai_face("THSarabun", "Italic"),
        TH_SARABUN_BOLD_ITALIC => exact_thai_face("THSarabun", "BoldItalic"),
        _ => None,
    }
}

/// The bundled Thai faces a form field can embed (none without craft-fonts), THSarabun first. A bundled
/// face the embedding policy refuses (a variable font such as the bundled Noto Sans Thai) is not
/// listed: it still shapes Thai in appearances, but a form cannot carry it.
pub fn bundled() -> Vec<FaceId> {
    static EMBEDDABLE: OnceLock<Vec<FaceId>> = OnceLock::new();
    EMBEDDABLE
        .get_or_init(|| {
            [FaceId(TH_SARABUN_REGULAR), FaceId(SARABUN)]
                .into_iter()
                .filter(|id| bundled_face(*id).is_some_and(|f| embedding::validate(f.bytes).is_ok()))
                .collect()
        })
        .clone()
}

/// Sarabun, when this build bundles it.
pub fn sarabun() -> Option<FaceId> {
    bundled_face(FaceId(SARABUN)).map(|_| FaceId(SARABUN))
}

/// The shaping script whose glyph ids this face owns.
pub fn script(id: FaceId) -> ShapedScript {
    match id.0 {
        PRIMARY => ShapedScript::Thai,
        SARABUN => ShapedScript::ThaiSarabun,
        n => ShapedScript::Custom(u8::try_from(n.saturating_sub(TH_SARABUN_REGULAR)).unwrap_or(u8::MAX)),
    }
}

/// The family name (empty for an id this process does not know).
pub fn family(id: FaceId) -> &'static str {
    if let Some(f) = bundled_face(id) {
        return f.family;
    }
    custom_family_slot(id.0.saturating_sub(FIRST_CUSTOM)).unwrap_or("")
}

/// The font program (empty for an id this process does not know).
pub fn bytes(id: FaceId) -> &'static [u8] {
    if let Some(f) = bundled_face(id) {
        return f.bytes;
    }
    custom_bytes_slot(id.0.saturating_sub(FIRST_CUSTOM)).unwrap_or(&[])
}

/// The face named `name` (case and spacing insensitive), bundled first.
pub fn by_family(name: &str) -> Option<FaceId> {
    let key = embedding::face_key(name);
    bundled()
        .into_iter()
        .find(|id| embedding::face_key(family(*id)) == key)
        .or_else(|| {
            (key == "thsarabunpsk" && bundled_face(FaceId(TH_SARABUN_REGULAR)).is_some_and(|f| embedding::validate(f.bytes).is_ok()))
                .then_some(FaceId(TH_SARABUN_REGULAR))
        })
        .or_else(|| {
            custom(|all| {
                all.iter()
                    .position(|c| embedding::face_key(c.family) == key && !c.bold && !c.italic)
                    .or_else(|| all.iter().position(|c| embedding::face_key(c.family) == key))
                    .and_then(|i| u16::try_from(i).ok())
                    .map(|i| FaceId(i + FIRST_CUSTOM))
            })
        })
}

/// Resolve `bold` / `italic` style for `id` within the same family, falling back to `id` if the
/// requested style is not bundled or registered.
pub fn resolve_style(id: FaceId, bold: bool, italic: bool) -> FaceId {
    if matches!(id.0, TH_SARABUN_REGULAR..=TH_SARABUN_BOLD_ITALIC) {
        let target = match (bold, italic) {
            (true, true) => TH_SARABUN_BOLD_ITALIC,
            (true, false) => TH_SARABUN_BOLD,
            (false, true) => TH_SARABUN_ITALIC,
            (false, false) => TH_SARABUN_REGULAR,
        };
        if bundled_face(FaceId(target)).is_some_and(|f| embedding::validate(f.bytes).is_ok()) {
            return FaceId(target);
        }
        return id;
    }
    let fam = family(id);
    if fam.is_empty() {
        return id;
    }
    let key = embedding::face_key(fam);
    custom(|all| {
        all.iter()
            .position(|c| embedding::face_key(c.family) == key && c.bold == bold && c.italic == italic)
            .or_else(|| {
                (bold || italic)
                    .then(|| all.iter().position(|c| embedding::face_key(c.family) == key && !c.bold && !c.italic))
                    .flatten()
            })
            .and_then(|i| u16::try_from(i).ok())
            .map_or(id, |i| FaceId(i + FIRST_CUSTOM))
    })
}

fn face_style_flags(font: &FontRef<'_>) -> (bool, bool) {
    if let Ok(os2) = font.os2() {
        let fs = os2.fs_selection();
        return (fs.bits() & 0x0020 != 0, fs.bits() & 0x0001 != 0);
    }
    if let Ok(head) = font.head() {
        let mac = head.mac_style();
        return (mac.bits() & 0x0001 != 0, mac.bits() & 0x0002 != 0);
    }
    (false, false)
}

/// Whether `bytes` is a font a form can carry as a Thai face: it passes the embedding checks,
/// yields a usable ASCII `/BaseFont` name, and has Thai letters. Registers nothing; the error says why not.
pub fn check(bytes: &[u8]) -> Result<(), String> {
    embedding::validate_document_input(bytes)?;
    embedding::validate(bytes)?;
    let (family, _) = embedding::names(bytes)?;
    let ascii: String = family.chars().filter(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_')).collect();
    if ascii.is_empty() || ascii.len() > 128 {
        return Err("the font family name has no ASCII letters for /BaseFont".into());
    }
    let font = FontRef::new(bytes).map_err(|e| e.to_string())?;
    if font.charmap().map('ก').is_none() {
        return Err("this font has no Thai letters".into());
    }
    Ok(())
}

/// The `FAMILY_NAME` string `embedding` will use as `/BaseFont` when `bytes` is embedded.
pub fn family_of(bytes: &[u8]) -> Result<String, String> {
    embedding::names(bytes).map(|(family, _)| family)
}

/// Register a font file as a Thai face. Fails with the embedding policy's own message when the font
/// may not be embedded, or says why it is not a Thai face; the same bytes always give the same id.
pub fn register(bytes: Vec<u8>) -> Result<FaceId, String> {
    check(&bytes)?;
    for id in [FaceId(TH_SARABUN_REGULAR), FaceId(TH_SARABUN_BOLD), FaceId(TH_SARABUN_ITALIC), FaceId(TH_SARABUN_BOLD_ITALIC), FaceId(SARABUN)] {
        if bundled_face(id).is_some_and(|f| f.bytes == bytes.as_slice()) {
            return Ok(id);
        }
    }
    let (family, _) = embedding::names(&bytes)?;
    let mut all = CUSTOM.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(i) = all.iter().position(|c| c.bytes == bytes.as_slice()) {
        return u16::try_from(i).map(|i| FaceId(i + FIRST_CUSTOM)).map_err(|e| e.to_string());
    }
    if all.len() >= MAX_CUSTOM {
        return Err(format!("at most {MAX_CUSTOM} extra Thai fonts can be used at once"));
    }
    let bytes: &'static [u8] = Box::leak(bytes.into_boxed_slice());
    let font = FontRef::new(bytes).map_err(|e| e.to_string())?;
    let (bold, italic) = face_style_flags(&font);
    let data = ShaperData::new(&font);
    let shaper: &'static Shaper = Box::leak(Box::new((font, data)));
    let family: &'static str = Box::leak(family.into_boxed_str());
    let id = FaceId(u16::try_from(all.len()).map_err(|e| e.to_string())? + FIRST_CUSTOM);
    all.push(Custom { family, bold, italic, bytes, shaper });
    Ok(id)
}

fn bundled_custom_id(slot: u16) -> Option<FaceId> {
    (usize::from(slot) < BUNDLED_CUSTOM_COUNT).then_some(FaceId(TH_SARABUN_REGULAR + slot))
}

fn custom_family_slot(slot: u16) -> Option<&'static str> {
    custom(|all| all.get(usize::from(slot)).map(|c| c.family))
}

fn custom_bytes_slot(slot: u16) -> Option<&'static [u8]> {
    custom(|all| all.get(usize::from(slot)).map(|c| c.bytes))
}

pub(crate) fn custom_family(slot: u16) -> Option<&'static str> {
    if let Some(id) = bundled_custom_id(slot) {
        return bundled_face(id).map(|f| f.family);
    }
    custom_family_slot(slot.saturating_sub(BUNDLED_CUSTOM_COUNT as u16))
}

pub(crate) fn custom_bytes(slot: u16) -> Option<&'static [u8]> {
    if let Some(id) = bundled_custom_id(slot) {
        return bundled_face(id).map(|f| f.bytes);
    }
    custom_bytes_slot(slot.saturating_sub(BUNDLED_CUSTOM_COUNT as u16))
}

pub(crate) fn custom_shaper(slot: u8) -> Option<&'static Shaper> {
    let slot_u16 = u16::from(slot);
    if let Some(id) = bundled_custom_id(slot_u16) {
        static SHAPERS: [OnceLock<Option<Shaper>>; BUNDLED_CUSTOM_COUNT] = [OnceLock::new(), OnceLock::new(), OnceLock::new(), OnceLock::new()];
        return SHAPERS[usize::from(slot_u16)]
            .get_or_init(|| {
                let face = bundled_face(id)?;
                let font = FontRef::new(face.bytes).ok()?;
                let data = ShaperData::new(&font);
                Some((font, data))
            })
            .as_ref();
    }
    custom(|all| all.get(usize::from(slot_u16.saturating_sub(BUNDLED_CUSTOM_COUNT as u16))).map(|c| c.shaper))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::thai_has_in;

    #[test]
    fn bundled_faces_have_thai_glyphs() {
        for id in bundled() {
            assert!(thai_has_in(script(id), 'ก'), "{} lacks ก", family(id));
        }
    }

    #[test]
    fn register_rejects_invalid_and_latin_only_faces() {
        assert!(register(vec![0; 16]).is_err());
        let inter = std::fs::read(concat!(env!("CARGO_MANIFEST_DIR"), "/../../assets/fonts/Inter-Regular.ttf")).unwrap_or_default();
        assert!(register(inter).is_err(), "a face without Thai glyphs is not a Thai face");
    }

    #[test]
    fn register_dedupes_identical_bytes() {
        let Some(id) = sarabun() else { return };
        let again = register(bytes(id).to_vec()).expect("Sarabun is openly licensed");
        assert_eq!(again, id, "bundled bytes resolve to the bundled id");
        assert_eq!(by_family("sarabun"), Some(id));
    }
}
