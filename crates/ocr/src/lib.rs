//! pdfcraft-ocr — Scan & OCR ▸ Recognize text (L4).
//!
//! Recognition runs the ocrs engine (MIT/Apache-2.0) with its pre-trained models (CC-BY-SA-4.0,
//! fetched by `cargo xtask models`; see ATTRIBUTION.toml). The caller renders a page to pixels;
//! [`Ocr::recognize`] finds the words in them, and [`text_layer`] turns words placed in user space
//! into page content: invisible text (rendering mode 3) over each word, so the page becomes a
//! searchable image (Acrobat's "Searchable Image (Exact)": the image is left untouched).
//!
//! The default models read the Latin alphabet (English and other languages written without
//! accents); Thai uses the `th_PP-OCRv5_mobile_rec` CTC recognizer over `ocrs` word detection.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod ctc;
pub mod metrics;

use std::path::{Path, PathBuf};

pub use pdfcraft_fonts::helvetica_width;

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.rten";
pub const RECOGNITION_MODEL: &str = "text-recognition.rten";
pub const THAI_RECOGNITION_MODEL: &str = "text-recognition-th.rten";
pub const THAI_DICT: &str = "dict-th.txt";

/// The recognition model's alphabet, as it was trained (ocrs-models' `DEFAULT_ALPHABET`): its
/// output classes follow this order. The ocrs crate's own copy has an `E` where the training had
/// `€`, which made a scanned "€250" read as "E250".
pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// The languages the models read (ISO 639-1).
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("th", "ไทย")];

#[derive(Debug, thiserror::Error)]
pub enum OcrError {
    /// Shown to end users (release packages ship the models; a source build fetches them with
    /// `cargo xtask models`). The UI catalogs key this exact text.
    #[error("the text recognition models are not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds them)")]
    NoModels,
    #[error("the {0} text recognition model is not installed (reinstall PdfCraft, or set PDFCRAFT_MODELS to the folder that holds it)")]
    NoLanguageModel(String),
    #[error("loading {0}: {1}")]
    Load(String, String),
    #[error("text recognition failed: {0}")]
    Recognize(String),
    #[error("the image is empty")]
    EmptyImage,
}

/// Where the model files are.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
    pub dictionary: Option<PathBuf>,
}

impl Models {
    /// The default (English/Latin) models in `dir`, if both files are there.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        let m = Models { detection: dir.join(DETECTION_MODEL), recognition: dir.join(RECOGNITION_MODEL), dictionary: None };
        (m.detection.is_file() && m.recognition.is_file()).then_some(m)
    }

    /// The models for `lang` (`"en"` or `"th"`) in `dir`, if all required files are there.
    pub fn in_dir_for(dir: &Path, lang: &str) -> Option<Models> {
        match lang {
            "th" | "tha" => {
                let detection = dir.join(DETECTION_MODEL);
                let recognition = dir.join(THAI_RECOGNITION_MODEL);
                let dict = dir.join(THAI_DICT);
                (detection.is_file() && recognition.is_file() && dict.is_file()).then_some(Models { detection, recognition, dictionary: Some(dict) })
            }
            "en" | "eng" | "" => Self::in_dir(dir),
            _ => None,
        }
    }

    /// Look for the default models: `$PDFCRAFT_MODELS`, then where the release packages install
    /// them beside the executable ([`Models::dirs_beside_exe`]), then the source tree's `assets/models/`.
    pub fn find() -> Option<Models> {
        Self::search_dirs().iter().find_map(|d| Self::in_dir(d))
    }

    /// Look for the models for `lang` across [`Models::search_dirs`].
    pub fn find_for(lang: &str) -> Option<Models> {
        Self::search_dirs().iter().find_map(|d| Self::in_dir_for(d, lang))
    }

    /// The directories [`Models::find`] looks in, in order.
    pub fn search_dirs() -> Vec<PathBuf> {
        let mut dirs = Vec::new();
        if let Some(d) = std::env::var_os("PDFCRAFT_MODELS") {
            dirs.push(PathBuf::from(d));
        }
        if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
            dirs.extend(Self::dirs_beside_exe(&exe));
        }
        dirs.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/models"));
        dirs
    }

    /// Where the release packages put the models, relative to the directory holding the
    /// executable (packaging/*):
    /// - `models/` beside it: the Windows MSI and portable zip;
    /// - `../Resources/models`: the macOS app bundle (`Contents/MacOS` → `Contents/Resources`);
    /// - `../share/pdfcraft/models`: the FHS-style trees (`bin/` → `share/`) of the Linux
    ///   deb/rpm/tar.gz/AppImage/Flatpak and the FreeBSD tarball, wherever they are installed.
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }
}

/// A recognised word: its text and its box in image pixels `[left, top, right, bottom]`.
#[derive(Clone, Debug, PartialEq)]
pub struct Word {
    pub text: String,
    pub rect: [f32; 4],
}

/// A recognised line of words, in reading order.
#[derive(Clone, Debug, PartialEq, Default)]
pub struct Line {
    pub words: Vec<Word>,
}

impl Line {
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

enum Recognizer {
    Ocrs(ocrs::OcrEngine),
    Ctc { detector: ocrs::OcrEngine, model: Box<rten::Model>, dict: Vec<char> },
}

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    recognizer: Recognizer,
}

/// Height (in pixels) expected by the PaddleOCR v5 mobile recognizer (`[1, 3, 48, W]`).
const CTC_HEIGHT: usize = 48;
/// Maximum resized crop width (in pixels) to bound memory allocation on pathological boxes.
const MAX_CTC_WIDTH: usize = 8192;

impl Ocr {
    pub fn load(models: &Models) -> Result<Ocr, OcrError> {
        let load = |p: &Path| rten::Model::load_file(p).map_err(|e| OcrError::Load(p.display().to_string(), e.to_string()));
        if let Some(dict_path) = &models.dictionary {
            let detector_params =
                ocrs::OcrEngineParams { detection_model: Some(load(&models.detection)?), recognition_model: None, ..Default::default() };
            let detector = ocrs::OcrEngine::new(detector_params).map_err(|e| OcrError::Load("ocr detector".into(), e.to_string()))?;
            let model = Box::new(load(&models.recognition)?);
            let raw = std::fs::read_to_string(dict_path).map_err(|e| OcrError::Load(dict_path.display().to_string(), e.to_string()))?;
            let mut dict: Vec<char> = raw.lines().filter_map(|line| line.chars().next()).collect();
            // PaddleOCR's `use_space_char: true` appends the space class after the dictionary entries.
            dict.push(' ');
            Ok(Ocr { recognizer: Recognizer::Ctc { detector, model, dict } })
        } else {
            let params = ocrs::OcrEngineParams {
                detection_model: Some(load(&models.detection)?),
                recognition_model: Some(load(&models.recognition)?),
                alphabet: Some(ALPHABET.into()),
                ..Default::default()
            };
            let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
            Ok(Ocr { recognizer: Recognizer::Ocrs(engine) })
        }
    }

    /// Load the default models found by [`Models::find`].
    pub fn find() -> Result<Ocr, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }

    /// Load the models for `lang` found by [`Models::find_for`].
    pub fn load_for(lang: &str) -> Result<Ocr, OcrError> {
        let models = match lang {
            "en" | "eng" | "" => Models::find().ok_or(OcrError::NoModels)?,
            other => Models::find_for(other).ok_or_else(|| OcrError::NoLanguageModel(other.into()))?,
        };
        Self::load(&models)
    }

    /// Recognise the text in an RGBA (or RGB, or grey) image, `width` × `height` pixels.
    pub fn recognize(&self, pixels: &[u8], width: u32, height: u32) -> Result<Vec<Line>, OcrError> {
        let rgb = channels_for_ocrs(pixels, width, height)?;
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        match &self.recognizer {
            Recognizer::Ocrs(engine) => {
                let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
                let input = engine.prepare_input(source).map_err(|e| err(&e))?;
                let found = engine.detect_words(&input).map_err(|e| err(&e))?;
                let lines = engine.find_text_lines(&input, &found);
                let read = engine.recognize_text(&input, &lines).map_err(|e| err(&e))?;
                use ocrs::TextItem;
                Ok(read
                    .into_iter()
                    .flatten()
                    .map(|line| Line {
                        words: line
                            .words()
                            .map(|w| {
                                let r = w.bounding_rect();
                                Word { text: w.to_string(), rect: [r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32] }
                            })
                            .filter(|w| !w.text.trim().is_empty())
                            .collect(),
                    })
                    .filter(|l| !l.words.is_empty())
                    .collect())
            }
            Recognizer::Ctc { detector, model, dict } => {
                let cleaned = erase_dot_leaders(&rgb, width, height);
                let rgb: &[u8] = cleaned.as_deref().unwrap_or(&rgb);
                let source = ocrs::ImageSource::from_bytes(rgb, (width, height)).map_err(|e| err(&e))?;
                let input = detector.prepare_input(source).map_err(|e| err(&e))?;
                let found = detector.detect_words(&input).map_err(|e| err(&e))?;
                let merged_boxes = ctc_line_boxes(detector, &input, found, width);

                let mut out = Vec::new();
                for [left, top, right, bottom] in merged_boxes {
                    let box_h = bottom - top;
                    // Pad vertical bounds (25% above, 15% below) so Thai marks above/below the base
                    // line are never clipped by the Latin-trained detector, plus a small horizontal margin.
                    let pad_x = (box_h * 0.15).max(2.0);
                    let x0 = (left - pad_x).floor().max(0.0) as u32;
                    let y0 = (top - box_h * 0.25).floor().max(0.0) as u32;
                    let x1 = (right + pad_x).ceil().min(width as f32).max(0.0) as u32;
                    let y1 = (bottom + box_h * 0.15).ceil().min(height as f32).max(0.0) as u32;
                    if x1 <= x0 || y1 <= y0 {
                        continue;
                    }
                    let Some(text) = recognize_ctc_crop(rgb, width, height, [x0, y0, x1, y1], model, dict)? else {
                        continue;
                    };
                    let trimmed = text.trim();
                    if trimmed.is_empty() {
                        continue;
                    }
                    let parts: Vec<&str> = trimmed.split_whitespace().collect();
                    let total_chars: usize = parts.iter().map(|p| p.chars().count().max(1)).sum();
                    if parts.len() <= 1 || total_chars == 0 {
                        out.push(Line { words: vec![Word { text: trimmed.to_string(), rect: [left, top, right, bottom] }] });
                    } else {
                        let span = right - left;
                        let mut cur = left;
                        let mut words = Vec::new();
                        for part in parts {
                            let w_span = span * (part.chars().count().max(1) as f32 / total_chars as f32);
                            words.push(Word { text: part.to_string(), rect: [cur, top, cur + w_span, bottom] });
                            cur += w_span;
                        }
                        out.push(Line { words });
                    }
                }
                Ok(out)
            }
        }
    }

    /// Recognise text directly from a crop `[x0, y0, x1, y1]` using the recognition model.
    /// The rectangle is clipped to the image; one that is empty after clipping (or inverted)
    /// reads as `None`, as does any crop under the default (non-CTC) models.
    pub fn recognize_crop(&self, pixels: &[u8], width: u32, height: u32, rect: [u32; 4]) -> Result<Option<String>, OcrError> {
        let rgb = channels_for_ocrs(pixels, width, height)?;
        let Some(rect) = clip_crop(rect, width, height) else {
            return Ok(None);
        };
        match &self.recognizer {
            Recognizer::Ocrs(_) => Ok(None),
            Recognizer::Ctc { model, dict, .. } => recognize_ctc_crop(&rgb, width, height, rect, model, dict),
        }
    }
}

/// The most pixels an image to recognise may have: four times the renderer's page budget.
/// Larger images are refused before they reach `ocrs`, which multiplies the dimensions as `u32`.
const MAX_PIXELS: u64 = 1 << 28;

/// `pixels` in the 1 or 3 channels `ocrs` and the CTC crops read (RGBA loses its alpha), once it
/// is known to hold exactly `width` × `height` grey, RGB or RGBA pixels. `ocrs` computed
/// `width * height` in `u32`, so 65536 × 65537 overflowed (a panic in debug builds; a wrapped
/// length, then a shape mismatch, in release).
fn channels_for_ocrs(pixels: &[u8], width: u32, height: u32) -> Result<std::borrow::Cow<'_, [u8]>, OcrError> {
    if width == 0 || height == 0 || pixels.is_empty() {
        return Err(OcrError::EmptyImage);
    }
    // Two u32 factors cannot overflow a u64.
    let count = u64::from(width) * u64::from(height);
    let too_large = || OcrError::Recognize(format!("the image is too large to recognise ({width} × {height} pixels)"));
    if count > MAX_PIXELS {
        return Err(too_large());
    }
    let count = usize::try_from(count).map_err(|_| too_large())?;
    // count ≤ 2^28, so four times it fits a 32-bit usize.
    match pixels.len() {
        n if n == count.saturating_mul(4) => Ok(pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>().into()),
        n if n == count || n == count.saturating_mul(3) => Ok(pixels.into()),
        n => Err(OcrError::Recognize(format!("{n} bytes are not {width} × {height} grey, RGB or RGBA pixels"))),
    }
}

/// The CTC path's text-line boxes `[left, top, right, bottom]` in image pixels, top to bottom:
/// `words` (from the detector) without seal-sized components, grouped into lines, the lines'
/// bounding boxes, and segments split by a seal merged back.
fn ctc_line_boxes(detector: &ocrs::OcrEngine, input: &ocrs::OcrInput, words: Vec<rten_imageproc::RotatedRect>, width: u32) -> Vec<[f32; 4]> {
    let (filtered_words, median_h) = filter_seal_words(words, width);
    let line_polys = detector.find_text_lines(input, &filtered_words);

    let mut raw_boxes = Vec::new();
    for poly_line in line_polys {
        let mut left = f32::INFINITY;
        let mut top = f32::INFINITY;
        let mut right = f32::NEG_INFINITY;
        let mut bottom = f32::NEG_INFINITY;
        for r in &poly_line {
            for p in r.corners() {
                left = left.min(p.x);
                top = top.min(p.y);
                right = right.max(p.x);
                bottom = bottom.max(p.y);
            }
        }
        if left.is_finite() && top.is_finite() && right > left && bottom > top {
            raw_boxes.push([left, top, right, bottom]);
        }
    }
    merge_colinear_line_boxes(raw_boxes, width, median_h)
}

/// The page width, in pixels, at which the dot-leader sizes below were measured (an A4-ish form
/// at 300 dpi); they scale with the image's width.
const LEADER_REF_WIDTH: f32 = 2413.0;
/// The fewest dots in a row that make a leader. Forms use runs this short too, and they hurt the
/// recogniser as much as long ones; an ellipsis ("...") in prose is lost from the text with them.
const MIN_LEADER_DOTS: usize = 3;
/// The most pixels [`find_dot_leaders`] labels (4 bytes each); larger images are read as they are.
const MAX_LEADER_PIXELS: usize = 1 << 26;

/// A fill-in dot leader ("..........") found on a page image: where a form asks to be filled in.
#[derive(Clone, Debug, PartialEq)]
pub struct DotLeader {
    /// Box around all its dots in image pixels `[left, top, right, bottom]`, inside the image.
    pub rect: [f32; 4],
    /// How many dots it has, at least 3.
    pub dots: usize,
}

/// The dot leaders on a page image (RGBA, RGB or grey, `width` × `height` pixels), top to
/// bottom and left to right. The same rows of dots Thai recognition erases (see
/// [`erase_dot_leaders`]); they do not depend on the recognition models. The buffer must be the
/// image, as for [`Ocr::recognize`]; an image too large to scan has none.
pub fn dot_leaders(pixels: &[u8], width: u32, height: u32) -> Result<Vec<DotLeader>, OcrError> {
    let rgb = channels_for_ocrs(pixels, width, height)?;
    let Some(found) = find_dot_leaders(&rgb, width, height) else { return Ok(Vec::new()) };
    Ok(found
        .runs
        .iter()
        .filter_map(|run| {
            let rect = run.iter().fold(None, |acc: Option<[usize; 4]>, d| {
                Some(acc.map_or(*d, |a| [a[0].min(d[0]), a[1].min(d[1]), a[2].max(d[2]), a[3].max(d[3])]))
            })?;
            Some(DotLeader { rect: rect.map(|v| v as f32), dots: run.len() })
        })
        .collect())
}

/// What [`find_dot_leaders`] found: the pixel layout it read and each leader's dots as boxes
/// `[left, top, right, bottom)`.
struct Leaders {
    channels: usize,
    /// How far around each dot [`erase_dot_leaders`] whitens, in pixels.
    margin: usize,
    runs: Vec<Vec<[usize; 4]>>,
}

/// Fill-in dot leaders ("..........") on a form, whitened in a copy of `pixels` (`width` ×
/// `height`, grey or RGB), or `None` when there are none or the image is too large to scan.
///
/// The Thai recogniser loses the words between leaders: a form line whose box and pixels are
/// fine read as 5 of its 49 letters, and a real form page's CER fell from 0.32 to 0.14 once the
/// dots were gone. Leaders carry no text worth keeping (the user fills them in), so they are
/// removed before detection. A leader is a row of [`MIN_LEADER_DOTS`] or more dots (ink blobs at most
/// ~0.8 mm across) standing apart, with ~2 mm gaps at most and no ink straight above or below each dot: that
/// last test keeps Thai marks, which sit on or under a letter. The sizes scale with the image
/// width, so a page scanned at 96 dpi keeps its (small) letters.
fn erase_dot_leaders(pixels: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let Leaders { channels, margin, runs } = find_dot_leaders(pixels, width, height)?;
    let (w, h) = (width as usize, height as usize);
    let mut out = pixels.to_vec();
    for &[x0, y0, x1, y1] in runs.iter().flatten() {
        for y in y0.saturating_sub(margin)..(y1 + margin).min(h) {
            let row = y * w;
            let span = (row + x0.saturating_sub(margin)) * channels..(row + (x1 + margin).min(w)) * channels;
            if let Some(px) = out.get_mut(span) {
                px.fill(255);
            }
        }
    }
    Some(out)
}

/// The rows of [`MIN_LEADER_DOTS`] or more dots in a grey or RGB image (see
/// [`erase_dot_leaders`] for what counts as a dot), or `None` when there are none, the buffer is
/// not the image, or it is too large to scan.
fn find_dot_leaders(pixels: &[u8], width: u32, height: u32) -> Option<Leaders> {
    let (w, h) = (width as usize, height as usize);
    let n = w.checked_mul(h).filter(|&n| n > 0 && n <= MAX_LEADER_PIXELS)?;
    let channels = match pixels.len() {
        l if l == n => 1,
        l if l == n * 3 => 3, // n ≤ 2^26
        _ => return None,
    };
    let ink: Vec<bool> = pixels.chunks_exact(channels).map(|p| p.iter().map(|&v| u32::from(v)).sum::<u32>() < 128 * channels as u32).collect();

    // 4-connected components, labelled with an explicit stack (no recursion).
    struct Blob {
        x0: usize,
        y0: usize,
        x1: usize,
        y1: usize,
        count: usize,
    }
    let mut label = vec![0u32; n];
    let mut blobs: Vec<Blob> = Vec::new();
    let mut stack = Vec::new();
    for start in 0..n {
        if !ink.get(start).copied().unwrap_or(false) || label.get(start).copied().unwrap_or(1) != 0 {
            continue;
        }
        // At most n ≤ 2^26 blobs, so the id fits a u32.
        let id = u32::try_from(blobs.len() + 1).ok()?;
        let mut b = Blob { x0: usize::MAX, y0: usize::MAX, x1: 0, y1: 0, count: 0 };
        if let Some(l) = label.get_mut(start) {
            *l = id;
        }
        stack.push(start);
        while let Some(i) = stack.pop() {
            let (x, y) = (i % w, i / w);
            b.x0 = b.x0.min(x);
            b.y0 = b.y0.min(y);
            b.x1 = b.x1.max(x + 1);
            b.y1 = b.y1.max(y + 1);
            b.count += 1;
            let left = (x > 0).then(|| i - 1);
            let right = (x + 1 < w).then_some(i + 1);
            let up = (y > 0).then(|| i - w);
            let down = (y + 1 < h).then_some(i + w);
            for j in [left, right, up, down].into_iter().flatten() {
                if let (Some(true), Some(l)) = (ink.get(j).copied(), label.get_mut(j))
                    && *l == 0
                {
                    *l = id;
                    stack.push(j);
                }
            }
        }
        blobs.push(b);
    }

    let k = w as f32 / LEADER_REF_WIDTH;
    let dot = ((9.0 * k).round() as usize).max(2);
    let gap = (25.0 * k).round().max(3.0);
    let min_px = ((4.0 * k * k).round() as usize).max(1);
    let band = (3.0 * k).max(1.0);
    let reach = 3 * dot;
    let margin = ((2.0 * k).round() as usize).max(1);
    // Leader dots stand apart (5-6 px clear at 300 dpi); a mark broken into pieces by JPEG
    // has them touching or 1 px apart.
    let clear = ((2.0 * k).round() as usize).max(1);
    let other_ink = |id: u32, ys: std::ops::Range<usize>, xs: std::ops::Range<usize>| {
        ys.flat_map(|y| xs.clone().map(move |x| y * w + x)).any(|i| label.get(i).is_some_and(|&l| l != 0 && l != id))
    };
    // Dots: (row band, centre x, blob index).
    let mut dots: Vec<(i64, f32, usize)> = blobs
        .iter()
        .enumerate()
        .filter(|(i, b)| {
            let id = *i as u32 + 1;
            b.x1 - b.x0 <= dot
                && b.y1 - b.y0 <= dot
                && b.count >= min_px
                && !other_ink(id, b.y0.saturating_sub(reach)..b.y0, b.x0..b.x1)
                && !other_ink(id, b.y1..(b.y1 + reach).min(h), b.x0..b.x1)
        })
        .map(|(i, b)| (((b.y0 + b.y1) as f32 * 0.5 / band).round() as i64, (b.x0 + b.x1) as f32 * 0.5, i))
        .collect();
    dots.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.total_cmp(&b.1)));

    let mut runs: Vec<Vec<[usize; 4]>> = Vec::new();
    let mut run: Vec<usize> = Vec::new();
    let mut close = |run: &mut Vec<usize>| {
        if run.len() >= MIN_LEADER_DOTS {
            runs.push(run.iter().filter_map(|&i| blobs.get(i)).map(|b| [b.x0, b.y0, b.x1, b.y1]).collect());
        }
        run.clear();
    };
    for (at, d) in dots.iter().enumerate() {
        let joins = at > 0
            && dots.get(at - 1).is_some_and(|p| {
                let apart = match (blobs.get(p.2), blobs.get(d.2)) {
                    (Some(a), Some(b)) => b.x0.saturating_sub(a.x1) >= clear,
                    _ => false,
                };
                p.0 == d.0 && d.1 - p.1 <= gap && apart
            });
        if !joins {
            close(&mut run);
        }
        run.push(d.2);
    }
    close(&mut run);
    (!runs.is_empty()).then_some(Leaders { channels, margin, runs })
}

/// The smallest median component size, as a fraction of the page width, that is taken as the
/// text height: about 2 mm on an A4 page. Scanned body text measures 1.4–1.9 % (FR-21 corpus);
/// the dots of a fill-in leader about 0.6 %.
// ponytail: assumes the image is the whole page width; a cropped strip of small text can fall
// under it, which only raises the median (keeps more components). Measure the text instead if
// that ever drops words.
const MIN_TEXT_FRACTION: f32 = 0.01;

/// Filter out non-text connected components (e.g. large circular seals, stamps, or logos)
/// that merge with text words and prevent line grouping. Returns `(filtered_words, median_height)`.
///
/// A component is a seal when its short side is over twice the text height, the median size.
/// On a form the dots of fill-in leaders ("......") come out as many tiny components and drag
/// the median down to a dot's height, so that every real word would read as a seal: when the
/// median is under [`MIN_TEXT_FRACTION`] of `page_width`, it is taken over the components at
/// least that size instead. A floor on the seal size itself does not work: the detector
/// returns a seal as fragments of its ring, only two to four times a word's height.
fn filter_seal_words(words: Vec<rten_imageproc::RotatedRect>, page_width: u32) -> (Vec<rten_imageproc::RotatedRect>, f32) {
    if words.is_empty() {
        return (words, 20.0);
    }
    // `total_cmp`, not `partial_cmp`: a NaN-sized word from a degenerate detection made the
    // comparator inconsistent, which `sort_by` may answer with a panic. Words without a finite
    // size are left out of the median and dropped.
    let mut heights: Vec<f32> = words.iter().filter_map(word_size).collect();
    heights.sort_by(f32::total_cmp);
    let median = |h: &[f32]| h.get(h.len() / 2).copied();
    let min_text = page_width as f32 * MIN_TEXT_FRACTION;
    let mut median_h = median(&heights).unwrap_or(20.0);
    if median_h < min_text {
        let text_start = heights.partition_point(|&h| h < min_text);
        median_h = heights.get(text_start..).and_then(median).unwrap_or(median_h);
    }
    let max_word_h = (median_h * 2.0).max(12.0);

    let filtered = words.into_iter().filter(|r| word_size(r).is_some_and(|h| h <= max_word_h)).collect();
    (filtered, median_h)
}

/// A detected word's size across its reading direction (the shorter side), when both sides are
/// finite: `f32::min` ignores a NaN, so a NaN-wide word would otherwise pass as its height.
fn word_size(r: &rten_imageproc::RotatedRect) -> Option<f32> {
    let (w, h) = (r.width(), r.height());
    (w.is_finite() && h.is_finite()).then_some(w.min(h))
}

/// Merge co-linear line segments that were split by stamps, seals, or column separators,
/// and align right margins for lines that stop prematurely due to ink from a stamp.
/// Boxes are `[left, top, right, bottom]`; non-finite or empty ones are dropped.
fn merge_colinear_line_boxes(mut raw_boxes: Vec<[f32; 4]>, page_width: u32, median_h: f32) -> Vec<[f32; 4]> {
    raw_boxes.retain(|b| b.iter().all(|v| v.is_finite()) && b[2] > b[0] && b[3] > b[1]);
    raw_boxes.sort_by(|a, b| a[1].total_cmp(&b[1]));
    let mut merged: Vec<[f32; 4]> = Vec::new();
    for b in raw_boxes {
        let b_h = b[3] - b[1];
        let b_mid = (b[1] + b[3]) * 0.5;
        let mut did_merge = false;
        for m in merged.iter_mut() {
            let m_h = m[3] - m[1];
            let m_mid = (m[1] + m[3]) * 0.5;
            let avg_h = (b_h + m_h) * 0.5;
            let v_overlap = m[3].min(b[3]) - m[1].max(b[1]);
            // A box inside the other's vertical band is on its row whatever its height: a run
            // of fill-in dots ("......") on a form line's baseline is a fraction of the line's
            // height, so neither test above holds for it and it became a line of its own.
            let nested = (b[1] >= m[1] && b[3] <= m[3]) || (m[1] >= b[1] && m[3] <= b[3]);
            if nested || v_overlap >= avg_h * 0.4 || (b_mid - m_mid).abs() <= avg_h * 0.35 {
                m[0] = m[0].min(b[0]);
                m[1] = m[1].min(b[1]);
                m[2] = m[2].max(b[2]);
                m[3] = m[3].max(b[3]);
                did_merge = true;
                break;
            }
        }
        if !did_merge {
            merged.push(b);
        }
    }
    merged.sort_by(|a, b| a[1].total_cmp(&b[1]));

    // Align right margin if a full line stops short due to stamp / seal ink.
    let long_line_rights: Vec<f32> = merged.iter().filter(|b| (b[2] - b[0]) > (page_width as f32) * 0.5).map(|b| b[2]).collect();
    if let Some(&max_right) = long_line_rights.iter().max_by(|a, b| a.total_cmp(b)) {
        for b in merged.iter_mut() {
            let span = b[2] - b[0];
            if b[0] < (page_width as f32) * 0.35 && span > (page_width as f32) * 0.35 && b[2] < max_right {
                let diff = max_right - b[2];
                if diff <= median_h * 3.0 {
                    b[2] = max_right;
                }
            }
        }
    }
    merged
}

/// `[x0, y0, x1, y1]` clipped to a `width` × `height` image, or `None` when nothing is left.
fn clip_crop([x0, y0, x1, y1]: [u32; 4], width: u32, height: u32) -> Option<[u32; 4]> {
    let (x1, y1) = (x1.min(width), y1.min(height));
    (x0 < x1 && y0 < y1).then_some([x0, y0, x1, y1])
}

/// Crop `[x0, y0, x1, y1]` from `rgb` (`width` × `height`, 3 or 1 channels), resize to height
/// [`CTC_HEIGHT`] with BGR channels normalized to `[-1.0, 1.0]`, run `model`, and decode with [`ctc::decode`].
fn recognize_ctc_crop(
    rgb: &[u8],
    width: u32,
    height: u32,
    [x0, y0, x1, y1]: [u32; 4],
    model: &rten::Model,
    dict: &[char],
) -> Result<Option<String>, OcrError> {
    let crop_w = x1.saturating_sub(x0) as usize;
    let crop_h = y1.saturating_sub(y0) as usize;
    if crop_w == 0 || crop_h == 0 {
        return Ok(None);
    }
    let dst_w = (crop_w.saturating_mul(CTC_HEIGHT) / crop_h).max(16);
    if dst_w > MAX_CTC_WIDTH {
        return Ok(None);
    }
    // PaddleOCR RecResizeImg pads to at least 320 wide when narrower, or runs dynamic width.
    let padded_w = dst_w.max(160);
    if padded_w > MAX_CTC_WIDTH {
        return Ok(None);
    }
    let plane = CTC_HEIGHT * padded_w;
    let mut tensor = vec![0.0f32; 3 * plane];
    let pixels = resize_crop(rgb, width, height, [x0, y0, x1, y1], dst_w, CTC_HEIGHT);
    for (i, [r, g, b]) in pixels.into_iter().enumerate() {
        // PaddleOCR DecodeImage(BGR) + RecResizeImg: (pixel / 255.0 - 0.5) / 0.5.
        let idx = (i / dst_w) * padded_w + i % dst_w;
        if let Some(slot) = tensor.get_mut(idx) {
            *slot = (b / 255.0 - 0.5) / 0.5;
        }
        if let Some(slot) = tensor.get_mut(plane + idx) {
            *slot = (g / 255.0 - 0.5) / 0.5;
        }
        if let Some(slot) = tensor.get_mut(2 * plane + idx) {
            *slot = (r / 255.0 - 0.5) / 0.5;
        }
    }
    let input = rten::Value::from_shape([1, 3, CTC_HEIGHT, padded_w], tensor).map_err(|e| OcrError::Recognize(e.to_string()))?;
    let output = model.run_one(input.into(), None).map_err(|e| OcrError::Recognize(e.to_string()))?;
    let (shape, probs): ([usize; 3], Vec<f32>) = output.into_shape_vec().map_err(|e| OcrError::Recognize(e.to_string()))?;
    let classes = shape[2];
    Ok(Some(ctc::decode(&probs, classes, dict)))
}

/// The `[x0, y0, x1, y1]` crop of `rgb` (`width` × `height`, 3 or 1 channels) resized to
/// `dst_w` × `dst_h`, row by row, as RGB values from 0 to 255. Enlarged crops (`crop_h < dst_h`)
/// are sampled bilinearly; shrunk crops keep nearest sampling. Pixels outside the image read as
/// white.
fn resize_crop(rgb: &[u8], width: u32, height: u32, [x0, y0, x1, y1]: [u32; 4], dst_w: usize, dst_h: usize) -> Vec<[f32; 3]> {
    let (width, height) = (width as usize, height as usize);
    let channels = if rgb.len() >= width.saturating_mul(height).saturating_mul(3) { 3usize } else { 1usize };
    let crop_w = x1.saturating_sub(x0) as usize;
    let crop_h = y1.saturating_sub(y0) as usize;
    let mut out = Vec::with_capacity(dst_w.saturating_mul(dst_h));
    if crop_w == 0 || crop_h == 0 || dst_w == 0 || dst_h == 0 {
        return out;
    }
    let pixel = |sx: usize, sy: usize| -> [f32; 3] {
        let base = sy.saturating_mul(width).saturating_add(sx).saturating_mul(channels);
        let at = |i: usize| f32::from(rgb.get(i).copied().unwrap_or(255));
        if channels == 3 { [at(base), at(base + 1), at(base + 2)] } else { [at(base); 3] }
    };
    if crop_h < dst_h {
        let map = |d: usize, src: usize, dst: usize, origin: u32, bound: usize| -> (usize, usize, f32) {
            let u = ((d as f32 + 0.5) * (src as f32) / (dst as f32) - 0.5).clamp(0.0, src.saturating_sub(1) as f32);
            let i0 = u.floor() as usize;
            let i1 = (i0 + 1).min(src.saturating_sub(1));
            let s0 = (origin as usize).saturating_add(i0).min(bound.saturating_sub(1));
            let s1 = (origin as usize).saturating_add(i1).min(bound.saturating_sub(1));
            (s0, s1, u - i0 as f32)
        };
        let xs: Vec<(usize, usize, f32)> = (0..dst_w).map(|dx| map(dx, crop_w, dst_w, x0, width)).collect();
        for dy in 0..dst_h {
            let (sy0, sy1, ty) = map(dy, crop_h, dst_h, y0, height);
            for &(sx0, sx1, tx) in &xs {
                let p00 = pixel(sx0, sy0);
                let p10 = pixel(sx1, sy0);
                let p01 = pixel(sx0, sy1);
                let p11 = pixel(sx1, sy1);
                out.push(std::array::from_fn(|c| {
                    let top = p00[c] + (p10[c] - p00[c]) * tx;
                    let bot = p01[c] + (p11[c] - p01[c]) * tx;
                    top + (bot - top) * ty
                }));
            }
        }
        return out;
    }
    for dy in 0..dst_h {
        let sy = (y0 as usize).saturating_add((dy * crop_h) / dst_h).min(height.saturating_sub(1));
        for dx in 0..dst_w {
            let sx = (x0 as usize).saturating_add((dx * crop_w) / dst_w).min(width.saturating_sub(1));
            out.push(pixel(sx, sy));
        }
    }
    out
}

/// A word placed on a page, in PDF user space: the bottom-left corner of its box, and the
/// vectors along its bottom edge (`across`, left → right as read) and its left edge (`up`).
#[derive(Clone, Debug, PartialEq)]
pub struct PlacedWord {
    pub text: String,
    pub origin: [f64; 2],
    pub across: [f64; 2],
    pub up: [f64; 2],
}

impl PlacedWord {
    /// Place a pixel-space word with `to_user`, which maps an image pixel (x right, y down) to
    /// user space.
    pub fn place(word: &Word, to_user: impl Fn(f32, f32) -> [f64; 2]) -> PlacedWord {
        let [l, t, r, b] = word.rect;
        let o = to_user(l, b);
        let br = to_user(r, b);
        let tl = to_user(l, t);
        PlacedWord { text: word.text.clone(), origin: o, across: [br[0] - o[0], br[1] - o[1]], up: [tl[0] - o[0], tl[1] - o[1]] }
    }
}

/// Helvetica's ascender and descender (em fractions): a word box spans about this much.
const BOX_EM: f64 = 0.93;
const DESCENT: f64 = 0.21;
/// Helvetica's cap height and x-height (em fractions).
const CAP_HEIGHT: f64 = 0.718;
const X_HEIGHT: f64 = 0.523;

/// How far `text`'s ink reaches above and below the baseline, in em fractions: a word box is
/// tight around the ink, so "DT" spans the cap height only, "sun" the x-height, and "gap" reaches
/// down to the descender. Text without letters or digits keeps the full ascender-descender span.
fn ink_extent(text: &str) -> (f64, f64) {
    if !text.chars().any(char::is_alphanumeric) {
        return (BOX_EM - DESCENT, -DESCENT);
    }
    let tall = text
        .chars()
        .any(|c| c.is_uppercase() || c.is_ascii_digit() || "bdfhijklt!?/\\()[]{}|'\"#$%&@".contains(c) || (c.is_alphabetic() && !c.is_ascii()));
    let low = text.chars().any(|c| "gjpqy()[]{}|_".contains(c));
    (if tall { CAP_HEIGHT } else { X_HEIGHT }, if low { -DESCENT } else { 0.0 })
}

fn num(v: f64) -> String {
    let s = format!("{:.4}", if v.abs() < 1e-9 { 0.0 } else { v });
    let s = s.trim_end_matches('0').trim_end_matches('.');
    if s == "-0" { "0".into() } else { s.into() }
}

/// Page content that writes `words` as invisible text in `/PCHelv` (standard Helvetica,
/// WinAnsiEncoding), each word stretched to its box so selection and search highlight the
/// right place. Marked content `/OCR` so it can be told apart from the page's own text.
pub fn text_layer(words: &[PlacedWord]) -> Vec<u8> {
    // `3 Tr` and the font are graphics state, which `ET` doesn't reset: without `q … Q` they
    // would stay set for every later content stream on the page, so text added after OCR (a
    // header or footer) drew invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n/PCHelv 1 Tf\n".to_vec();
    for w in words {
        let width = helvetica_width(&w.text, 1.0);
        let height = w.up[0].hypot(w.up[1]);
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        // The em square: the word's own ink (cap height or x-height, descenders or not) spans the
        // box, so the text is as tall as the scanned word; its width is stretched to fit the word.
        let (top, bottom) = ink_extent(&w.text);
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        let ax = [w.across[0] / width, w.across[1] / width];
        let o = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];
        let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
        let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
        out.extend_from_slice(nums.join(" ").as_bytes());
        out.extend_from_slice(b" Tm ");
        out.extend(pdfcraft_fonts::literal(&pdfcraft_fonts::win_ansi(&w.text)));
        out.extend_from_slice(b" Tj\n");
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    out
}

/// One generated font of an invisible Unicode OCR layer: its resource name (`PCOcrU1`, `PCOcrU2`, …)
/// and the characters mapped to codes `1..=chars.len()` (at most 255).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LayerFont {
    pub name: String,
    pub chars: Vec<char>,
}

/// Combining marks that sit on the preceding base character and advance the pen by 0.
/// Kept in sync with `pdfcraft_edit::invisible_font::is_zero_width`.
fn is_zero_width(c: char) -> bool {
    matches!(u32::from(c), 0x0300..=0x036F | 0x0E31 | 0x0E34..=0x0E3A | 0x0E47..=0x0E4E)
}

/// Page content that writes `words` as invisible text in generated 1-byte fonts (`PCOcrU1`, …),
/// so scripts outside WinAnsiEncoding keep their Unicode text for search and copy.
pub fn unicode_text_layer(words: &[PlacedWord]) -> (Vec<u8>, Vec<LayerFont>) {
    const PER_FONT: usize = 255;
    let mut fonts: Vec<LayerFont> = Vec::new();
    let mut map: std::collections::HashMap<char, (usize, u8)> = std::collections::HashMap::new();
    for w in words {
        for ch in w.text.chars() {
            if map.contains_key(&ch) {
                continue;
            }
            let need_new = fonts.last().is_none_or(|f| f.chars.len() >= PER_FONT);
            if need_new {
                let n = fonts.len().saturating_add(1);
                fonts.push(LayerFont { name: format!("PCOcrU{n}"), chars: Vec::new() });
            }
            let font_idx = fonts.len().saturating_sub(1);
            if let Some(font) = fonts.get_mut(font_idx) {
                font.chars.push(ch);
                let code = u8::try_from(font.chars.len()).unwrap_or(255);
                map.insert(ch, (font_idx, code));
            }
        }
    }

    // `3 Tr` and fonts are graphics state: wrap in `q … Q` so text added after OCR is not drawn invisibly (#3029).
    let mut out = b"/OCR BMC\nq\nBT\n3 Tr\n".to_vec();
    let mut current_font: Option<usize> = None;
    let (top, bottom) = (BOX_EM - DESCENT, -DESCENT);
    for w in words {
        let base_count = w.text.chars().filter(|c| !is_zero_width(*c)).count();
        let width = (base_count.max(1) as f64) * 0.5;
        let height = w.up[0].hypot(w.up[1]);
        if width <= 0.0 || height <= 0.0 || w.text.is_empty() {
            continue;
        }
        let em = [w.up[0] / (top - bottom), w.up[1] / (top - bottom)];
        let ax = [w.across[0] / width, w.across[1] / width];
        let base_origin = [w.origin[0] - em[0] * bottom, w.origin[1] - em[1] * bottom];

        let mut runs: Vec<(usize, Vec<u8>, usize)> = Vec::new();
        for ch in w.text.chars() {
            let Some(&(font_idx, code)) = map.get(&ch) else { continue };
            let adv = usize::from(!is_zero_width(ch));
            match runs.last_mut() {
                Some((last_font, codes, count)) if *last_font == font_idx => {
                    codes.push(code);
                    *count = count.saturating_add(adv);
                }
                _ => runs.push((font_idx, vec![code], adv)),
            }
        }

        let mut advance_em = 0.0;
        for (font_idx, codes, run_bases) in runs {
            if current_font != Some(font_idx) {
                if let Some(font) = fonts.get(font_idx) {
                    out.extend_from_slice(format!("/{} 1 Tf\n", font.name).as_bytes());
                }
                current_font = Some(font_idx);
            }
            let o = [base_origin[0] + ax[0] * advance_em, base_origin[1] + ax[1] * advance_em];
            let m = [ax[0], ax[1], em[0], em[1], o[0], o[1]];
            let nums: Vec<String> = m.iter().map(|v| num(*v)).collect();
            out.extend_from_slice(nums.join(" ").as_bytes());
            out.extend_from_slice(b" Tm <");
            for code in codes {
                out.extend_from_slice(format!("{code:02X}").as_bytes());
            }
            out.extend_from_slice(b"> Tj\n");
            advance_em += (run_bases as f64) * 0.5;
        }
    }
    out.extend_from_slice(b"ET\nQ\nEMC\n");
    (out, fonts)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A crop shorter than the recognizer's 48 rows (a line on a ~96 dpi scan) is enlarged
    /// smoothly: a black-to-white edge gets grey rows between, not a staircase of copied rows.
    #[test]
    fn an_enlarged_crop_is_sampled_smoothly() {
        // 4 x 2, one channel: a black row over a white row.
        let img = [0, 0, 0, 0, 255, 255, 255, 255];
        let out = resize_crop(&img, 4, 2, [0, 0, 4, 2], 96, 48);
        assert_eq!(out.len(), 96 * 48);
        let column: Vec<f32> = (0..48).map(|y| out[y * 96][0]).collect();
        assert!(column.iter().any(|v| *v > 10.0 && *v < 245.0), "no grey between the rows: {column:?}");
        assert!(column.windows(2).all(|w| w[0] <= w[1]), "not monotonic: {column:?}");
        assert_eq!((column[0], column[47]), (0.0, 255.0));
        // Three channels, and a crop inside a larger image, read the same way.
        let rgb: Vec<u8> = [9, 9, 9, 9, 0, 0, 255, 255, 9, 9, 9, 9].iter().flat_map(|v| [*v; 3]).collect();
        let out = resize_crop(&rgb, 2, 6, [0, 2, 2, 4], 4, 48);
        assert_eq!(out[0], [0.0; 3]);
        assert_eq!(out[47 * 4], [255.0; 3]);
        assert!(out.iter().any(|p| p[0] > 10.0 && p[0] < 245.0));
    }

    /// A crop taller than 48 rows keeps nearest sampling (blur and area resampling did not help
    /// there): every value is one of the source pixels, at the same place as before.
    #[test]
    fn a_shrunk_crop_keeps_nearest_sampling() {
        let (w, h) = (7u32, 100u32);
        let img: Vec<u8> = (0..w * h).map(|i| (i * 37 % 251) as u8).collect();
        let out = resize_crop(&img, w, h, [1, 3, 6, 99], 10, 48);
        for (i, p) in out.iter().enumerate() {
            let (dx, dy) = (i % 10, i / 10);
            let (sx, sy) = (1 + dx * 5 / 10, 3 + dy * 96 / 48);
            assert_eq!(p[0], f32::from(img[sy * w as usize + sx]), "at {dx},{dy}");
        }
        // An empty crop or an out-of-range one never panics.
        assert!(resize_crop(&img, w, h, [3, 3, 3, 9], 10, 48).is_empty());
        assert_eq!(resize_crop(&img, w, h, [5, 90, 900, 9000], 4, 48).len(), 4 * 48);
        assert_eq!(resize_crop(&[], 0, 0, [0, 0, 2, 2], 4, 48).len(), 4 * 48);
    }

    #[test]
    fn places_words_through_the_mapping() {
        // 2 pixels per point, page 612 × 792, y flipped.
        let to_user = |x: f32, y: f32| [x as f64 / 2.0, 792.0 - y as f64 / 2.0];
        let w = PlacedWord::place(&Word { text: "Hello".into(), rect: [100.0, 200.0, 300.0, 240.0] }, to_user);
        assert_eq!(w.origin, [50.0, 672.0]);
        assert_eq!(w.across, [100.0, 0.0]);
        assert_eq!(w.up, [0.0, 20.0]);
    }

    /// The Latin text layer is Helvetica in WinAnsiEncoding: every character the Latin model can
    /// recognise must be one it can hold, or recognised text on that path would be saved as `?`.
    /// Models for other scripts use [`unicode_text_layer`] instead.
    #[test]
    fn the_model_alphabet_fits_the_text_layer_font() {
        assert_eq!(pdfcraft_fonts::first_undrawable(ALPHABET), None);
    }

    /// SRS AC-10: broken model files are load errors, never a panic.
    #[test]
    fn broken_model_files_are_load_errors() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-broken-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let junk = dir.join("junk.rten");
        std::fs::write(&junk, b"RTEN\x01\x00\x00\x00not a model").unwrap();
        let fails = |m: &Models| matches!(Ocr::load(m), Err(OcrError::Load(..)));
        assert!(fails(&Models { detection: junk.clone(), recognition: junk.clone(), dictionary: None }));
        if let Some(thai) = Models::find_for("th") {
            let half = dir.join("half.rten");
            let model = std::fs::read(&thai.recognition).unwrap();
            std::fs::write(&half, model.get(..model.len() / 2).unwrap()).unwrap();
            assert!(fails(&Models { recognition: half, ..thai.clone() }), "a model cut in half");
            let dict = dir.join("dict.txt");
            std::fs::write(&dict, [0xff, 0xfe, 0x80, 0x0a]).unwrap();
            assert!(fails(&Models { dictionary: Some(dict), ..thai.clone() }), "a dictionary that isn't UTF-8");
        } else {
            eprintln!("skipped the truncated-model cases: Thai OCR models not installed");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn missing_language_model_names_it() {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-lang-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        for file in [DETECTION_MODEL, RECOGNITION_MODEL] {
            std::fs::write(dir.join(file), b"model").unwrap();
        }
        assert!(Models::in_dir_for(&dir, "en").is_some());
        assert!(Models::in_dir_for(&dir, "th").is_none(), "English files alone don't make Thai");
        let msg = OcrError::NoLanguageModel("th".into()).to_string();
        assert!(msg.contains("th") && msg.contains("PDFCRAFT_MODELS") && !msg.contains("cargo"), "{msg}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn unicode_layer_mixes_scripts() {
        let w = |t: &str, x: f64| PlacedWord { text: t.into(), origin: [x, 100.0], across: [40.0, 0.0], up: [0.0, 12.0] };
        let (content, fonts) = unicode_text_layer(&[w("ใบแจ้งหนี้", 10.0), w("No.", 60.0), w("2569/001", 90.0)]);
        let s = String::from_utf8_lossy(&content);
        assert!(s.starts_with("/OCR BMC") && s.contains("3 Tr") && s.contains("/PCOcrU1 1 Tf"), "{s}");
        assert_eq!(fonts.len(), 1);
        let all: String = fonts[0].chars.iter().collect();
        for c in "ใบแจ้งหนี้No.2569/01".chars() {
            assert!(all.contains(c), "{c} missing from {all}");
        }
        assert_eq!(fonts[0].chars.iter().filter(|&&c| c == 'น').count(), 1, "each char gets one code");
    }

    #[test]
    fn unicode_layer_splits_fonts_past_255_codes() {
        let text: String = (0x4E00u32..0x4E00 + 300).filter_map(char::from_u32).collect();
        let w = PlacedWord { text, origin: [0.0, 0.0], across: [500.0, 0.0], up: [0.0, 10.0] };
        let (content, fonts) = unicode_text_layer(&[w]);
        assert_eq!(fonts.len(), 2);
        assert_eq!(fonts[0].chars.len(), 255);
        assert_eq!(fonts[1].chars.len(), 45);
        assert!(String::from_utf8_lossy(&content).contains("/PCOcrU2 1 Tf"));
    }

    /// #3029 for the Unicode layer: its `3 Tr` and fonts end with the layer, so text added to the
    /// page after OCR is not drawn invisibly.
    #[test]
    fn unicode_layer_restores_the_graphics_state_it_changes() {
        let w = PlacedWord { text: "น้ำ".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
        let (content, _) = unicode_text_layer(&[w]);
        let s = String::from_utf8(content).unwrap();
        let mut depth = 0_i32;
        for token in s.split_ascii_whitespace() {
            match token {
                "q" => depth += 1,
                "Q" => depth -= 1,
                "Tr" | "Tf" | "Tm" => assert!(depth > 0, "`{token}` outside q … Q in {s:?}"),
                _ => {}
            }
            assert!(depth >= 0, "unbalanced Q in {s:?}");
        }
        assert_eq!(depth, 0, "the layer leaves a q open: {s:?}");
    }

    #[test]
    fn text_layer_is_invisible_text_fitted_to_each_box() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        assert!(s.contains("3 Tr") && s.contains("/PCHelv 1 Tf"), "{s}");
        let width = helvetica_width("Hi", 1.0);
        // "Hi" has no descender: its box spans the cap height, from the baseline up.
        assert!(s.contains(&format!("{} 0 0 {} 10 20 Tm (Hi) Tj", num(30.0 / width), num(9.3 / CAP_HEIGHT))), "{s}");
        assert!(s.starts_with("/OCR BMC") && s.ends_with("EMC\n"));
    }

    /// #3029: the layer's invisible rendering mode and font end with the layer. Every operator
    /// that sets them sits inside a `q … Q` pair that the layer closes itself, so content drawn
    /// after it on the page starts from the graphics state it had before.
    #[test]
    fn the_layer_restores_the_graphics_state_it_changes() {
        let w = PlacedWord { text: "Hi".into(), origin: [10.0, 20.0], across: [30.0, 0.0], up: [0.0, 9.3] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        let mut depth = 0_i32;
        for token in s.split_ascii_whitespace() {
            match token {
                "q" => depth += 1,
                "Q" => depth -= 1,
                "Tr" | "Tf" | "Tm" => assert!(depth > 0, "`{token}` outside q … Q in {s:?}"),
                _ => {}
            }
            assert!(depth >= 0, "unbalanced Q in {s:?}");
        }
        assert_eq!(depth, 0, "the layer leaves a q open: {s:?}");
    }

    /// A word box is tight around the ink, so the text's size and baseline come from the letters:
    /// capitals, lowercase without ascenders, descenders.
    #[test]
    fn the_text_is_as_tall_as_the_scanned_word() {
        assert_eq!(ink_extent("DT"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("402337"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("sun"), (X_HEIGHT, 0.0));
        assert_eq!(ink_extent("gap"), (X_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("Gallus.Ze@camtel.cm"), (CAP_HEIGHT, 0.0));
        assert_eq!(ink_extent("Corporation"), (CAP_HEIGHT, -DESCENT));
        assert_eq!(ink_extent("--"), (BOX_EM - DESCENT, -DESCENT));
        let w = PlacedWord { text: "gap".into(), origin: [0.0, 100.0], across: [20.0, 0.0], up: [0.0, 7.3] };
        let s = String::from_utf8(text_layer(&[w])).unwrap();
        let em = 7.3 / (X_HEIGHT + DESCENT);
        assert!(s.contains(&format!(" 0 0 {} 0 {} Tm (gap) Tj", num(em), num(100.0 + em * DESCENT))), "{s}");
    }

    #[test]
    fn rotated_pages_keep_the_reading_direction() {
        // A page turned 90°: image x runs up the page.
        let to_user = |x: f32, y: f32| [y as f64, x as f64];
        let w = PlacedWord::place(&Word { text: "Up".into(), rect: [0.0, 0.0, 50.0, 10.0] }, to_user);
        assert_eq!(w.across, [0.0, 50.0]);
        assert_eq!(w.up, [-10.0, 0.0]);
    }

    /// #103: every release package's layout puts the models where the installed app looks.
    /// Mirrors packaging/{windows/package.ps1,macos/package.sh,linux/package.sh,freebsd/package.sh}.
    #[test]
    fn finds_the_models_in_every_release_package_layout() {
        let base = std::env::temp_dir().join(format!("pdfcraft-ocr-layouts-{}", std::process::id()));
        let layouts = [
            ("windows", "PdfCraft", "PdfCraft/models"),
            ("macos", "PdfCraft.app/Contents/MacOS", "PdfCraft.app/Contents/Resources/models"),
            ("linux-deb-rpm", "usr/bin", "usr/share/pdfcraft/models"),
            ("linux-tar", "pdfcraft-1.0.0-linux-x86_64/bin", "pdfcraft-1.0.0-linux-x86_64/share/pdfcraft/models"),
            ("appimage", "PdfCraft.AppDir/usr/bin", "PdfCraft.AppDir/usr/share/pdfcraft/models"),
            ("flatpak", "app/bin", "app/share/pdfcraft/models"),
            ("freebsd", "usr/local/bin", "usr/local/share/pdfcraft/models"),
        ];
        for (name, exe_dir, models_dir) in layouts {
            let root = base.join(name);
            let (exe_dir, models_dir) = (root.join(exe_dir), root.join(models_dir));
            std::fs::create_dir_all(&exe_dir).unwrap();
            std::fs::create_dir_all(&models_dir).unwrap();
            let found = || Models::dirs_beside_exe(&exe_dir).iter().find_map(|d| Models::in_dir(d));
            assert_eq!(found(), None, "{name}: no models yet");
            for file in [DETECTION_MODEL, RECOGNITION_MODEL] {
                std::fs::write(models_dir.join(file), b"model").unwrap();
            }
            let models = found().unwrap_or_else(|| panic!("{name}: models in {} not found", models_dir.display()));
            assert_eq!(models.detection.canonicalize().unwrap(), models_dir.join(DETECTION_MODEL).canonicalize().unwrap(), "{name}");
        }
        let _ = std::fs::remove_dir_all(&base);
    }

    /// The message tells end users what to do; release builds have no cargo.
    #[test]
    fn the_alphabet_is_the_trained_one() {
        let chars: Vec<char> = ALPHABET.chars().collect();
        assert_eq!(chars.len(), 96);
        // The euro sign follows the ASCII punctuation (class 44; 0 is the CTC blank), not a second E.
        assert_eq!(chars.iter().position(|&c| c == '€'), Some(43));
        assert_eq!(chars.iter().filter(|&&c| c == 'E').count(), 1);
    }

    #[test]
    fn missing_models_message_is_for_end_users() {
        let message = OcrError::NoModels.to_string();
        assert!(!message.contains("cargo") && message.contains("PDFCRAFT_MODELS"), "{message}");
    }

    #[test]
    fn seal_filter_removes_oversized_components_while_keeping_words() {
        use rten_imageproc::{PointF, RotatedRect, Vec2};
        let make_word = |w: f32, h: f32| RotatedRect::new(PointF::from_yx(0.0, 0.0), Vec2::from_yx(-1.0, 0.0), w, h);
        let words = vec![
            make_word(100.0, 30.0),
            make_word(120.0, 32.0),
            make_word(80.0, 28.0),
            make_word(400.0, 150.0), // Giant stamp/seal
            make_word(90.0, 31.0),
        ];
        let (filtered, median_h) = filter_seal_words(words, 1000);
        assert!((median_h - 31.0).abs() < 2.0);
        assert_eq!(filtered.len(), 4);
        assert!(filtered.iter().all(|r| r.height().min(r.width()) <= 65.0));
    }

    #[test]
    fn merge_colinear_line_boxes_combines_split_segments_and_aligns_margin() {
        let raw = vec![
            [100.0, 100.0, 400.0, 130.0], // Line 1: left piece
            [600.0, 102.0, 900.0, 132.0], // Line 1: right piece (separated by seal)
            [100.0, 150.0, 900.0, 180.0], // Line 2: normal full line
            [100.0, 200.0, 850.0, 230.0], // Line 3: cut slightly short by seal
        ];
        let merged = merge_colinear_line_boxes(raw, 1000, 30.0);
        assert_eq!(merged.len(), 3);
        // Line 1 merged across the gap
        assert_eq!(merged[0][0], 100.0);
        assert_eq!(merged[0][2], 900.0);
        // Line 3 right margin aligned to 900.0
        assert_eq!(merged[2][2], 900.0);
    }

    /// A form line at 300 dpi (2413 px wide, an A4-ish page), grey: a word, a leader of 20 dots
    /// after it (boxes `[420 + 18 i, 150, 425 + 18 i, 155]`), three marks sitting on letters, a
    /// lone pair of dots, and a mark broken into three touching pieces.
    fn leader_line() -> (usize, usize, Vec<u8>) {
        let (w, h) = (2413usize, 200usize);
        let mut img = vec![255u8; w * h];
        let mut ink = |x0: usize, y0: usize, x1: usize, y1: usize| {
            for y in y0..y1 {
                img[y * w + x0..y * w + x1].fill(0);
            }
        };
        ink(100, 100, 400, 160); // a word
        for i in 0..20 {
            ink(420 + i * 18, 150, 425 + i * 18, 155); // the leader, on the word's baseline
        }
        for i in 0..3 {
            ink(900 + i * 20, 100, 915 + i * 20, 160); // three letters
            ink(905 + i * 20, 90, 910 + i * 20, 95); // a tone mark over each
        }
        ink(1500, 150, 1505, 155); // two dots, not a leader
        ink(1520, 150, 1525, 155);
        // A mark broken into pieces by JPEG (a 72 dpi gazette page): pieces 1 px apart are
        // not a row of dots.
        ink(1700, 50, 1703, 53);
        ink(1704, 50, 1707, 53);
        ink(1708, 50, 1711, 53);
        (w, h, img)
    }

    /// Only the leader of [`leader_line`] is whitened.
    #[test]
    fn dot_leaders_are_erased_and_marks_kept() {
        let (w, h, img) = leader_line();
        let dark = |img: &[u8], x0: usize, y0: usize, x1: usize, y1: usize| (y0..y1).any(|y| img[y * w + x0..y * w + x1].iter().any(|&v| v < 128));

        let out = erase_dot_leaders(&img, w as u32, h as u32).expect("a leader to erase");
        assert!(!dark(&out, 418, 148, 785, 157), "the leader is gone");
        assert!(dark(&out, 100, 100, 400, 160) && out[130 * w + 250] == 0, "the word stays");
        assert!(dark(&out, 905, 90, 910, 95) && dark(&out, 945, 90, 950, 95), "marks on letters stay");
        assert!(dark(&out, 1500, 150, 1525, 155), "a pair of dots stays");
        assert!(dark(&out, 1700, 50, 1703, 53) && dark(&out, 1708, 50, 1711, 53), "touching pieces stay");
        // RGB reads the same.
        let rgb: Vec<u8> = img.iter().flat_map(|&v| [v; 3]).collect();
        let out = erase_dot_leaders(&rgb, w as u32, h as u32).expect("a leader to erase");
        assert_eq!(out.len(), rgb.len());
        assert_eq!(out[(152 * w + 422) * 3..(152 * w + 422) * 3 + 3], [255; 3]);
        // Nothing to erase, or nothing usable, is None and never a panic.
        assert!(erase_dot_leaders(&vec![255; w * h], w as u32, h as u32).is_none());
        assert!(erase_dot_leaders(&[], 0, 0).is_none());
        assert!(erase_dot_leaders(&[0; 7], 3, 3).is_none());
        assert!(erase_dot_leaders(&[0], 1, 1).is_none());
        assert!(erase_dot_leaders(&[0; 4], u32::MAX, u32::MAX).is_none());
    }

    #[test]
    fn dot_leaders_are_reported_with_their_boxes_and_dot_counts() {
        let (w, h, img) = leader_line();
        let found = dot_leaders(&img, w as u32, h as u32).expect("a grey image");
        assert_eq!(found, [DotLeader { rect: [420.0, 150.0, 767.0, 155.0], dots: 20 }], "the pair and the broken mark are not leaders");
        // RGB and RGBA read the same.
        let rgb: Vec<u8> = img.iter().flat_map(|&v| [v; 3]).collect();
        let rgba: Vec<u8> = img.iter().flat_map(|&v| [v, v, v, 255]).collect();
        assert_eq!(dot_leaders(&rgb, w as u32, h as u32).unwrap(), found);
        assert_eq!(dot_leaders(&rgba, w as u32, h as u32).unwrap(), found);
        // The eraser whitens exactly the dots found, 2 px around each (what it did before the finder was split out).
        let mut expected = img.clone();
        for i in 0..20usize {
            for y in 148..157 {
                expected[y * w + 418 + i * 18..y * w + 427 + i * 18].fill(255);
            }
        }
        assert_eq!(erase_dot_leaders(&img, w as u32, h as u32).expect("a leader to erase"), expected);
        // Nothing to find is empty; a buffer that is not the image is an error, never a panic.
        assert!(dot_leaders(&vec![255; w * h], w as u32, h as u32).unwrap().is_empty());
        assert!(dot_leaders(&[], 0, 0).is_err());
        assert!(dot_leaders(&[0; 7], 3, 3).is_err());
        assert!(dot_leaders(&[0; 4], u32::MAX, u32::MAX).is_err());
    }

    #[test]
    fn each_row_of_dots_is_its_own_leader_top_to_bottom() {
        let (w, h) = (2413usize, 120usize);
        let mut img = vec![255u8; w * h];
        // Three dots are the fewest that make a leader; a pair is not (see `leader_line`).
        for (y, x0, count) in [(30usize, 200usize, 8usize), (30, 1400, 4), (80, 200, 30), (100, 800, 3), (100, 1200, 2)] {
            for i in 0..count {
                for yy in y..y + 5 {
                    img[yy * w + x0 + i * 18..yy * w + x0 + i * 18 + 5].fill(0);
                }
            }
        }
        let found = dot_leaders(&img, w as u32, h as u32).unwrap();
        let rows: Vec<(f32, f32, usize)> = found.iter().map(|l| (l.rect[1], l.rect[0], l.dots)).collect();
        assert_eq!(rows, [(30.0, 200.0, 8), (30.0, 1400.0, 4), (80.0, 200.0, 30), (100.0, 800.0, 3)]);
    }

    fn rotated(w: f32, h: f32) -> rten_imageproc::RotatedRect {
        use rten_imageproc::{PointF, RotatedRect, Vec2};
        RotatedRect::new(PointF::from_yx(0.0, 0.0), Vec2::from_yx(-1.0, 0.0), w, h)
    }

    #[test]
    fn seal_filter_handles_empty_and_single_inputs() {
        let (none, median) = filter_seal_words(Vec::new(), 1000);
        assert!(none.is_empty());
        assert_eq!(median, 20.0);
        let (one, median) = filter_seal_words(vec![rotated(300.0, 150.0)], 1000);
        assert_eq!(one.len(), 1, "a lone component is the norm, not a seal");
        assert_eq!(median, 150.0);
    }

    #[test]
    fn seal_filter_drops_non_finite_sizes_and_keeps_zero_sized_words() {
        let words =
            vec![rotated(f32::NAN, 30.0), rotated(100.0, f32::INFINITY), rotated(0.0, 0.0), rotated(100.0, 30.0), rotated(f32::MAX, f32::MAX)];
        let (kept, median) = filter_seal_words(words, 1000);
        assert!(median.is_finite(), "{median}");
        assert!(kept.iter().all(|r| r.height().is_finite() && r.width().is_finite()), "non-finite sizes never reach line grouping");
        assert!(kept.iter().any(|r| r.width() == 0.0), "a zero-sized word is under the cap");
        assert!(kept.iter().all(|r| r.width() < f32::MAX), "the f32::MAX component is oversized");
    }

    #[test]
    fn seal_filter_keeps_everything_when_all_components_are_large() {
        // Large print: every word is "oversized" against a small-text page, but they are the
        // page's own median, so nothing is a seal.
        let words: Vec<_> = (0..7).map(|i| rotated(400.0, 150.0 + i as f32)).collect();
        let (kept, median) = filter_seal_words(words, 1000);
        assert_eq!(kept.len(), 7);
        assert_eq!(median, 153.0);
    }

    #[test]
    fn seal_filter_sorts_many_nan_sizes_without_panicking() {
        // A NaN in a `partial_cmp`-based sort broke the total order `sort_by` requires (it may
        // panic, though this input does not trip the check on every toolchain).
        let words: Vec<_> = (0..200).map(|i| if i % 3 == 0 { rotated(f32::NAN, f32::NAN) } else { rotated(100.0, 20.0 + (i % 17) as f32) }).collect();
        let (kept, median) = filter_seal_words(words, 1000);
        assert!((20.0..=37.0).contains(&median), "{median}");
        assert!(kept.iter().all(|r| !r.height().is_nan()));
    }

    #[test]
    fn merge_line_boxes_handles_empty_single_and_degenerate_boxes() {
        assert!(merge_colinear_line_boxes(Vec::new(), 1000, 30.0).is_empty());
        assert_eq!(merge_colinear_line_boxes(vec![[10.0, 10.0, 200.0, 40.0]], 1000, 30.0), vec![[10.0, 10.0, 200.0, 40.0]]);
        let junk = vec![
            [f32::NAN, 0.0, 10.0, 10.0],
            [0.0, 0.0, f32::INFINITY, 10.0],
            [5.0, 5.0, 5.0, 20.0],   // zero width
            [5.0, 20.0, 50.0, 10.0], // inverted
            [10.0, 10.0, 200.0, 40.0],
        ];
        assert_eq!(merge_colinear_line_boxes(junk, 1000, 30.0), vec![[10.0, 10.0, 200.0, 40.0]]);
    }

    #[test]
    fn merge_line_boxes_absorbs_nested_and_overlapping_duplicates() {
        let raw = vec![[100.0, 100.0, 900.0, 140.0], [300.0, 105.0, 500.0, 135.0], [120.0, 101.0, 880.0, 139.0], [100.0, 200.0, 900.0, 240.0]];
        assert_eq!(merge_colinear_line_boxes(raw, 1000, 30.0), vec![[100.0, 100.0, 900.0, 140.0], [100.0, 200.0, 900.0, 240.0]]);
    }

    #[test]
    fn merge_line_boxes_takes_extreme_page_widths_and_medians() {
        let raw = vec![[0.0, 0.0, 100.0, 30.0], [0.0, 50.0, 90.0, 80.0]];
        for (width, median) in [(0, 30.0), (u32::MAX, 30.0), (1000, f32::NAN), (1000, f32::INFINITY), (1000, 0.0), (1000, -5.0)] {
            let merged = merge_colinear_line_boxes(raw.clone(), width, median);
            assert_eq!(merged.len(), 2, "{width} {median}");
            assert!(merged.iter().flatten().all(|v| v.is_finite()));
        }
        // On a zero-width page every line is "long" and starts at the left margin, but nothing
        // aligns to a right edge farther away than three median heights.
        let merged = merge_colinear_line_boxes(raw.clone(), 0, 1.0);
        assert_eq!(merged[1][2], 90.0);
    }

    #[test]
    fn crops_are_clipped_to_the_image() {
        assert_eq!(clip_crop([10, 10, 20, 20], 100, 100), Some([10, 10, 20, 20]));
        assert_eq!(clip_crop([10, 10, 500, 500], 100, 100), Some([10, 10, 100, 100]));
        assert_eq!(clip_crop([20, 10, 10, 20], 100, 100), None, "inverted");
        assert_eq!(clip_crop([10, 10, 10, 20], 100, 100), None, "empty");
        assert_eq!(clip_crop([100, 0, u32::MAX, u32::MAX], 100, 100), None, "starts past the edge");
        assert_eq!(clip_crop([0, 0, u32::MAX, u32::MAX], u32::MAX, u32::MAX), Some([0, 0, u32::MAX, u32::MAX]));
    }

    /// The recogniser's crop maths with an inverted rectangle (`x1 < x0` underflowed) and an
    /// enormous one, when the Thai models are installed.
    #[test]
    fn recognize_crop_rejects_inverted_and_huge_rectangles() {
        let Some(models) = Models::find_for("th") else {
            eprintln!("skipped: Thai OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        let px = vec![255u8; 64 * 32 * 3];
        assert_eq!(ocr.recognize_crop(&px, 64, 32, [40, 0, 10, 32]).unwrap(), None);
        assert_eq!(ocr.recognize_crop(&px, 64, 32, [0, 40, 64, 10]).unwrap(), None);
        assert!(ocr.recognize_crop(&px, 64, 32, [0, 0, u32::MAX, u32::MAX]).is_ok());
    }

    proptest::proptest! {
        #[test]
        fn seal_filter_keeps_a_subset_under_the_cap(sizes in proptest::collection::vec((0.0f32..2000.0, 0.0f32..2000.0), 0..60)) {
            let words: Vec<_> = sizes.iter().map(|&(w, h)| rotated(w, h)).collect();
            let (kept, median) = filter_seal_words(words, 1000);
            proptest::prop_assert!(kept.len() <= sizes.len());
            proptest::prop_assert!(median.is_finite());
            proptest::prop_assert!(sizes.is_empty() || !kept.is_empty(), "the median word always survives");
            let cap = (median * 2.0).max(12.0);
            proptest::prop_assert!(kept.iter().all(|r| r.height().min(r.width()) <= cap));
        }

        #[test]
        fn merged_line_boxes_cover_every_input_box(
            boxes in proptest::collection::vec((-100.0f32..3000.0, -100.0f32..3000.0, 0.0f32..1500.0, 0.0f32..200.0), 0..40),
            width in proptest::prelude::any::<u32>(),
            median in -10.0f32..200.0,
        ) {
            let raw: Vec<[f32; 4]> = boxes.iter().map(|&(x, y, w, h)| [x, y, x + w, y + h]).collect();
            let merged = merge_colinear_line_boxes(raw.clone(), width, median);
            let valid: Vec<_> = raw.iter().filter(|b| b[2] > b[0] && b[3] > b[1]).collect();
            proptest::prop_assert!(merged.len() <= valid.len());
            proptest::prop_assert!(merged.iter().all(|m| m.iter().all(|v| v.is_finite()) && m[2] > m[0] && m[3] > m[1]));
            proptest::prop_assert!(merged.windows(2).all(|p| p[0][1] <= p[1][1]), "sorted top to bottom");
            for b in valid {
                proptest::prop_assert!(merged.iter().any(|m| m[0] <= b[0] && m[1] <= b[1] && m[2] >= b[2] && m[3] >= b[3]), "{b:?} lost");
            }
        }

        #[test]
        fn dot_leaders_stay_inside_the_image_whatever_it_holds(
            w in 1u32..80,
            h in 1u32..40,
            channels in proptest::prop_oneof![proptest::strategy::Just(1usize), proptest::strategy::Just(3), proptest::strategy::Just(4)],
            seed in proptest::collection::vec(0u8..=255, 1..64),
        ) {
            let len = w as usize * h as usize * channels;
            let pixels: Vec<u8> = (0..len).map(|i| seed.get(i % seed.len()).copied().unwrap_or(0).wrapping_mul(i as u8 | 1)).collect();
            let found = dot_leaders(&pixels, w, h).expect("a buffer that matches the image");
            for l in found {
                proptest::prop_assert!(l.dots >= MIN_LEADER_DOTS);
                proptest::prop_assert!(l.rect[0] >= 0.0 && l.rect[1] >= 0.0 && l.rect[0] < l.rect[2] && l.rect[1] < l.rect[3]);
                proptest::prop_assert!(l.rect[2] <= w as f32 && l.rect[3] <= h as f32);
            }
        }
    }

    #[test]
    fn pixel_buffers_must_match_their_dimensions() {
        assert!(matches!(channels_for_ocrs(&[], 2, 2), Err(OcrError::EmptyImage)));
        assert!(matches!(channels_for_ocrs(&[0; 4], 0, 2), Err(OcrError::EmptyImage)));
        assert_eq!(channels_for_ocrs(&[1, 2, 3, 4], 2, 2).unwrap().as_ref(), &[1, 2, 3, 4], "grey");
        assert_eq!(channels_for_ocrs(&[9; 12], 2, 2).unwrap().len(), 12, "RGB");
        assert_eq!(channels_for_ocrs(&[1, 2, 3, 255, 4, 5, 6, 255], 2, 1).unwrap().as_ref(), &[1, 2, 3, 4, 5, 6], "RGBA");
        for (len, w, h) in [(5, 2, 2), (8, 2, 2), (13, 2, 2), (65536 * 3, 65536, 65537), (16, u32::MAX, u32::MAX), (16, u32::MAX, 1)] {
            assert!(matches!(channels_for_ocrs(&vec![0; len], w, h), Err(OcrError::Recognize(_))), "{len} bytes for {w} × {h}");
        }
    }

    /// Dimensions whose product overflows `u32` reached `ocrs` unchecked, which panicked in its
    /// `width * height`. Runs when the models are installed.
    #[test]
    fn recognize_refuses_dimensions_that_overflow() {
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        let px = vec![255u8; 65536 * 3];
        assert!(ocr.recognize(&px, 65536, 65537).is_err());
        assert!(ocr.recognize(&px, 65537, 65536).is_err());
        assert!(ocr.recognize_crop(&px, 65536, 65537, [0, 0, 10, 10]).is_err());
    }

    /// Images one pixel wide or tall, and thin strips, read as nothing rather than failing.
    #[test]
    fn recognize_takes_degenerate_images() {
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        for (w, h) in [(1u32, 1u32), (1, 3000), (3000, 1), (2, 2), (7, 4000), (4000, 7)] {
            let px = vec![0u8; (w * h) as usize];
            let _ = ocr.recognize(&px, w, h);
        }
    }

    /// End to end on rendered text, when the models are installed (`cargo xtask models`).
    #[test]
    fn reads_rendered_text() {
        let Some(models) = Models::find() else {
            eprintln!("skipped: OCR models not installed");
            return;
        };
        let ocr = Ocr::load(&models).unwrap();
        let (w, h, px) = test_image::hello();
        let lines = ocr.recognize(&px, w, h).unwrap();
        let text: Vec<String> = lines.iter().map(Line::text).collect();
        assert!(text.iter().any(|t| t.contains("HELL") && t.contains("WOR")), "{text:?}");
        let word = lines.iter().flat_map(|l| &l.words).find(|w| w.text.contains("HELL")).unwrap();
        assert!(word.rect[0] >= 10.0 && word.rect[0] < 60.0, "{word:?}");
    }

    /// The detector alone (the line-box pipeline needs no recogniser), when installed.
    fn detector() -> Option<ocrs::OcrEngine> {
        let path = Models::search_dirs().into_iter().map(|d| d.join(DETECTION_MODEL)).find(|p| p.is_file())?;
        let model = rten::Model::load_file(path).ok()?;
        ocrs::OcrEngine::new(ocrs::OcrEngineParams { detection_model: Some(model), ..Default::default() }).ok()
    }

    fn line_boxes_of(engine: &ocrs::OcrEngine, (w, h, px): &(u32, u32, Vec<u8>)) -> (Vec<rten_imageproc::RotatedRect>, Vec<[f32; 4]>) {
        let source = ocrs::ImageSource::from_bytes(px, (*w, *h)).unwrap();
        let input = engine.prepare_input(source).unwrap();
        let words = engine.detect_words(&input).unwrap();
        let boxes = ctc_line_boxes(engine, &input, words.clone(), *w);
        (words, boxes)
    }

    #[test]
    fn seal_filter_keeps_words_when_dot_leaders_dominate() {
        // A form page: the detector turns fill-in leaders into many short, thin components
        // (here 73 of them, 7 px tall) beside 22 words 32 px tall. The median is a dot's
        // height, and twice it used to make every word a "seal".
        let mut words: Vec<_> = (0..73).map(|i| rotated(20.0 + (i % 5) as f32 * 20.0, 7.0)).collect();
        words.extend((0..22).map(|i| rotated(60.0 + (i % 4) as f32 * 30.0, 32.0)));
        let (kept, _) = filter_seal_words(words, 1240);
        assert_eq!(kept.iter().filter(|r| r.height() == 32.0).count(), 22, "every word survives");
        // A seal (4 cm across, ~240 px at 150 dpi) on the same page still goes.
        let mut with_seal = kept;
        with_seal.push(rotated(260.0, 240.0));
        let (kept, _) = filter_seal_words(with_seal, 1240);
        assert!(kept.iter().all(|r| r.height() < 240.0));
    }

    #[test]
    fn seal_filter_drops_seal_fragments_smaller_than_a_whole_seal() {
        // The detector does not return a seal whole: it returns pieces of its ring and lettering,
        // a few times a word's height, which then join two text lines into one crop. Shapes from
        // real scans: w2 (W 2413 px at 300 dpi, words ~35 px) and rk (W 1080 px, words ~20 px).
        for (page_width, word_h, fragment) in [(2413, 35.0, 110.0), (1080, 20.0, 50.0)] {
            let mut words: Vec<_> = (0..40).map(|i| rotated(word_h * (3.0 + (i % 5) as f32), word_h)).collect();
            words.push(rotated(fragment * 1.5, fragment));
            let (kept, _) = filter_seal_words(words, page_width);
            assert_eq!(kept.len(), 40, "W {page_width}: the {fragment} px fragment survived");
        }
    }

    #[test]
    fn merge_line_boxes_joins_a_leader_run_to_its_form_line() {
        // A label and typed value (36 px tall), then the rest of the row's dot leader, 6 px
        // tall on the baseline, which the line grouper left as a line of its own.
        let raw = vec![[80.0, 184.0, 793.0, 220.0], [872.0, 208.0, 1154.0, 214.0], [81.0, 273.0, 1153.0, 280.0]];
        let merged = merge_colinear_line_boxes(raw, 1240, 30.0);
        assert_eq!(merged, vec![[80.0, 184.0, 1154.0, 220.0], [81.0, 273.0, 1154.0, 280.0]]);
    }

    /// Forms with dot leaders through the real detector, when its model is installed: every
    /// text line found without the leaders is still found with them, no line box spans two
    /// rows, and no leader is left as a separate box on a text row.
    #[test]
    fn dot_leaders_keep_form_lines() {
        let Some(engine) = detector() else {
            eprintln!("skipped: detection model not installed");
            return;
        };
        let (_, plain) = line_boxes_of(&engine, &test_image::form(false));
        let (_, dots) = line_boxes_of(&engine, &test_image::form(true));
        assert_eq!(plain.len(), 5, "{plain:?}");
        for p in &plain {
            let mid = (p[1] + p[3]) * 0.5;
            let row: Vec<_> = dots.iter().filter(|d| d[1] <= mid && mid <= d[3]).collect();
            assert_eq!(row.len(), 1, "text line {p:?} → {row:?} in {dots:?}");
            let d = row[0];
            assert!(d[0] <= p[0] + 4.0 && d[2] >= p[2] - 4.0, "text line {p:?} lost words: {d:?}");
            assert!(d[3] - d[1] < 66.0, "{d:?} spans two rows");
        }
    }

    mod test_image {
        /// Blocky glyphs, 5 dots wide and 7 tall, plus marks above (rows -2..0) or below (7..9),
        /// standing in for Thai consonants, vowels and tone marks.
        const SHAPES: [[&str; 7]; 6] = [
            ["XXXX.", "....X", ".XX.X", "X..XX", "X...X", "X...X", "X...X"],
            ["X...X", "X...X", "XX..X", "X.X.X", "X..XX", "X...X", ".XXX."],
            [".XXX.", "X...X", "....X", "..XX.", "....X", "X...X", ".XXX."],
            ["X.X.X", "XXXXX", "X...X", "X...X", "X...X", "X...X", "X...X"],
            ["XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "X..XX"],
            ["..X..", "..X..", ".XXX.", "X.X.X", "..X..", "..X..", ".XXX."],
        ];

        pub struct Canvas {
            pub w: u32,
            pub h: u32,
            pub px: Vec<u8>,
        }

        impl Canvas {
            pub fn new(w: u32, h: u32) -> Canvas {
                Canvas { w, h, px: vec![255; (w * h) as usize] }
            }
            pub fn fill(&mut self, x: u32, y: u32, w: u32, h: u32) {
                for yy in y..(y + h).min(self.h) {
                    for xx in x..(x + w).min(self.w) {
                        self.px[(yy * self.w + xx) as usize] = 0;
                    }
                }
            }
            /// Text of `n` glyphs per word, `dot` pixels per glyph dot, glyph tops at `y`;
            /// returns the right edge.
            pub fn text(&mut self, x: u32, y: u32, dot: u32, words: &[usize]) -> u32 {
                let mut cx = x;
                let mut k = 0usize;
                for &n in words {
                    for _ in 0..n {
                        let shape = SHAPES[k % SHAPES.len()];
                        for (ry, row) in shape.iter().enumerate() {
                            for (rx, b) in row.bytes().enumerate() {
                                if b == b'X' {
                                    self.fill(cx + rx as u32 * dot, y + ry as u32 * dot, dot, dot);
                                }
                            }
                        }
                        // A mark above every third glyph, below every fifth.
                        if k % 3 == 1 {
                            self.fill(cx + dot, y - 2 * dot, 3 * dot, dot);
                        }
                        if k % 5 == 3 {
                            self.fill(cx + 2 * dot, y + 8 * dot, 2 * dot, dot);
                        }
                        cx += 6 * dot;
                        k += 1;
                    }
                    cx += 4 * dot;
                }
                cx
            }
            /// A dot leader: `size`-pixel squares every `pitch` pixels from `x0` to `x1`, tops at `y`.
            pub fn leader(&mut self, x0: u32, x1: u32, y: u32, size: u32, pitch: u32) {
                let mut x = x0;
                while x + size <= x1 {
                    self.fill(x, y, size, size);
                    x += pitch;
                }
            }
            pub fn rgb(self) -> (u32, u32, Vec<u8>) {
                (self.w, self.h, self.px.iter().flat_map(|&g| [g, g, g]).collect())
            }
        }

        /// A government-form-like page (A4 width at ~150 dpi): a sentence, label + empty dot
        /// leader, label + leader with a value typed over it + second label + leader, a
        /// leader-only continuation line, and a closing sentence. `dots: false` leaves the
        /// leaders out, as the control.
        pub fn form(dots: bool) -> (u32, u32, Vec<u8>) {
            let (dot, pitch, line) = (3u32, 9u32, 66u32);
            let mut c = Canvas::new(1240, 480);
            let base = |row: u32| 60 + row * line;
            // Glyph core is 7 dots tall; leader dots sit on the baseline.
            let leader_y = |row: u32| base(row) + 7 * dot - 3;
            c.text(80, base(0), dot, &[5, 3, 6, 4, 7, 5, 4, 6]);
            let end = c.text(80, base(1), dot, &[4, 3]);
            if dots {
                c.leader(end + 6, 1160, leader_y(1), 3, pitch);
            }
            let end = c.text(80, base(2), dot, &[3]);
            if dots {
                c.leader(end + 6, 600, leader_y(2), 3, pitch);
            }
            c.text(end + 60, base(2), dot, &[4, 5]);
            let end2 = c.text(620, base(2), dot, &[5]);
            if dots {
                c.leader(end2 + 6, 1160, leader_y(2), 3, pitch);
            }
            if dots {
                c.leader(80, 1160, leader_y(3), 3, pitch);
            }
            c.text(80, base(4), dot, &[6, 4, 5, 3, 6, 4]);
            c.text(80, base(5), dot, &[2, 3]);
            if dots {
                c.leader(80 + 6 * dot * 5 + 8 * dot, 1160, leader_y(5), 3, pitch);
            }
            c.rgb()
        }

        /// "HELLO WORLD" drawn with blocky 5×7 letters, 4 pixels per dot, black on white.
        pub fn hello() -> (u32, u32, Vec<u8>) {
            const GLYPHS: &[(char, [&str; 7])] = &[
                ('H', ["X...X", "X...X", "X...X", "XXXXX", "X...X", "X...X", "X...X"]),
                ('E', ["XXXXX", "X....", "X....", "XXXX.", "X....", "X....", "XXXXX"]),
                ('L', ["X....", "X....", "X....", "X....", "X....", "X....", "XXXXX"]),
                ('O', [".XXX.", "X...X", "X...X", "X...X", "X...X", "X...X", ".XXX."]),
                ('W', ["X...X", "X...X", "X...X", "X.X.X", "X.X.X", "XX.XX", "X...X"]),
                ('R', ["XXXX.", "X...X", "X...X", "XXXX.", "X.X..", "X..X.", "X...X"]),
                ('D', ["XXXX.", "X...X", "X...X", "X...X", "X...X", "X...X", "XXXX."]),
            ];
            let text = "HELLO WORLD";
            let (dot, x0, y0) = (4u32, 40u32, 40u32);
            let (w, h) = (x0 * 2 + text.len() as u32 * 6 * dot, y0 * 2 + 7 * dot);
            let mut px = vec![255u8; (w * h * 4) as usize];
            for (i, c) in text.chars().enumerate() {
                let Some((_, rows)) = GLYPHS.iter().find(|g| g.0 == c) else { continue };
                for (ry, row) in rows.iter().enumerate() {
                    for (rx, b) in row.bytes().enumerate() {
                        if b != b'X' {
                            continue;
                        }
                        for dy in 0..dot {
                            for dx in 0..dot {
                                let x = x0 + (i as u32 * 6 + rx as u32) * dot + dx;
                                let y = y0 + ry as u32 * dot + dy;
                                let o = ((y * w + x) * 4) as usize;
                                px[o..o + 3].copy_from_slice(&[0, 0, 0]);
                            }
                        }
                    }
                }
            }
            (w, h, px)
        }
    }
}
