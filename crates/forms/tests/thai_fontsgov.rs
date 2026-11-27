use pdfcraft_cos::{Document, Object};
use pdfcraft_fonts::thai_faces;
use pdfcraft_forms::{BorderStyle, FieldFont, FieldProps, Look, NewField, add_field, appearance, fields, set_props, set_value};

fn one_page_doc() -> Document {
    let mut doc = Document::new_empty();
    let page_ref = doc.add(Object::Dict(Default::default()));
    let mut pages = pdfcraft_cos::Dict::new();
    pages.set(b"Type".to_vec(), Object::name("Pages"));
    pages.set(b"Kids".to_vec(), Object::Array(vec![Object::Ref(page_ref)]));
    pages.set(b"Count".to_vec(), Object::Int(1));
    let pages_ref = doc.add(Object::Dict(pages));

    let mut page = pdfcraft_cos::Dict::new();
    page.set(b"Type".to_vec(), Object::name("Page"));
    page.set(b"Parent".to_vec(), Object::Ref(pages_ref));
    page.set(b"MediaBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(0), Object::Int(600), Object::Int(800)]));
    doc.set(page_ref, Object::Dict(page));

    let mut catalog = pdfcraft_cos::Dict::new();
    catalog.set(b"Type".to_vec(), Object::name("Catalog"));
    catalog.set(b"Pages".to_vec(), Object::Ref(pages_ref));
    let cat_ref = doc.add(Object::Dict(catalog));
    doc.trailer_mut().set(b"Root".to_vec(), Object::Ref(cat_ref));
    doc
}

fn strip_gpos_table(bytes: &[u8]) -> Vec<u8> {
    let mut out = bytes.to_vec();
    let count = u16::from_be_bytes([out[4], out[5]]) as usize;
    let tables: Vec<_> = out[12..12 + count * 16].as_chunks::<16>().0.iter().filter(|t| &t[..4] != b"GPOS").map(|t| t.to_vec()).collect();
    out[4..6].copy_from_slice(&(tables.len() as u16).to_be_bytes());
    for (i, table) in tables.iter().enumerate() {
        out[12 + i * 16..28 + i * 16].copy_from_slice(table);
    }
    out
}

fn widget_ap_text(doc: &Document, field_name: &str) -> String {
    let f = fields(doc).into_iter().find(|f| f.name == field_name).expect("field exists");
    let w = &f.widgets[0];
    let wobj = doc.get(w.obj);
    let wd = wobj.as_dict().expect("widget dict");
    let ap = doc.resolve(wd.get(b"AP").expect("AP")).as_dict().cloned().expect("AP dict");
    let n = ap.get(b"N").expect("AP /N");
    match doc.resolve(n).as_ref() {
        Object::Stream(s) => String::from_utf8_lossy(&s.decoded_within(1 << 20).expect("decode AP")).into_owned(),
        other => panic!("unexpected AP /N: {other:?}"),
    }
}

#[test]
fn thai_form_appearance_renders_gpos_false_fontsgov_without_error() {
    let Some(thsarabun) = thai_faces::by_family("THSarabun") else {
        eprintln!("skipping thai_form_appearance_renders_gpos_false_fontsgov_without_error: built without craft-fonts");
        return;
    };
    let gpos_false = strip_gpos_table(thai_faces::bytes(thsarabun));
    let face = thai_faces::register(gpos_false).expect("SIPA/NECTEC GPOS=false font registers");

    let mut doc = one_page_doc();
    add_field(&mut doc, 0, [20.0, 20.0, 220.0, 48.0], &NewField::Text { multiline: false }, Some("name")).unwrap();
    let props = FieldProps {
        look: Some(Look { border: None, fill: None, width: 1.0, style: BorderStyle::Solid, text: [0.0; 3], font: FieldFont::Thai(face) }),
        ..Default::default()
    };
    set_props(&mut doc, "name", &props).unwrap();
    set_value(&mut doc, "name", &pdfcraft_forms::FieldValue::Text("กิ่ น้ำใจ ๑๒๓".into())).unwrap();

    let ap = widget_ap_text(&doc, "name");
    assert!(!ap.contains('?'), "GPOS=false FontsGOV must draw Thai without '?': {ap}");

    let f = fields(&doc).into_iter().find(|f| f.name == "name").unwrap();
    let res = appearance::parse_da(&f.da).font;
    let af = pdfcraft_forms::acroform(&doc).expect("AcroForm");
    let dr = doc.resolve(af.get(b"DR").expect("DR")).as_dict().cloned().expect("DR dict");
    let fonts = doc.resolve(dr.get(b"Font").expect("Font")).as_dict().cloned().expect("Font dict");
    let entry = fonts.get(res.as_bytes()).expect("DA font in /DR /Font");
    let font_dict = doc.resolve(entry).as_dict().cloned().expect("font dict");
    assert_eq!(font_dict.name(b"Subtype"), Some(&b"Type0"[..]));
}
