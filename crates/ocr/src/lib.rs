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
        if width == 0 || height == 0 || pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        // ocrs wants 1 or 3 channels.
        let rgb: std::borrow::Cow<[u8]> = if pixels.len() == (width as usize).saturating_mul(height as usize).saturating_mul(4) {
            pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>().into()
        } else {
            pixels.into()
        };
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
                let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
                let input = detector.prepare_input(source).map_err(|e| err(&e))?;
                let found = detector.detect_words(&input).map_err(|e| err(&e))?;
                let line_polys = detector.find_text_lines(&input, &found);
                let mut out = Vec::new();
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
                    if !left.is_finite() || !top.is_finite() || right <= left || bottom <= top {
                        continue;
                    }
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
                    let Some(text) = recognize_ctc_crop(&rgb, width, height, [x0, y0, x1, y1], model, dict)? else {
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
    let crop_w = (x1 - x0) as usize;
    let crop_h = (y1 - y0) as usize;
    if crop_w == 0 || crop_h == 0 {
        return Ok(None);
    }
    let dst_w = ((crop_w * CTC_HEIGHT) / crop_h.max(1)).max(16);
    if dst_w > MAX_CTC_WIDTH {
        return Ok(None);
    }
    // PaddleOCR RecResizeImg pads to at least 320 wide when narrower, or runs dynamic width.
    let padded_w = dst_w.max(160);
    if padded_w > MAX_CTC_WIDTH {
        return Ok(None);
    }
    let channels = if rgb.len() >= (width as usize).saturating_mul(height as usize).saturating_mul(3) { 3usize } else { 1usize };
    let plane = CTC_HEIGHT * padded_w;
    let mut tensor = vec![0.0f32; 3 * plane];
    for dy in 0..CTC_HEIGHT {
        let sy = (y0 as usize).saturating_add((dy * crop_h) / CTC_HEIGHT).min((height as usize).saturating_sub(1));
        for dx in 0..dst_w {
            let sx = (x0 as usize).saturating_add((dx * crop_w) / dst_w).min((width as usize).saturating_sub(1));
            let base = (sy * (width as usize) + sx) * channels;
            let (r, g, b) = if channels == 3 {
                (rgb.get(base).copied().unwrap_or(255), rgb.get(base + 1).copied().unwrap_or(255), rgb.get(base + 2).copied().unwrap_or(255))
            } else {
                let v = rgb.get(base).copied().unwrap_or(255);
                (v, v, v)
            };
            // PaddleOCR DecodeImage(BGR) + RecResizeImg: (pixel / 255.0 - 0.5) / 0.5.
            let idx = dy * padded_w + dx;
            if let Some(slot) = tensor.get_mut(idx) {
                *slot = (f32::from(b) / 255.0 - 0.5) / 0.5;
            }
            if let Some(slot) = tensor.get_mut(plane + idx) {
                *slot = (f32::from(g) / 255.0 - 0.5) / 0.5;
            }
            if let Some(slot) = tensor.get_mut(2 * plane + idx) {
                *slot = (f32::from(r) / 255.0 - 0.5) / 0.5;
            }
        }
    }
    let input = rten::Value::from_shape([1, 3, CTC_HEIGHT, padded_w], tensor).map_err(|e| OcrError::Recognize(e.to_string()))?;
    let output = model.run_one(input.into(), None).map_err(|e| OcrError::Recognize(e.to_string()))?;
    let (shape, probs): ([usize; 3], Vec<f32>) = output.into_shape_vec().map_err(|e| OcrError::Recognize(e.to_string()))?;
    let classes = shape[2];
    Ok(Some(ctc::decode(&probs, classes, dict)))
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

    let mut out = b"/OCR BMC\nBT\n3 Tr\n".to_vec();
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
    out.extend_from_slice(b"ET\nEMC\n");
    (out, fonts)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    mod test_image {
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
