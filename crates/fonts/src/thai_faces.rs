//! The Thai faces a form field can be set to: the bundled Thai faces and up to sixteen openly
//! licensed faces registered at run time (found in a font folder). A [`FaceId`] is a small `Copy`
//! handle, so field looks that hold one stay `Copy`.
//!
//! A face is registered only when it passes the embedding checks ([`embedding::validate_document_input`]
//! and [`embedding::validate`]) and has a Thai glyph. A face that fails them is never registered,
//! so nothing here can put a restricted font into a PDF.

use std::sync::{Mutex, PoisonError};

use harfrust::ShaperData;
use skrifa::{FontRef, MetadataProvider};

use crate::{ShapedScript, embedding};

/// A Thai face: a bundled one or a registered one.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FaceId(u16);

/// Registered faces beyond the bundled ones, so a folder of fonts cannot grow memory without bound.
const MAX_CUSTOM: usize = 16;
/// Ids 0 and 1 are the bundled primary Thai face and Sarabun; registered faces follow.
const PRIMARY: u16 = 0;
const SARABUN: u16 = 1;
const FIRST_CUSTOM: u16 = 2;

type Shaper = (FontRef<'static>, ShaperData);

struct Custom {
    family: &'static str,
    bytes: &'static [u8],
    shaper: &'static Shaper,
}

/// Registered faces. The bytes are kept for the life of the process (at most [`MAX_CUSTOM`]).
static CUSTOM: Mutex<Vec<Custom>> = Mutex::new(Vec::new());

fn custom<T>(f: impl FnOnce(&[Custom]) -> T) -> T {
    f(&CUSTOM.lock().unwrap_or_else(PoisonError::into_inner))
}

fn bundled_face(id: FaceId) -> Option<&'static crate::EmbeddedFace> {
    match id.0 {
        PRIMARY => ShapedScript::Thai.font(),
        SARABUN => ShapedScript::ThaiSarabun.font(),
        _ => None,
    }
}

/// The bundled Thai faces a form field can embed (none without craft-fonts), Sarabun last. A bundled
/// face the embedding policy refuses (a variable font such as the bundled Noto Sans Thai) is not
/// listed: it still shapes Thai in appearances, but a form cannot carry it.
pub fn bundled() -> Vec<FaceId> {
    static EMBEDDABLE: std::sync::OnceLock<Vec<FaceId>> = std::sync::OnceLock::new();
    EMBEDDABLE
        .get_or_init(|| {
            [FaceId(PRIMARY), FaceId(SARABUN)]
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
        n => ShapedScript::Custom(u8::try_from(n.saturating_sub(FIRST_CUSTOM)).unwrap_or(u8::MAX)),
    }
}

/// The family name (empty for an id this process does not know).
pub fn family(id: FaceId) -> &'static str {
    if let Some(f) = bundled_face(id) {
        return f.family;
    }
    custom_family(id.0.saturating_sub(FIRST_CUSTOM)).unwrap_or("")
}

/// The font program (empty for an id this process does not know).
pub fn bytes(id: FaceId) -> &'static [u8] {
    if let Some(f) = bundled_face(id) {
        return f.bytes;
    }
    custom_bytes(id.0.saturating_sub(FIRST_CUSTOM)).unwrap_or(&[])
}

/// The face named `name` (case and spacing insensitive), bundled first.
pub fn by_family(name: &str) -> Option<FaceId> {
    let key = embedding::face_key(name);
    bundled().into_iter().find(|id| embedding::face_key(family(*id)) == key).or_else(|| {
        custom(|all| {
            all.iter().position(|c| embedding::face_key(c.family) == key).and_then(|i| u16::try_from(i).ok()).map(|i| FaceId(i + FIRST_CUSTOM))
        })
    })
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
    if let Some(id) = bundled().into_iter().find(|id| self::bytes(*id) == bytes.as_slice()) {
        return Ok(id);
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
    let data = ShaperData::new(&font);
    let shaper: &'static Shaper = Box::leak(Box::new((font, data)));
    let family: &'static str = Box::leak(family.into_boxed_str());
    let id = FaceId(u16::try_from(all.len()).map_err(|e| e.to_string())? + FIRST_CUSTOM);
    all.push(Custom { family, bytes, shaper });
    Ok(id)
}

pub(crate) fn custom_family(slot: u16) -> Option<&'static str> {
    custom(|all| all.get(usize::from(slot)).map(|c| c.family))
}

pub(crate) fn custom_bytes(slot: u16) -> Option<&'static [u8]> {
    custom(|all| all.get(usize::from(slot)).map(|c| c.bytes))
}

pub(crate) fn custom_shaper(slot: u8) -> Option<&'static Shaper> {
    custom(|all| all.get(usize::from(slot)).map(|c| c.shaper))
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
