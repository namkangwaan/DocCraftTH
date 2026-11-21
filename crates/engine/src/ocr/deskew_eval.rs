//! Opt-in evaluation of auto-deskew over a folder of real scanned PDFs. Not part of the normal
//! test run: it needs a folder of documents and takes as long as they do.
//!
//! ```text
//! DESKEW_EVAL_DIR=/path/to/pdfs DESKEW_EVAL_OUT=/path/out.csv \
//!   cargo test --release -p pdfcraft-engine --lib deskew_eval_folder -- --ignored --nocapture
//! ```
//!
//! For every PDF (sorted, recursively) up to [`MAX_PAGES_PER_FILE`] evenly spaced pages are read
//! the way an OCR job would read them (`language = "th"`: rendered at the scan's own resolution, at
//! least 200 dpi), skipping pages that carry their own text. Each page that is a scan gets the
//! detector's verdict, its timing, and a second measurement of the straightened result (the
//! residual: a good correction leaves none). One CSV row per page; the file's path goes only to a
//! separate `<out>.files.csv`, so the rows can be shared without names. A run resumes after the
//! last file already in the CSV.

use super::*;
use pdfcraft_ocr::deskew::{DeskewOptions, analyze, straighten};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Instant;

const MAX_PAGES_PER_FILE: usize = 100;

fn collect(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(read) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = read.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        if p.is_dir() {
            collect(&p, out);
        } else if p.extension().is_some_and(|e| e.eq_ignore_ascii_case("pdf")) {
            out.push(p);
        }
    }
}

/// Up to `cap` page numbers of `n`, evenly spaced (all when `n <= cap`).
fn spread(n: usize, cap: usize) -> Vec<usize> {
    if n <= cap { (0..n).collect() } else { (0..cap).map(|i| i * n / cap).collect() }
}

fn eval_file(idx: usize, folder: &str, bytes: Vec<u8>, only: Option<&Vec<usize>>, csv: &mut impl Write) {
    let mut s = Session::new();
    let id = match s.open("eval.pdf", None, Arc::new(bytes), None) {
        Ok(id) => id,
        Err(e) => {
            let _ = writeln!(
                csv,
                "{idx},{folder},-1,0,0,0,open-error:{},,,,,,,,,,,,",
                e.to_string().replace([',', '\n'], " ").chars().take(60).collect::<String>()
            );
            return;
        }
    };
    let total = s.get(id).map_or(0, |d| d.info.pages.len());
    let settings = OcrSettings {
        language: "th".into(),
        skip_text_pages: true,
        force_ocr: false,
        deskew: false,
        auto_rotate: false,
        dpi: 300.0,
        ..Default::default()
    };
    let Some(job) = s.ocr_job(id, &only.cloned().unwrap_or_else(|| spread(total, MAX_PAGES_PER_FILE)), settings) else { return };
    let mut steps = job.steps();
    let opts = DeskewOptions::default();
    loop {
        let t_render = Instant::now();
        let Some(next) = steps.next_page() else { break };
        let render_ms = t_render.elapsed().as_secs_f64() * 1000.0;
        match next {
            Err(skipped) => {
                let why = skipped.skipped.unwrap_or_default();
                let kind = if why.contains("already contains text") {
                    "has-text"
                } else if why.contains("unreadable") {
                    "unusable-text"
                } else {
                    "skipped"
                };
                let _ = writeln!(csv, "{idx},{folder},{},{total},0,0,{kind},,,,,,,,,,,{render_ms:.0},", skipped.page);
            }
            Ok(shot) => {
                let Some(img) = shot.image() else { continue };
                let (w, h) = (img.width(), img.height());
                let t = Instant::now();
                let r = analyze(img, &opts);
                let analyze_ms = t.elapsed().as_secs_f64() * 1000.0;
                let t = Instant::now();
                let (resid, resid_conf, straighten_ms) = match straighten(img, &opts) {
                    Ok(out) => {
                        let ms = t.elapsed().as_secs_f64() * 1000.0;
                        let again = analyze(&out.image, &opts);
                        (again.detected_deg, again.confidence, ms)
                    }
                    Err(_) => (f64::NAN, f64::NAN, f64::NAN),
                };
                let _ = writeln!(
                    csv,
                    "{idx},{folder},{},{total},{w},{h},scan,{:.3},{:.3},{:.3},{},{:.3},{:.3},{analyze_ms:.1},{straighten_ms:.1},{resid:.3},{resid_conf:.3},{render_ms:.0},",
                    shot.page, r.detected_deg, r.applied_deg, r.confidence, r.orientation_turns, r.orientation_confidence, r.axis_ratio
                );
            }
        }
    }
}

#[test]
#[ignore = "evaluation over a folder of real PDFs (DESKEW_EVAL_DIR, DESKEW_EVAL_OUT)"]
fn deskew_eval_folder() {
    let (Some(dir), Some(out)) = (std::env::var_os("DESKEW_EVAL_DIR"), std::env::var_os("DESKEW_EVAL_OUT")) else {
        eprintln!("skipped: set DESKEW_EVAL_DIR and DESKEW_EVAL_OUT");
        return;
    };
    let dir = PathBuf::from(dir);
    let out = PathBuf::from(out);
    // Optional: only the pages named in a file of `file:page` lines (indices as in an earlier CSV).
    let only: Option<std::collections::HashMap<usize, Vec<usize>>> = std::env::var_os("DESKEW_EVAL_PAGES").map(|p| {
        let mut m: std::collections::HashMap<usize, Vec<usize>> = std::collections::HashMap::new();
        for l in std::fs::read_to_string(p).unwrap_or_default().lines() {
            if let Some((f, pg)) = l.trim().split_once(':')
                && let (Ok(f), Ok(pg)) = (f.parse::<usize>(), pg.parse::<usize>())
            {
                m.entry(f).or_default().push(pg);
            }
        }
        m
    });
    let mut files = Vec::new();
    collect(&dir, &mut files);
    // Resume after the last file index already written.
    let done = std::fs::read_to_string(&out).ok().and_then(|t| t.lines().filter_map(|l| l.split(',').next()?.parse::<usize>().ok()).max());
    let first = done.map_or(0, |d| d + 1);
    let header = !out.exists();
    let mut csv = std::fs::OpenOptions::new().create(true).append(true).open(&out).unwrap();
    let mut names = std::fs::OpenOptions::new().create(true).append(true).open(out.with_extension("files.csv")).unwrap();
    if header {
        writeln!(csv, "file,folder,page,pages,w,h,status,detected,applied,confidence,turns,orient_conf,axis_ratio,analyze_ms,straighten_ms,resid,resid_conf,render_ms,").unwrap();
    }
    eprintln!("{} PDFs under {}, starting at #{first}", files.len(), dir.display());
    let began = Instant::now();
    for (idx, path) in files.iter().enumerate().skip(first) {
        let folder = path
            .strip_prefix(&dir)
            .ok()
            .and_then(|p| p.components().next())
            .map_or_else(String::new, |c| c.as_os_str().to_string_lossy().into_owned());
        let pages = match &only {
            Some(m) => match m.get(&idx) {
                Some(p) => Some(p),
                None => continue,
            },
            None => None,
        };
        let size = std::fs::metadata(path).map_or(0, |m| m.len());
        writeln!(names, "{idx},{size},{}", path.strip_prefix(&dir).unwrap_or(path).display()).unwrap();
        let Ok(bytes) = std::fs::read(path) else { continue };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| eval_file(idx, &folder, bytes, pages, &mut csv)));
        if result.is_err() {
            writeln!(csv, "{idx},{folder},-1,0,0,0,panic,,,,,,,,,,,,").unwrap();
        }
        csv.flush().unwrap();
        if idx % 20 == 0 {
            eprintln!("#{idx}/{} done, {:.0}s elapsed", files.len(), began.elapsed().as_secs_f64());
        }
    }
    eprintln!("finished in {:.0}s", began.elapsed().as_secs_f64());
}

/// Box-average `rgba` (w x h) down to `tw` wide; returns grey bytes and the new height.
fn thumb(rgba: &[u8], w: usize, h: usize, tw: usize) -> (Vec<u8>, usize) {
    let th = (h * tw / w).max(1);
    let mut sum = vec![0u32; tw * th];
    let mut cnt = vec![0u32; tw * th];
    for y in 0..h {
        let dy = (y * th / h).min(th - 1);
        for x in 0..w {
            let o = (y * w + x) * 4;
            let g = (299 * u32::from(rgba[o]) + 587 * u32::from(rgba[o + 1]) + 114 * u32::from(rgba[o + 2])) / 1000;
            let i = dy * tw + (x * tw / w).min(tw - 1);
            sum[i] += g;
            cnt[i] += 1;
        }
    }
    (sum.iter().zip(&cnt).map(|(&s, &c)| s.checked_div(c).map_or(255, |v| v as u8)).collect(), th)
}

/// Writes before | after thumbnails of chosen pages for looking at: `DESKEW_DUMP_LIST` is
/// `file:page,file:page,...` (indices as in the evaluation CSV; pages 0-based), `DESKEW_DUMP_DIR`
/// the output folder. The thumbnails show real documents: keep them local.
#[test]
#[ignore = "looks at chosen pages of a folder of real PDFs (DESKEW_EVAL_DIR, DESKEW_DUMP_LIST, DESKEW_DUMP_DIR)"]
fn deskew_eval_dump() {
    let (Some(dir), Some(list), Some(out)) =
        (std::env::var_os("DESKEW_EVAL_DIR"), std::env::var("DESKEW_DUMP_LIST").ok(), std::env::var_os("DESKEW_DUMP_DIR"))
    else {
        eprintln!("skipped: set DESKEW_EVAL_DIR, DESKEW_DUMP_LIST and DESKEW_DUMP_DIR");
        return;
    };
    let (dir, out) = (PathBuf::from(dir), PathBuf::from(out));
    std::fs::create_dir_all(&out).unwrap();
    let mut files = Vec::new();
    collect(&dir, &mut files);
    for item in list.split(',') {
        let Some((f, p)) = item.split_once(':') else { continue };
        let (Ok(f), Ok(p)) = (f.parse::<usize>(), p.parse::<usize>()) else { continue };
        let Some(path) = files.get(f) else { continue };
        let mut s = Session::new();
        let id = s.open("d.pdf", None, Arc::new(std::fs::read(path).unwrap()), None).unwrap();
        let settings =
            OcrSettings { language: "th".into(), skip_text_pages: true, deskew: false, auto_rotate: false, dpi: 300.0, ..Default::default() };
        let mut steps = s.ocr_job(id, &[p], settings).unwrap().steps();
        let Some(Ok(shot)) = steps.next_page() else { continue };
        let img = shot.image().unwrap();
        let fixed = straighten(img, &DeskewOptions::default()).unwrap();
        let r = fixed.report;
        let (a, ha) = thumb(img.rgba(), img.width() as usize, img.height() as usize, 420);
        let (b, hb) = thumb(fixed.image.rgba(), fixed.image.width() as usize, fixed.image.height() as usize, 420);
        let hh = ha.max(hb);
        let mut sheet = vec![160u8; (420 * 2 + 10) * hh];
        for y in 0..ha {
            sheet[y * 850..y * 850 + 420].copy_from_slice(&a[y * 420..y * 420 + 420]);
        }
        for y in 0..hb {
            sheet[y * 850 + 430..y * 850 + 850].copy_from_slice(&b[y * 420..y * 420 + 420]);
        }
        let mut png_bytes = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png_bytes, 850, hh as u32);
            enc.set_color(png::ColorType::Grayscale);
            enc.write_header().unwrap().write_image_data(&sheet).unwrap();
        }
        std::fs::write(out.join(format!("f{f}_p{p}.png")), png_bytes).unwrap();
        eprintln!(
            "f{f} p{p}: detected {:.2} applied {:.2} conf {:.2} turns {} oconf {:.2}",
            r.detected_deg, r.applied_deg, r.confidence, r.orientation_turns, r.orientation_confidence
        );
    }
}

fn read_stats(rec: &dyn pdfcraft_ocr::Recognizer, img: &pdfcraft_ocr::OcrImage) -> (f32, usize, usize, f64) {
    let t = Instant::now();
    let lines = rec.recognize(img, &pdfcraft_ocr::RecognizeOptions { language: "th".into() }).unwrap_or_default();
    let ms = t.elapsed().as_secs_f64() * 1000.0;
    let words: usize = lines.iter().map(|l| l.words.len()).sum();
    let chars: usize = lines.iter().flat_map(|l| &l.words).map(|w| w.text.chars().count()).sum();
    (pdfcraft_ocr::mean_confidence(&lines).unwrap_or(0.0), words, chars, ms)
}

/// OCR confidence before and after deskew, and under each quarter turn, on the pages named in
/// `DESKEW_EVAL_PAGES` (`file:page` lines): per page the mean word confidence, words and
/// characters of the raw page, of the deskewed page, and of the deskewed page turned 90, 180
/// and 270 degrees clockwise, plus the detector's own orientation cue. Needs the Thai models
/// (`PDFCRAFT_MODELS`). Writes `DESKEW_EVAL_OUT`.
#[test]
#[ignore = "OCR over chosen pages of a folder of real PDFs (DESKEW_EVAL_DIR, DESKEW_EVAL_PAGES, DESKEW_EVAL_OUT, PDFCRAFT_MODELS)"]
fn deskew_eval_ocr() {
    let (Some(dir), Some(list), Some(out)) =
        (std::env::var_os("DESKEW_EVAL_DIR"), std::env::var_os("DESKEW_EVAL_PAGES"), std::env::var_os("DESKEW_EVAL_OUT"))
    else {
        eprintln!("skipped: set DESKEW_EVAL_DIR, DESKEW_EVAL_PAGES and DESKEW_EVAL_OUT");
        return;
    };
    let base = OcrSettings {
        language: "th".into(),
        skip_text_pages: true,
        force_ocr: false,
        deskew: false,
        auto_rotate: false,
        dpi: 300.0,
        ..Default::default()
    };
    let Ok(recognizers) = recognizers(&base) else {
        eprintln!("skipped: the Thai OCR models are not installed (set PDFCRAFT_MODELS)");
        return;
    };
    let rec = recognizers.primary.clone();
    let (dir, out) = (PathBuf::from(dir), PathBuf::from(out));
    let mut files = Vec::new();
    collect(&dir, &mut files);
    let mut csv = std::fs::File::create(&out).unwrap();
    let cols = ["conf", "words", "chars", "ms"];
    let mut head = String::from("file,page,cue_turns,axis_ratio,applied,det_conf");
    for tag in ["raw", "dsk", "t1", "t2", "t3"] {
        for c in cols {
            head.push_str(&format!(",{tag}_{c}"));
        }
    }
    writeln!(csv, "{head}").unwrap();
    let began = Instant::now();
    let mut n = 0;
    for line in std::fs::read_to_string(list).unwrap_or_default().lines() {
        let Some((f, p)) = line.trim().split_once(':') else { continue };
        let (Ok(f), Ok(p)) = (f.parse::<usize>(), p.parse::<usize>()) else { continue };
        let Some(path) = files.get(f) else { continue };
        let mut s = Session::new();
        let Ok(id) = s.open("o.pdf", None, Arc::new(std::fs::read(path).unwrap()), None) else { continue };
        let Some(job) = s.ocr_job(id, &[p], base.clone()) else { continue };
        let mut steps = job.steps();
        let Some(Ok(shot)) = steps.next_page() else { continue };
        let Some(raw) = shot.image() else { continue };
        let cue = analyze(raw, &DeskewOptions { detect_orientation: true, ..Default::default() });
        let Ok(fixed) = straighten(raw, &DeskewOptions::default()) else { continue };
        let mut row =
            format!("{f},{p},{},{:.3},{:.3},{:.3}", cue.orientation_turns, cue.axis_ratio, fixed.report.applied_deg, fixed.report.confidence);
        let mut push = |st: (f32, usize, usize, f64)| row.push_str(&format!(",{:.2},{},{},{:.0}", st.0, st.1, st.2, st.3));
        push(read_stats(rec.as_ref(), raw));
        push(read_stats(rec.as_ref(), &fixed.image));
        for k in 1..=3u32 {
            push(read_stats(rec.as_ref(), &pdfcraft_ocr::preprocess::turn_image(&fixed.image, k)));
        }
        writeln!(csv, "{row}").unwrap();
        csv.flush().unwrap();
        n += 1;
        if n % 10 == 0 {
            eprintln!("{n} pages, {:.0}s", began.elapsed().as_secs_f64());
        }
    }
    eprintln!("finished {n} pages in {:.0}s", began.elapsed().as_secs_f64());
}

/// The product pipeline end to end on the pages named in `DESKEW_EVAL_PAGES`, in three
/// configurations: nothing, deskew only, deskew plus the orientation check
/// (`OcrShot::read`). Per page: characters, mean confidence, words and milliseconds of each, and
/// the turn the last one chose. Needs the Thai models. Writes `DESKEW_EVAL_OUT`.
#[test]
#[ignore = "OCR over chosen pages of a folder of real PDFs (DESKEW_EVAL_DIR, DESKEW_EVAL_PAGES, DESKEW_EVAL_OUT, PDFCRAFT_MODELS)"]
fn deskew_eval_pipeline() {
    let (Some(dir), Some(list), Some(out)) =
        (std::env::var_os("DESKEW_EVAL_DIR"), std::env::var_os("DESKEW_EVAL_PAGES"), std::env::var_os("DESKEW_EVAL_OUT"))
    else {
        eprintln!("skipped: set DESKEW_EVAL_DIR, DESKEW_EVAL_PAGES and DESKEW_EVAL_OUT");
        return;
    };
    let base = OcrSettings { language: "th".into(), skip_text_pages: true, force_ocr: false, dpi: 300.0, ..Default::default() };
    let Ok(recognizers) = recognizers(&base) else {
        eprintln!("skipped: the Thai OCR models are not installed (set PDFCRAFT_MODELS)");
        return;
    };
    let (dir, out) = (PathBuf::from(dir), PathBuf::from(out));
    let mut files = Vec::new();
    collect(&dir, &mut files);
    let mut csv = std::fs::File::create(&out).unwrap();
    writeln!(
        csv,
        "file,page,none_chars,none_conf,none_words,none_ms,dsk_chars,dsk_conf,dsk_words,dsk_ms,rot_chars,rot_conf,rot_words,rot_ms,rot_turns"
    )
    .unwrap();
    let began = Instant::now();
    let mut n = 0;
    for line in std::fs::read_to_string(list).unwrap_or_default().lines() {
        let Some((f, p)) = line.trim().split_once(':') else { continue };
        let (Ok(f), Ok(p)) = (f.parse::<usize>(), p.parse::<usize>()) else { continue };
        let Some(path) = files.get(f) else { continue };
        let bytes = Arc::new(std::fs::read(path).unwrap());
        let mut row = format!("{f},{p}");
        let mut turns = 0;
        let mut ok = true;
        for (deskew, rotate) in [(false, false), (true, false), (true, true)] {
            let mut s = Session::new();
            let Ok(id) = s.open("o.pdf", None, bytes.clone(), None) else {
                ok = false;
                break;
            };
            let settings = OcrSettings { deskew, auto_rotate: rotate, ..base.clone() };
            let Some(job) = s.ocr_job(id, &[p], settings) else {
                ok = false;
                break;
            };
            let mut steps = job.steps();
            let Some(Ok(shot)) = steps.next_page() else {
                ok = false;
                break;
            };
            let t = Instant::now();
            let page = shot.read(&recognizers);
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let chars: usize = page.words.iter().map(|w| w.text.chars().count()).sum();
            row.push_str(&format!(",{chars},{:.2},{},{ms:.0}", page.mean_confidence().unwrap_or(f32::NAN), page.words.len()));
            if rotate {
                turns = page.deskew.map_or(0, |d| d.orientation_turns);
            }
        }
        if !ok {
            continue;
        }
        writeln!(csv, "{row},{turns}").unwrap();
        csv.flush().unwrap();
        n += 1;
        if n % 10 == 0 {
            eprintln!("{n} pages, {:.0}s", began.elapsed().as_secs_f64());
        }
    }
    eprintln!("finished {n} pages in {:.0}s", began.elapsed().as_secs_f64());
}
