//! Thai OCR real-document evaluation harness (Set A recognizer-only, Set A end-to-end, Set B real scan lines).
//! Skips cleanly when `THAI_TESTSET_DIR` is unset. Never modifies `assets/models/`.

use pdfcraft_ocr::metrics::cer;
use pdfcraft_ocr::{Line, Models, Ocr, Word};
use std::fs::File;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};

const SENTENCES_5: &[&str] =
    &["รายงานประจำปี ๒๕๖๙", "ใบแจ้งหนี้ No. 2569/001", "น้ำใจ ปฏิบัติ ฤๅษี ฎีกา", "กรุณาลงนามและส่งคืนภายในวันที่ 30 กันยายน", "ข้อมูลส่วนบุคคลได้รับการคุ้มครองตามกฎหมาย"];

#[derive(Clone, Debug)]
struct ExpectedLine {
    rect: [f32; 4],
    text: String,
}

/// Match ground-truth lines against recognized lines by vertical overlap / vertical proximity
/// so reading-order differences on multi-column headers or form blocks do not inflate page CER.
fn page_cer_by_line_matching(expected: &[ExpectedLine], recognized: &[Line]) -> f64 {
    if expected.is_empty() {
        return 0.0;
    }
    let mut used = vec![false; recognized.len()];
    let mut total_weighted_cer = 0.0_f64;
    let mut total_chars = 0usize;

    for exp in expected {
        let exp_chars = exp.text.chars().count();
        if exp_chars == 0 {
            continue;
        }
        let exp_mid_y = (exp.rect[1] + exp.rect[3]) * 0.5;
        let exp_h = (exp.rect[3] - exp.rect[1]).max(1.0);

        let mut best_idx: Option<usize> = None;
        let mut best_score = f32::INFINITY;

        for (idx, got) in recognized.iter().enumerate() {
            if used[idx] || got.words.is_empty() {
                continue;
            }
            let got_top = got.words.iter().map(|w| w.rect[1]).fold(f32::INFINITY, f32::min);
            let got_bot = got.words.iter().map(|w| w.rect[3]).fold(f32::NEG_INFINITY, f32::max);
            let got_left = got.words.iter().map(|w| w.rect[0]).fold(f32::INFINITY, f32::min);
            let got_mid_y = (got_top + got_bot) * 0.5;
            let dy = (exp_mid_y - got_mid_y).abs();
            if dy <= exp_h * 1.2 {
                let dx = (exp.rect[0] - got_left).abs() * 0.1;
                let score = dy + dx;
                if score < best_score {
                    best_score = score;
                    best_idx = Some(idx);
                }
            }
        }

        let got_text = match best_idx {
            Some(i) => {
                used[i] = true;
                recognized[i].text()
            }
            None => String::new(),
        };
        let line_cer = cer(&exp.text, &got_text);
        total_weighted_cer += line_cer * (exp_chars as f64);
        total_chars += exp_chars;
    }

    if total_chars == 0 { 0.0 } else { total_weighted_cer / (total_chars as f64) }
}

#[test]
fn line_matching_cer_is_invariant_to_line_order() {
    let expected = vec![
        ExpectedLine { rect: [80.0, 100.0, 300.0, 130.0], text: "ที่ ศธ ๐๒๐๖.๖/ว ๑๗".into() },
        ExpectedLine {
            rect: [80.0, 200.0, 400.0, 230.0], text: "เรื่อง หลักเกณฑ์และวิธีการ".into()
        },
    ];
    // Recognized in reversed order
    let recognized = vec![
        Line {
            words: vec![Word {
                text: "เรื่อง หลักเกณฑ์และวิธีการ".into(), rect: [82.0, 202.0, 398.0, 228.0]
            }],
        },
        Line { words: vec![Word { text: "ที่ ศธ ๐๒๐๖.๖/ว ๑๗".into(), rect: [81.0, 101.0, 299.0, 129.0] }] },
    ];
    let rate = page_cer_by_line_matching(&expected, &recognized);
    assert!(rate < 1e-6, "expected 0.0 CER regardless of line order, got {rate}");
}

fn eval_synthetic_5(ocr: &Ocr) -> f64 {
    use pdfcraft_engine::{AddedText, Edit, Session};
    use pdfcraft_render::{PageRenderer, RenderRequest};

    if pdfcraft_fonts::document_thai_font().is_none() {
        return 0.0;
    }

    let mut rates = Vec::new();
    for sentence in SENTENCES_5 {
        let mut session = Session::new();
        let bytes = session.create_blank(612.0, 792.0, 1).expect("blank page");
        let id = session.open("thai-ocr.pdf", None, bytes, None).expect("open blank page");
        let text = AddedText { rect: [72.0, 640.0, 540.0, 760.0], text: sentence.to_string(), size: 16.0, ..Default::default() };
        session.apply(id, Edit::AddText { page: 0, text }).expect("add text");
        let doc = session.get(id).expect("document");
        let shot =
            PageRenderer::new(doc.bytes.clone(), Default::default()).render(RenderRequest { page: 0, scale: 300.0_f32 / 72.0, ..Default::default() });
        assert!(shot.error.is_none(), "{:?}", shot.error);
        let lines = ocr.recognize(&shot.rgba, shot.width, shot.height).expect("recognize");
        let got = lines.iter().map(|line| line.text()).collect::<Vec<_>>().join(" ");
        rates.push(cer(sentence, &got));
    }
    rates.iter().sum::<f64>() / rates.len() as f64
}

#[test]
fn compare_models_on_real_corpus() {
    let Some(testset_dir_str) = std::env::var_os("THAI_TESTSET_DIR") else {
        eprintln!("skipping Thai real docs test: THAI_TESTSET_DIR is not set");
        return;
    };
    let testset_dir = PathBuf::from(testset_dir_str);
    if !testset_dir.is_dir() {
        eprintln!("skipping Thai real docs test: {} is not a directory", testset_dir.display());
        return;
    }

    let assets_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
    let baseline_models = Models::in_dir_for(&assets_dir, "th").expect("baseline Thai models in assets/models");
    let baseline_ocr = Ocr::load(&baseline_models).expect("load baseline OCR");

    let candidate_dir =
        std::env::var_os("THAI_CANDIDATE_MODELS_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from(r"K:\src\DocCraftTH\models\finetuned"));

    let candidate_models = Models::in_dir_for(&candidate_dir, "th").expect("candidate Thai models in candidate_dir");
    let candidate_ocr = Ocr::load(&candidate_models).expect("load candidate OCR");

    // 1. Evaluate synthetic 5 sentences
    eprintln!("=== 1. Synthetic 5 Sentences ===");
    let syn5_base = eval_synthetic_5(&baseline_ocr);
    let syn5_cand = eval_synthetic_5(&candidate_ocr);
    eprintln!("Baseline Synthetic 5 CER:   {syn5_base:.4}");
    eprintln!("Candidate Synthetic 5 CER:  {syn5_cand:.4}");

    // 2. Evaluate Set A Recognizer-only
    let labels_tsv = testset_dir.join("set_a/labels.tsv");
    let mut rec_only_base_rates = Vec::new();
    let mut rec_only_cand_rates = Vec::new();

    if labels_tsv.is_file() {
        eprintln!("=== 2. Set A Recognizer-Only ===");
        let file = File::open(&labels_tsv).expect("open labels.tsv");
        let reader = BufReader::new(file);
        let mut lines = reader.lines();
        let _header = lines.next();

        let mut count = 0;
        for line_res in lines {
            let line_str = line_res.expect("read line");
            let cols: Vec<&str> = line_str.split('\t').collect();
            if cols.len() < 11 {
                continue;
            }
            let ground_truth = cols[8];
            let crop_rel = cols[10];
            let crop_path = testset_dir.join("set_a").join(crop_rel);
            if !crop_path.is_file() {
                continue;
            }

            let Ok(dyn_img) = image::open(&crop_path) else { continue };
            let rgb = dyn_img.to_rgb8();
            let (w, h) = (rgb.width(), rgb.height());
            let px = rgb.into_raw();

            let base_got = baseline_ocr.recognize_crop(&px, w, h, [0, 0, w, h]).ok().flatten().unwrap_or_default();
            let cand_got = candidate_ocr.recognize_crop(&px, w, h, [0, 0, w, h]).ok().flatten().unwrap_or_default();

            rec_only_base_rates.push(cer(ground_truth, &base_got));
            rec_only_cand_rates.push(cer(ground_truth, &cand_got));
            count += 1;
        }

        let base_mean = rec_only_base_rates.iter().sum::<f64>() / rec_only_base_rates.len().max(1) as f64;
        let cand_mean = rec_only_cand_rates.iter().sum::<f64>() / rec_only_cand_rates.len().max(1) as f64;
        eprintln!("Set A Rec-Only (evaluated {count} lines):");
        eprintln!("  Baseline CER:   {base_mean:.4}");
        eprintln!("  Candidate CER:  {cand_mean:.4}");
    }

    // 3. Evaluate Set A End-to-End
    let pages_dir = testset_dir.join("set_a/pages");
    let mut e2e_base_page_rates = Vec::new();
    let mut e2e_cand_page_rates = Vec::new();

    if pages_dir.is_dir() {
        eprintln!("=== 3. Set A End-to-End ===");
        if let Ok(entries) = std::fs::read_dir(&pages_dir) {
            for entry in entries.flatten() {
                let p = entry.path();
                let file_name = p.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if file_name.ends_with(".lines.tsv") {
                    let page_png_name = file_name.replace(".lines.tsv", ".png");
                    let img_path = pages_dir.join(&page_png_name);
                    if !img_path.is_file() {
                        continue;
                    }

                    let Ok(lines_file) = File::open(&p) else { continue };
                    let mut reader_lines = BufReader::new(lines_file).lines();
                    let _tsv_hdr = reader_lines.next();

                    let mut expected_lines = Vec::new();
                    for line_str in reader_lines.map_while(Result::ok) {
                        let parts: Vec<&str> = line_str.split('\t').collect();
                        if parts.len() >= 5 {
                            let x0 = parts[0].parse::<f32>().unwrap_or(0.0);
                            let y0 = parts[1].parse::<f32>().unwrap_or(0.0);
                            let x1 = parts[2].parse::<f32>().unwrap_or(0.0);
                            let y1 = parts[3].parse::<f32>().unwrap_or(0.0);
                            let text = parts[4].to_string();
                            expected_lines.push(ExpectedLine { rect: [x0, y0, x1, y1], text });
                        }
                    }

                    if expected_lines.is_empty() {
                        continue;
                    }

                    let Ok(dyn_img) = image::open(&img_path) else { continue };
                    let rgb = dyn_img.to_rgb8();
                    let (w, h) = (rgb.width(), rgb.height());
                    let px = rgb.into_raw();

                    let base_lines = baseline_ocr.recognize(&px, w, h).unwrap_or_default();
                    let cand_lines = candidate_ocr.recognize(&px, w, h).unwrap_or_default();

                    let base_cer = page_cer_by_line_matching(&expected_lines, &base_lines);
                    let cand_cer = page_cer_by_line_matching(&expected_lines, &cand_lines);

                    e2e_base_page_rates.push(base_cer);
                    e2e_cand_page_rates.push(cand_cer);
                }
            }
        }
        let base_mean = e2e_base_page_rates.iter().sum::<f64>() / e2e_base_page_rates.len().max(1) as f64;
        let cand_mean = e2e_cand_page_rates.iter().sum::<f64>() / e2e_cand_page_rates.len().max(1) as f64;
        eprintln!("Set A End-to-End (evaluated {} pages):", e2e_base_page_rates.len());
        eprintln!("  Baseline CER:   {base_mean:.4}");
        eprintln!("  Candidate CER:  {cand_mean:.4}");
    }

    // 4. Print Decision Gate Summary
    eprintln!("\n================ DECISION GATE EVALUATION ================");
    let rec_base_mean = rec_only_base_rates.iter().sum::<f64>() / rec_only_base_rates.len().max(1) as f64;
    let rec_cand_mean = rec_only_cand_rates.iter().sum::<f64>() / rec_only_cand_rates.len().max(1) as f64;
    let e2e_base_mean = e2e_base_page_rates.iter().sum::<f64>() / e2e_base_page_rates.len().max(1) as f64;
    let e2e_cand_mean = e2e_cand_page_rates.iter().sum::<f64>() / e2e_cand_page_rates.len().max(1) as f64;

    eprintln!("Criterion 1: Synthetic 5 CER <= 0.0136? Candidate = {syn5_cand:.4} -> {}", if syn5_cand <= 0.0136 + 1e-6 { "PASS" } else { "FAIL" });
    eprintln!(
        "Criterion 2: Set A Rec-Only Candidate <= Baseline? ({rec_cand_mean:.4} vs {rec_base_mean:.4}) -> {}",
        if rec_cand_mean <= rec_base_mean + 1e-6 { "PASS" } else { "FAIL" }
    );
    eprintln!(
        "Criterion 3: Set A E2E Candidate <= Baseline? ({e2e_cand_mean:.4} vs {e2e_base_mean:.4}) -> {}",
        if e2e_cand_mean <= e2e_base_mean + 1e-6 { "PASS" } else { "FAIL" }
    );

    let promoted = (syn5_cand <= 0.0136 + 1e-6) && (rec_cand_mean <= rec_base_mean + 1e-6) && (e2e_cand_mean <= e2e_base_mean + 1e-6);
    eprintln!("FINAL DECISION: {}", if promoted { "PROMOTE CANDIDATE" } else { "REJECT CANDIDATE (KEEP BASELINE)" });
    eprintln!("==========================================================\n");
}

#[test]
fn draft_set_b_candidates() {
    let Some(testset_dir_str) = std::env::var_os("THAI_TESTSET_DIR") else {
        eprintln!("skipping draft_set_b_candidates: THAI_TESTSET_DIR is not set");
        return;
    };
    let testset_dir = PathBuf::from(testset_dir_str);
    let candidates_tsv = testset_dir.join("real-lines/candidates.tsv");
    if !candidates_tsv.is_file() {
        eprintln!("skipping draft_set_b_candidates: candidates.tsv not found");
        return;
    }

    let assets_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
    let baseline_models = Models::in_dir_for(&assets_dir, "th").expect("baseline Thai models");
    let baseline_ocr = Ocr::load(&baseline_models).expect("load baseline OCR");

    let file = File::open(&candidates_tsv).expect("open candidates.tsv");
    let reader = BufReader::new(file);
    let mut lines = reader.lines().map_while(Result::ok);
    let header = lines.next().expect("header");

    let mut updated_rows = vec![header];
    let crops_dir = testset_dir.join("real-lines");

    for line_str in lines {
        let mut cols: Vec<String> = line_str.split('\t').map(|s| s.to_string()).collect();
        if cols.len() >= 11 {
            let crop_rel = &cols[10];
            let crop_path = crops_dir.join(crop_rel);
            if crop_path.is_file() {
                let Ok(dyn_img) = image::open(&crop_path) else { continue };
                let rgb = dyn_img.to_rgb8();
                let (w, h) = (rgb.width(), rgb.height());
                let px = rgb.into_raw();
                let text = baseline_ocr.recognize_crop(&px, w, h, [0, 0, w, h]).ok().flatten().unwrap_or_default();
                cols[7] = text.trim().to_string();
            }
        }
        updated_rows.push(cols.join("\t"));
    }

    std::fs::write(&candidates_tsv, updated_rows.join("\n")).expect("write candidates.tsv");
    eprintln!("Drafted text for {} lines in candidates.tsv", updated_rows.len().saturating_sub(1));
}

#[test]
fn test_eval_set_b_real_scan_lines() {
    let Some(testset_dir_str) = std::env::var_os("THAI_TESTSET_DIR") else {
        eprintln!("skipping test_eval_set_b_real_scan_lines: THAI_TESTSET_DIR is not set");
        return;
    };
    let testset_dir = PathBuf::from(testset_dir_str);
    let labels_tsv = testset_dir.join("real-lines/labels.tsv");
    if !labels_tsv.is_file() {
        eprintln!("skipping test_eval_set_b_real_scan_lines: labels.tsv not found");
        return;
    }

    let assets_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models");
    let baseline_models = Models::in_dir_for(&assets_dir, "th").expect("baseline Thai models");
    let baseline_ocr = Ocr::load(&baseline_models).expect("load baseline OCR");

    let candidate_dir = Path::new(r"K:\src\DocCraftTH\models\finetuned");
    let candidate_models = Models::in_dir_for(candidate_dir, "th").expect("candidate Thai models");
    let candidate_ocr = Ocr::load(&candidate_models).expect("load candidate OCR");

    let file = File::open(&labels_tsv).expect("open labels.tsv");
    let reader = BufReader::new(file);
    let mut lines = reader.lines().map_while(Result::ok);
    let _header = lines.next().expect("header");

    let mut base_rates = Vec::new();
    let mut cand_rates = Vec::new();
    let crops_base = testset_dir.join("real-lines");

    let mut count = 0;
    for line_str in lines {
        let cols: Vec<&str> = line_str.split('\t').collect();
        if cols.len() < 3 {
            continue;
        }
        let crop_rel = cols[1];
        let ground_truth = cols[2];
        let crop_path = crops_base.join(crop_rel);
        if !crop_path.is_file() {
            continue;
        }

        let Ok(dyn_img) = image::open(&crop_path) else { continue };
        let rgb = dyn_img.to_rgb8();
        let (w, h) = (rgb.width(), rgb.height());
        let px = rgb.into_raw();

        let base_got = baseline_ocr.recognize_crop(&px, w, h, [0, 0, w, h]).ok().flatten().unwrap_or_default();
        let cand_got = candidate_ocr.recognize_crop(&px, w, h, [0, 0, w, h]).ok().flatten().unwrap_or_default();

        let b_cer = cer(ground_truth, &base_got);
        let c_cer = cer(ground_truth, &cand_got);
        base_rates.push(b_cer);
        cand_rates.push(c_cer);
        count += 1;
    }

    let base_mean = base_rates.iter().sum::<f64>() / base_rates.len().max(1) as f64;
    let cand_mean = cand_rates.iter().sum::<f64>() / cand_rates.len().max(1) as f64;

    eprintln!("\n=== Set B Real-Scan Lines (evaluated {} human-verified lines) ===", count);
    eprintln!("  Baseline CER:   {base_mean:.4}");
    eprintln!("  Candidate CER:  {cand_mean:.4}");
    eprintln!("  CER Delta:      {:+.4}", cand_mean - base_mean);
    eprintln!("===================================================================\n");
}
