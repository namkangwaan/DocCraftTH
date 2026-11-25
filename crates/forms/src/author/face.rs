//! Which face a field's default appearance names: one of the standard fonts, a Thai face the app
//! knows, or something else that must be kept as it is.

use pdfcraft_fonts::embedding;
use pdfcraft_fonts::thai_faces::{self, FaceId};

use super::{Document, Field, FieldKind, FormError, Object, appearance, ensure_form, fields, invalid, tree};

/// The face a field's `/DA` font refers to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum FieldFace {
    /// Helvetica, Times or Courier.
    Standard,
    /// An embedded Thai face this process knows (bundled, or registered from a font folder).
    Thai(FaceId),
    /// Any other font, by the name the form gives it: kept as written, never shown as Helvetica.
    Unknown(String),
}

const STANDARD: [&str; 6] = ["Helv", "TiRo", "Cour", "Helvetica", "Times-Roman", "Courier"];

/// The face the field's first widget's `/DA` (else the field's) draws with, read through the form's
/// `/DR /Font` entry for its resource.
pub fn field_face(doc: &Document, f: &Field) -> FieldFace {
    let wd = f.widgets.first().and_then(|w| doc.get(w.obj).as_dict().cloned()).unwrap_or_default();
    let da = wd.get(b"DA").and_then(|o| doc.resolve(o).as_string().map(|s| s.to_text())).unwrap_or_else(|| f.da.clone());
    let resource = appearance::parse_da(&da).font;
    if STANDARD.contains(&resource.as_str()) {
        return FieldFace::Standard;
    }
    match dr_base_font(doc, &resource) {
        Some(base) => match thai_faces::by_family(&base) {
            Some(id) => FieldFace::Thai(id),
            None if STANDARD.contains(&base.as_str()) => FieldFace::Standard,
            None => FieldFace::Unknown(base),
        },
        None => FieldFace::Unknown(resource),
    }
}

/// The `/BaseFont` of the form's `/DR /Font` entry `resource`, when it has one.
fn dr_base_font(doc: &Document, resource: &str) -> Option<String> {
    crate::acroform(doc)
        .and_then(|af| af.get(b"DR").and_then(|o| doc.resolve(o).as_dict().cloned()))
        .and_then(|dr| dr.get(b"Font").and_then(|o| doc.resolve(o).as_dict().cloned()))
        .and_then(|fonts| fonts.get(resource.as_bytes()).and_then(|o| doc.resolve(o).as_dict().cloned()))
        .and_then(|font| font.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned()))
}

/// The shaping script of the Thai face the `/DA` font `resource` names, or the default Thai face
/// when it names none: the face an appearance draws Thai runs with.
pub(crate) fn thai_script_of(doc: &Document, resource: &str) -> pdfcraft_fonts::ShapedScript {
    dr_base_font(doc, resource).and_then(|base| thai_faces::by_family(&base)).map_or(pdfcraft_fonts::ShapedScript::Thai, thai_faces::script)
}

/// The `/DR /Font` resource the form holds `face` under, embedding the whole font there the first
/// time (once per document, however many fields use it; as an object, which Acrobat needs). A viewer
/// that edits the field then has the font even where it is not installed.
///
/// A form a signature covers is refused: its resources must stay as the signer saw them.
pub(super) fn ensure_thai_font(doc: &mut Document, face: FaceId) -> Result<String, FormError> {
    if fields(doc).iter().any(|f| f.kind == FieldKind::Signature && tree::holds_value(doc, f.obj)) {
        return invalid("this form is signed: changing a field's font would change what was signed");
    }
    let form = ensure_form(doc)?;
    let mut af = doc.get(form).as_dict().cloned().unwrap_or_default();
    let (dr, mut fonts) = tree::form_fonts(doc, &af);
    let resource = embedding::embed(doc, &mut fonts, thai_faces::bytes(face), thai_faces::family(face)).map_err(FormError::Invalid)?;
    tree::store_form_fonts(doc, &mut af, dr, fonts);
    doc.set(form, Object::Dict(af));
    Ok(resource)
}
