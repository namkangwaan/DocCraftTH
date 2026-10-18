//! Empty-glyph Type 3 fonts with a `ToUnicode` CMap (ISO 32000-2 §9.6.4 and §9.10.3) for an
//! invisible OCR text layer whose characters lie outside WinAnsiEncoding.

use pdfcraft_cos::{Dict, Document, ObjRef, Object, Stream};

/// Combining marks that sit on the preceding base character and advance the pen by 0.
/// Kept in sync with `pdfcraft_ocr::is_zero_width`.
pub(crate) fn is_zero_width(c: char) -> bool {
    matches!(u32::from(c), 0x0300..=0x036F | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E)
}

fn unicode_hex(ch: char) -> String {
    let mut units = [0u16; 2];
    ch.encode_utf16(&mut units).iter().map(|u| format!("{u:04X}")).collect()
}

/// Add a 1-byte Type 3 font whose codes `1..=chars.len()` paint nothing and map to `chars`
/// through `/ToUnicode`.
pub(crate) fn add_invisible_font(doc: &mut Document, chars: &[char]) -> ObjRef {
    let count = chars.len().min(255);
    let slice = &chars[..count];

    let base_proc = doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"500 0 d0\n".to_vec())));
    let mut zero_proc: Option<ObjRef> = None;

    let mut charprocs = Dict::new();
    let mut widths = Vec::with_capacity(slice.len());
    let mut differences = vec![Object::Int(1)];
    for (idx, &ch) in slice.iter().enumerate() {
        let code = idx.saturating_add(1);
        let glyph_name = format!("g{code}");
        let zero = is_zero_width(ch);
        let proc_ref = if zero {
            *zero_proc.get_or_insert_with(|| doc.add(Object::Stream(Stream::from_raw(Dict::new(), b"0 0 d0\n".to_vec()))))
        } else {
            base_proc
        };
        charprocs.set(glyph_name.as_bytes().to_vec(), Object::Ref(proc_ref));
        differences.push(Object::name(&glyph_name));
        widths.push(Object::Int(if zero { 0 } else { 500 }));
    }

    let mut cmap = String::from(
        "/CIDInit /ProcSet findresource begin\n12 dict begin\nbegincmap\n/CMapType 2 def\n1 begincodespacerange\n<01> <FF>\nendcodespacerange\n",
    );
    for (block_idx, chunk) in slice.chunks(100).enumerate() {
        cmap.push_str(&format!("{} beginbfchar\n", chunk.len()));
        for (offset, &ch) in chunk.iter().enumerate() {
            let code = block_idx.saturating_mul(100).saturating_add(offset).saturating_add(1);
            cmap.push_str(&format!("<{code:02X}> <{}>\n", unicode_hex(ch)));
        }
        cmap.push_str("endbfchar\n");
    }
    cmap.push_str("endcmap\nCMapName currentdict /CMap defineresource pop\nend\nend\n");
    let cmap_ref = doc.add(Object::Stream(Stream::flate(Dict::new(), cmap.as_bytes())));

    let mut encoding = Dict::new();
    encoding.set(b"Type".to_vec(), Object::name("Encoding"));
    encoding.set(b"Differences".to_vec(), Object::Array(differences));

    let mut font = Dict::new();
    font.set(b"Type".to_vec(), Object::name("Font"));
    font.set(b"Subtype".to_vec(), Object::name("Type3"));
    font.set(b"FontBBox".to_vec(), Object::Array(vec![Object::Int(0), Object::Int(-210), Object::Int(500), Object::Int(720)]));
    font.set(
        b"FontMatrix".to_vec(),
        Object::Array(vec![Object::Real(0.001), Object::Int(0), Object::Int(0), Object::Real(0.001), Object::Int(0), Object::Int(0)]),
    );
    font.set(b"FirstChar".to_vec(), Object::Int(1));
    font.set(b"LastChar".to_vec(), Object::Int(slice.len() as i64));
    font.set(b"Widths".to_vec(), Object::Array(widths));
    font.set(b"Encoding".to_vec(), Object::Dict(encoding));
    font.set(b"CharProcs".to_vec(), Object::Dict(charprocs));
    font.set(b"Resources".to_vec(), Object::Dict(Dict::new()));
    font.set(b"ToUnicode".to_vec(), Object::Ref(cmap_ref));
    doc.add(Object::Dict(font))
}
