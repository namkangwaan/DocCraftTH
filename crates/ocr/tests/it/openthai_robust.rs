//! SRS NFR-01 / NFR-03 for the OpenThai engine: hostile images and malformed model files must
//! produce an error or an empty reading, never a panic, a hang or an unbounded allocation.
//!
//! The image tests need the model set (`openthai-*` in `$PDFCRAFT_MODELS`, beside the executable
//! or in `assets/models/`) and skip when it is not installed. The malformed-file tests need none.

use pdfcraft_ocr::openthai::{DET_MODEL, DICT, OpenThai, REC_MODEL};
use pdfcraft_ocr::{OcrError, OcrImage};

/// A deterministic pseudo-random byte stream (xorshift), so a failure reproduces.
struct Noise(u64);

impl Noise {
    fn next(&mut self) -> u8 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 24) as u8
    }
}

fn engine() -> Option<OpenThai> {
    let dir = OpenThai::find_dir()?;
    OpenThai::load(&dir).ok()
}

fn image(w: u32, h: u32, mut px: impl FnMut(u32, u32) -> [u8; 3]) -> OcrImage {
    let mut rgba = Vec::with_capacity(w as usize * h as usize * 4);
    for y in 0..h {
        for x in 0..w {
            let [r, g, b] = px(x, y);
            rgba.extend([r, g, b, 255]);
        }
    }
    OcrImage::new(w, h, rgba).expect("a valid image")
}

/// Reading never fails and never yields more lines than a page could hold.
fn reads_without_panicking(engine: &OpenThai, name: &str, img: &OcrImage) {
    let lines = engine.read(img).unwrap_or_else(|e| panic!("{name}: {e}"));
    assert!(lines.len() <= 3000, "{name}: {} lines", lines.len());
    for line in &lines {
        for w in &line.words {
            assert!(w.rect.iter().all(|v| v.is_finite()), "{name}: a word box is not finite: {:?}", w.rect);
        }
    }
}

#[test]
fn hostile_images_are_read_without_panicking() {
    let Some(engine) = engine() else {
        eprintln!("skipped: the OpenThai models are not installed");
        return;
    };
    let mut noise = Noise(0x9e37_79b9_7f4a_7c15);
    let cases: Vec<(&str, OcrImage)> = vec![
        ("1x1", image(1, 1, |_, _| [0, 0, 0])),
        ("blank", image(300, 200, |_, _| [255, 255, 255])),
        ("black", image(300, 200, |_, _| [0, 0, 0])),
        ("tall strip", image(3, 4000, |_, y| [(y % 255) as u8; 3])),
        ("wide strip", image(4000, 3, |x, _| [(x % 255) as u8; 3])),
        ("one row", image(2000, 1, |x, _| [(x % 2 * 255) as u8; 3])),
        ("one column", image(1, 2000, |_, y| [(y % 2 * 255) as u8; 3])),
        ("checkerboard", image(512, 512, |x, y| [((x / 4 + y / 4) % 2 * 255) as u8; 3])),
        ("dense stripes", image(600, 400, |_, y| [(y % 2 * 255) as u8; 3])),
        ("noise", image(640, 480, |_, _| [noise.next(), noise.next(), noise.next()])),
        ("gradient", image(700, 300, |x, _| [(x * 255 / 699) as u8; 3])),
        ("coloured blocks", image(400, 400, |x, y| [(x / 50 * 30) as u8, (y / 50 * 30) as u8, 128])),
    ];
    for (name, img) in &cases {
        reads_without_panicking(&engine, name, img);
    }
}

/// A page with ink only at its edge, and one filled edge to edge, exercise the padding and the
/// detector's border handling.
#[test]
fn ink_at_the_edges_is_read_without_panicking() {
    let Some(engine) = engine() else {
        eprintln!("skipped: the OpenThai models are not installed");
        return;
    };
    let edge = image(400, 300, |x, y| if x < 2 || y < 2 || x >= 398 || y >= 298 { [0, 0, 0] } else { [255, 255, 255] });
    reads_without_panicking(&engine, "edge frame", &edge);
    let bars = image(400, 300, |x, _| if x % 20 < 10 { [0, 0, 0] } else { [255, 255, 255] });
    reads_without_panicking(&engine, "vertical bars", &bars);
}

/// A4 at 300 dpi (2480 x 3508, 8.7 MP): the largest page the engine is expected to read. It must
/// finish, and the detector's working size (long side 1280 px) keeps memory far under NFR-03's 2 GB.
#[test]
fn an_a4_page_at_300_dpi_is_read() {
    let Some(engine) = engine() else {
        eprintln!("skipped: the OpenThai models are not installed");
        return;
    };
    let mut noise = Noise(7);
    let page = image(2480, 3508, |x, y| if (x / 40 + y / 40) % 7 == 0 { [noise.next(); 3] } else { [255, 255, 255] });
    reads_without_panicking(&engine, "A4 300 dpi", &page);
}

#[test]
fn the_image_caps_still_apply() {
    assert!(matches!(OcrImage::new(10_001, 1, vec![0; 10_001 * 4]), Err(OcrError::ImageTooLarge)));
    assert!(matches!(OcrImage::new(0, 5, Vec::new()), Err(OcrError::EmptyImage)));
}

fn real_files() -> Option<(Vec<u8>, Vec<u8>, Vec<u8>)> {
    let dir = OpenThai::find_dir()?;
    let read = |f: &str| std::fs::read(dir.join(f)).ok();
    Some((read(DET_MODEL)?, read(REC_MODEL)?, read(DICT)?))
}

/// Truncated, garbage and empty model files are load errors, never a panic or a hang.
#[test]
fn malformed_model_files_are_load_errors() {
    let Some((det, rec, dict)) = real_files() else {
        eprintln!("skipped: the OpenThai models are not installed");
        return;
    };
    let junk = vec![0xffu8; 64];
    let truncated = |v: &[u8]| v.get(..v.len() / 2).unwrap_or_default().to_vec();
    let cases: Vec<(&str, Vec<u8>, Vec<u8>)> = vec![
        ("empty det", Vec::new(), rec.clone()),
        ("junk det", junk.clone(), rec.clone()),
        ("truncated det", truncated(&det), rec.clone()),
        ("empty rec", det.clone(), Vec::new()),
        ("junk rec", det.clone(), junk.clone()),
        ("truncated rec", det.clone(), truncated(&rec)),
        ("an ONNX-looking header", b"\x08\x07\x12\x07pytorch".to_vec(), rec.clone()),
    ];
    let mut bad = Vec::new();
    for (name, d, r) in cases {
        let dict = dict.clone();
        check_load(&mut bad, name, move || OpenThai::from_bytes(d, r, None, &dict));
    }
    let (det2, rec2, dict2) = (det.clone(), rec.clone(), dict.clone());
    check_load(&mut bad, "a junk rec2", move || OpenThai::from_bytes(det2, rec2, Some(junk), &dict2));
    check_load(&mut bad, "a dictionary that is not UTF-8", move || OpenThai::from_bytes(det, rec, None, &[0xff, 0xfe, 0xfd]));
    assert!(bad.is_empty(), "these did not fail cleanly: {bad:?}");
}

/// How long loading a malformed file may take before it counts as a hang.
const LOAD_LIMIT: std::time::Duration = std::time::Duration::from_secs(20);

/// Runs `load` on its own thread and records in `bad` when it hangs, panics or succeeds: a
/// malformed model must be a clean error. (A hung thread is left behind; the process ends the test run.)
fn check_load(bad: &mut Vec<String>, name: &str, load: impl FnOnce() -> Result<OpenThai, OcrError> + Send + 'static) {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(load().map(|_| ()));
    });
    match rx.recv_timeout(LOAD_LIMIT) {
        Ok(Err(_)) => {}
        Ok(Ok(())) => bad.push(format!("{name}: loaded")),
        Err(std::sync::mpsc::RecvTimeoutError::Timeout) => bad.push(format!("{name}: hung (over {LOAD_LIMIT:?})")),
        Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => bad.push(format!("{name}: panicked")),
    }
}

/// Valid models in the wrong roles (the recognizer as the detector and the other way round) load,
/// but reading with them must be an error or an empty reading, never a panic.
#[test]
fn models_in_the_wrong_roles_read_without_panicking() {
    let Some((det, rec, dict)) = real_files() else {
        eprintln!("skipped: the OpenThai models are not installed");
        return;
    };
    let engine = OpenThai::from_bytes(rec, det, None, &dict).expect("both are valid .rten models");
    let stripes = image(500, 100, |x, _| if x % 12 < 6 { [0, 0, 0] } else { [255, 255, 255] });
    let _ = engine.read(&stripes);
}

/// A dictionary that does not match the recognizer (empty, short, or longer) must not panic when
/// a page is read: class numbers past the table decode to nothing.
#[test]
fn a_mismatched_dictionary_reads_without_panicking() {
    let Some((det, rec, _)) = real_files() else {
        eprintln!("skipped: the OpenThai models are not installed");
        return;
    };
    let stripes = image(500, 100, |x, _| if x % 12 < 6 { [0, 0, 0] } else { [255, 255, 255] });
    for (name, dict) in [("empty", String::new()), ("one entry", "ก\n".into()), ("blank lines", "\n\n\n".into()), ("long", "ก\n".repeat(5000))] {
        let engine = OpenThai::from_bytes(det.clone(), rec.clone(), None, dict.as_bytes()).expect("the models load");
        reads_without_panicking(&engine, name, &stripes);
    }
}
