//! Thai OCR quality on synthetic scans: Thai sentences drawn by Add text, rendered at 300 dpi,
//! read back. Prints CER per sentence; fails when the mean is above the gate.

use pdfcraft_engine::{AddedText, Edit, Session};
use pdfcraft_render::{PageRenderer, RenderRequest};

const SENTENCES: &[&str] =
    &["รายงานประจำปี ๒๕๖๙", "ใบแจ้งหนี้ No. 2569/001", "น้ำใจ ปฏิบัติ ฤๅษี ฎีกา", "กรุณาลงนามและส่งคืนภายในวันที่ 30 กันยายน", "ข้อมูลส่วนบุคคลได้รับการคุ้มครองตามกฎหมาย"];
/// Mean CER allowed on clean 300 dpi synthetic pages (Task 10's decision gate).
const GATE: f64 = 0.05;

/// Task 11 replaces this with `pdfcraft_engine::ocr::available_for("th")`.
fn thai_models_ready() -> bool {
    false
}

#[test]
fn thai_synthetic_pages_meet_the_cer_gate() {
    if !thai_models_ready() {
        eprintln!("skipping Thai OCR harness: no Thai OCR model");
        return;
    }
    if pdfcraft_fonts::document_thai_font().is_none() {
        eprintln!("skipping Thai OCR harness: no Thai face");
        return;
    }
    let mut rates = Vec::new();
    for sentence in SENTENCES {
        let rate = cer_of_sentence(sentence);
        eprintln!("{rate:.4}\t{sentence}");
        rates.push(rate);
    }
    let mean = rates.iter().sum::<f64>() / rates.len() as f64;
    eprintln!("mean\t{mean:.4}");
    assert!(mean <= GATE, "mean CER {mean} is above the gate {GATE}");
}

fn cer_of_sentence(sentence: &str) -> f64 {
    let mut session = Session::new();
    let bytes = session.create_blank(612.0, 792.0, 1).expect("blank page");
    let id = session.open("thai-ocr.pdf", None, bytes, None).expect("open blank page");
    let text = AddedText { rect: [72.0, 640.0, 540.0, 760.0], text: sentence.to_string(), size: 16.0, ..Default::default() };
    session.apply(id, Edit::AddText { page: 0, text }).expect("add text");
    let doc = session.get(id).expect("document");
    let shot =
        PageRenderer::new(doc.bytes.clone(), Default::default()).render(RenderRequest { page: 0, scale: 300.0_f32 / 72.0, ..Default::default() });
    assert!(shot.error.is_none(), "{:?}", shot.error);
    let ocr = pdfcraft_ocr::Ocr::find().expect("recognition models");
    let lines = ocr.recognize(&shot.rgba, shot.width, shot.height).expect("recognize");
    let got = lines.iter().map(|line| line.text()).collect::<Vec<_>>().join(" ");
    pdfcraft_ocr::metrics::cer(sentence, &got)
}
