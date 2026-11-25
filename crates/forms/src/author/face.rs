//! Which face a field's default appearance names: one of the standard fonts, a Thai face the app
//! knows, or something else that must be kept as it is.

use pdfcraft_fonts::thai_faces::{self, FaceId};

use super::{Document, Field, appearance};

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
    let base = crate::acroform(doc)
        .and_then(|af| af.get(b"DR").and_then(|o| doc.resolve(o).as_dict().cloned()))
        .and_then(|dr| dr.get(b"Font").and_then(|o| doc.resolve(o).as_dict().cloned()))
        .and_then(|fonts| fonts.get(resource.as_bytes()).and_then(|o| doc.resolve(o).as_dict().cloned()))
        .and_then(|font| font.name(b"BaseFont").map(|n| String::from_utf8_lossy(n).into_owned()));
    match base {
        Some(base) => match thai_faces::by_family(&base) {
            Some(id) => FieldFace::Thai(id),
            None if STANDARD.contains(&base.as_str()) => FieldFace::Standard,
            None => FieldFace::Unknown(base),
        },
        None => FieldFace::Unknown(resource),
    }
}
