//! The pure-Rust engine: the ocrs crate with its pre-trained models, wrapped as a
//! [`Recognizer`] (`OcrsRecognizer`). Reading is offline and local; the models do not report a
//! confidence, so its words have `confidence: None`.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rten_imageproc::{RectF, RotatedRect};

use super::{Line, MAX_PIXELS, MAX_SIDE, OcrError, OcrImage, RecognizeOptions, Recognizer, Word};
use crate::thai::{MAX_CTC_WIDTH, channels_for_ocrs, clip_crop, recognize_ctc, recognize_ctc_crop};

/// The validated byte length of a `width` × `height` RGBA raster, with the same caps
/// [`OcrImage::new`] applies: zero sides are empty, sides over [`MAX_SIDE`] or totals over
/// [`MAX_PIXELS`] are too large. `Ocr::recognize` takes raw dimensions (they come from
/// callers, ultimately from documents, so they are untrusted), and the naive
/// `width * height * 4` overflows `u32` for absurd values — the caps are checked first, in
/// `usize`, so nothing overflows and nothing huge is ever touched.
fn checked_len(width: u32, height: u32) -> Result<usize, OcrError> {
    if width == 0 || height == 0 {
        return Err(OcrError::EmptyImage);
    }
    if width > MAX_SIDE || height > MAX_SIDE {
        return Err(OcrError::ImageTooLarge);
    }
    let pixels = (width as usize).checked_mul(height as usize).ok_or(OcrError::ImageTooLarge)?;
    if pixels > MAX_PIXELS as usize {
        return Err(OcrError::ImageTooLarge);
    }
    pixels.checked_mul(4).ok_or(OcrError::ImageTooLarge)
}

/// The languages the models read (ISO 639-1): English with the default models (the Latin
/// alphabet, no accents), and Thai with the CTC recognizer ([`THAI_RECOGNITION_MODEL`]).
pub const LANGUAGES: &[(&str, &str)] = &[("en", "English"), ("th", "ไทย")];

/// The model files, as named in ATTRIBUTION.toml.
pub const DETECTION_MODEL: &str = "text-detection.onnx";
pub const RECOGNITION_MODEL: &str = "text-recognition.onnx";
pub const THAI_RECOGNITION_MODEL: &str = "text-recognition-th.rten";
pub const THAI_DICT: &str = "dict-th.txt";
/// Their SHA-256s, as in ATTRIBUTION.toml. [`Ocr::from_bytes`] loads nothing else: rten's ONNX
/// parser can spin forever on bytes that aren't a model (64 × 0xff does).
pub const DETECTION_MODEL_SHA256: &str = "a917b23dbd9524b465df7e922641b3ff2981623df4ded5a0234004ef2fee7cfe";
pub const RECOGNITION_MODEL_SHA256: &str = "86c145c2edb96c8caed5b1ebb8f44d706408922211451c309c625157dd6061c5";

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes).iter().map(|b| format!("{b:02x}")).collect()
}

pub const ALPHABET: &str = " 0123456789!\"#$%&'()*+,-./:;<=>?@[\\]^_`{|}~€ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";

/// Where the model files are. `dictionary` is set for the CTC (Thai) recognizer only.
#[derive(Clone, Debug, PartialEq)]
pub struct Models {
    pub detection: PathBuf,
    pub recognition: PathBuf,
    pub dictionary: Option<PathBuf>,
}

impl Models {
    pub fn dirs_beside_exe(exe_dir: &Path) -> Vec<PathBuf> {
        vec![exe_dir.join("models"), exe_dir.join("../Resources/models"), exe_dir.join("../share/pdfcraft/models")]
    }

    /// The models in `dir`, preferring current ONNX files over legacy RTEN files. An incomplete
    /// update never mixes formats or hides an intact legacy pair.
    pub fn in_dir(dir: &Path) -> Option<Models> {
        [(DETECTION_MODEL, RECOGNITION_MODEL), ("text-detection.rten", "text-recognition.rten")].into_iter().find_map(|(detection, recognition)| {
            let m = Models { detection: dir.join(detection), recognition: dir.join(recognition), dictionary: None };
            (m.detection.is_file() && m.recognition.is_file()).then_some(m)
        })
    }

    /// The models for `lang` (`"en"` or `"th"`) in `dir`, if all required files are there.
    pub fn in_dir_for(dir: &Path, lang: &str) -> Option<Models> {
        match lang {
            "th" | "tha" => {
                let detection = [DETECTION_MODEL, "text-detection.rten"].into_iter().map(|f| dir.join(f)).find(|p| p.is_file())?;
                let recognition = dir.join(THAI_RECOGNITION_MODEL);
                let dict = dir.join(THAI_DICT);
                (recognition.is_file() && dict.is_file()).then_some(Models { detection, recognition, dictionary: Some(dict) })
            }
            "en" | "eng" | "" => Self::in_dir(dir),
            _ => None,
        }
    }

    /// Look for the models: `$PDFCRAFT_MODELS`, then `models/` beside the executable (and
    /// `Resources/models` in a macOS bundle), then the source tree's `assets/models/`.
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
}

/// How far each found line is widened before its image is cut out for recognition, as fractions
/// of the line's median word height: above and below every word, and before its first and after
/// its last word. ocrs grows each word box by a fixed 3 pixels, a sliver of a letter on a page
/// scanned at 300 dpi or more: the line images clipped the first and last letters ("The" read as
/// "he", "and" as "an", closing full stops lost), and the more so the higher the resolution.
/// Chosen on synthetic pages at 150 to 400 dpi, clean and noisy, and on MDPBench's pages. More
/// padding above and below costs small, noisy text (the recogniser scales each line image to a
/// fixed height, so the margin leaves less height for the letters): at 0.15 the 8 pt "Telefon"
/// label of a noisy form scanned at 150 dpi read "Teleton".
const LINE_PAD_ACROSS: f32 = 0.12;
const LINE_PAD_ALONG: f32 = 0.1;

/// Widen `line` (its word boxes, in reading order) as [`LINE_PAD_ACROSS`] and
/// [`LINE_PAD_ALONG`] say, after the lines were found, so finding them is unchanged. Returns the
/// box the words covered before, which the recognised words are clipped to: the text layer sizes
/// its text from them.
fn pad_line(line: &mut [RotatedRect]) -> Option<RectF> {
    let before = rten_imageproc::bounding_rect(line.iter())?;
    // A box's height runs along its up axis and its width along the line, whichever way the
    // line lies on the page.
    let mut heights: Vec<f32> = line.iter().map(RotatedRect::height).filter(|h| h.is_finite() && *h > 0.0).collect();
    heights.sort_by(f32::total_cmp);
    let height = heights.get(heights.len() / 2).copied().unwrap_or(0.0);
    let last = line.len().saturating_sub(1);
    for (i, r) in line.iter_mut().enumerate() {
        let along = if i == 0 || i == last { 2.0 * LINE_PAD_ALONG * height } else { 0.0 };
        let across = 2.0 * LINE_PAD_ACROSS * height;
        r.resize(r.width() + along, r.height() + across);
    }
    Some(before)
}

/// `rect` ([left, top, right, bottom]) clipped to `to`; unchanged if that leaves nothing: a word
/// wholly in the padding is a letter the line found had cut off, and keeps its own box.
fn clip(rect: [f32; 4], to: Option<RectF>) -> [f32; 4] {
    let Some(to) = to else { return rect };
    let [l, t, r, b] = rect;
    let clipped = [l.max(to.left()), t.max(to.top()), r.min(to.right()), b.min(to.bottom())];
    if clipped[0] < clipped[2] && clipped[1] < clipped[3] { clipped } else { rect }
}

/// Parse `"image_shape": [3, 48, W]` from `model_meta.json` beside the recognition model, if present.
fn parse_model_meta_width(recognition_path: &Path) -> Option<usize> {
    let meta_path = recognition_path.parent()?.join("model_meta.json");
    let text = std::fs::read_to_string(meta_path).ok()?;
    let idx = text.find("\"image_shape\"")?;
    let after = &text[idx..];
    let open = after.find('[')?;
    let close = after[open..].find(']')?;
    let nums: Vec<usize> = after[open + 1..open + close].split(',').filter_map(|s| s.trim().parse::<usize>().ok()).collect();
    nums.get(2).copied().filter(|&w| (16..=MAX_CTC_WIDTH).contains(&w))
}

/// The two ways of reading: the ocrs word and line recogniser (English), or the CTC recognizer
/// for Thai (ocrs only detects words).
enum Engine {
    Ocrs(ocrs::OcrEngine),
    Ctc { detector: ocrs::OcrEngine, model: Box<rten::Model>, dict: Vec<char>, fixed_width: Option<usize> },
}

/// A loaded recogniser. Loading takes a moment; keep one and reuse it.
pub struct Ocr {
    engine: Engine,
}

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
            let fixed_width = parse_model_meta_width(&models.recognition);
            Ok(Ocr { engine: Engine::Ctc { detector, model, dict, fixed_width } })
        } else {
            Self::with_models(load(&models.detection)?, load(&models.recognition)?)
        }
    }

    /// Load the models from the contents of their files, where there is no file system (the web
    /// build downloads them). Bytes that aren't the files ATTRIBUTION.toml names are an error.
    pub fn from_bytes(detection: Vec<u8>, recognition: Vec<u8>) -> Result<Ocr, OcrError> {
        let load = |name: &str, sha256: &str, data: Vec<u8>| {
            if sha256_hex(&data) != sha256 {
                return Err(OcrError::Load(name.into(), "not the expected model file (SHA-256 differs)".into()));
            }
            rten::Model::load(data).map_err(|e| OcrError::Load(name.into(), e.to_string()))
        };
        Self::with_models(load(DETECTION_MODEL, DETECTION_MODEL_SHA256, detection)?, load(RECOGNITION_MODEL, RECOGNITION_MODEL_SHA256, recognition)?)
    }

    fn with_models(detection: rten::Model, recognition: rten::Model) -> Result<Ocr, OcrError> {
        let params = ocrs::OcrEngineParams {
            detection_model: Some(detection),
            recognition_model: Some(recognition),
            alphabet: Some(ALPHABET.into()),
            ..Default::default()
        };
        let engine = ocrs::OcrEngine::new(params).map_err(|e| OcrError::Load("ocr engine".into(), e.to_string()))?;
        Ok(Ocr { engine: Engine::Ocrs(engine) })
    }

    /// Load the models found by [`Models::find`].
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
        if pixels.is_empty() {
            return Err(OcrError::EmptyImage);
        }
        let want = checked_len(width, height)?;
        // ocrs wants 1 or 3 channels.
        let rgb: std::borrow::Cow<[u8]> = if pixels.len() == want {
            pixels.as_chunks::<4>().0.iter().flat_map(|p| [p[0], p[1], p[2]]).collect::<Vec<u8>>().into()
        } else {
            pixels.into()
        };
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        match &self.engine {
            Engine::Ocrs(engine) => {
                let source = ocrs::ImageSource::from_bytes(&rgb, (width, height)).map_err(|e| err(&e))?;
                let input = engine.prepare_input(source).map_err(|e| err(&e))?;
                let found = engine.detect_words(&input).map_err(|e| err(&e))?;
                let mut lines = engine.find_text_lines(&input, &found);
                let before: Vec<Option<RectF>> = lines.iter_mut().map(|l| pad_line(l)).collect();
                let read = engine.recognize_text(&input, &lines).map_err(|e| err(&e))?;
                use ocrs::TextItem;
                Ok(read
                    .into_iter()
                    .zip(before)
                    .filter_map(|(line, before)| line.map(|l| (l, before)))
                    .map(|(line, before)| Line {
                        words: line
                            .words()
                            .map(|w| {
                                let r = w.bounding_rect();
                                Word::new(w.to_string(), clip([r.left() as f32, r.top() as f32, r.right() as f32, r.bottom() as f32], before), "ocrs")
                            })
                            .filter(|w| !w.text.trim().is_empty())
                            .collect(),
                    })
                    .filter(|l| !l.words.is_empty())
                    .collect())
            }
            Engine::Ctc { detector, model, dict, fixed_width } => recognize_ctc(&rgb, (width, height), detector, model, dict, *fixed_width),
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
        match &self.engine {
            Engine::Ocrs(_) => Ok(None),
            Engine::Ctc { model, dict, fixed_width, .. } => Ok(recognize_ctc_crop(&rgb, width, height, rect, model, dict, *fixed_width)?.map(|(text, _)| text)),
        }
    }
}

/// The ocrs engine as a [`Recognizer`] (the default engine).
pub struct OcrsRecognizer {
    engine: Ocr,
    /// Loaded from bytes ([`OcrsRecognizer::from_ocr`]), not from model files: it stays
    /// available without them.
    in_memory: bool,
    /// The Thai recognizer, loaded on first use (`None` inside when its models are missing).
    thai: OnceLock<Option<Ocr>>,
}

impl OcrsRecognizer {
    /// Load the models from `models`.
    pub fn load(models: &Models) -> Result<OcrsRecognizer, OcrError> {
        Ok(OcrsRecognizer { engine: Ocr::load(models)?, in_memory: false, thai: OnceLock::new() })
    }

    /// A recogniser loaded some other way than from model files: the web build downloads the
    /// models and loads them with [`Ocr::from_bytes`].
    pub fn from_ocr(engine: Ocr) -> OcrsRecognizer {
        OcrsRecognizer { engine, in_memory: true, thai: OnceLock::new() }
    }

    /// Load the models found by [`Models::find`].
    pub fn find() -> Result<OcrsRecognizer, OcrError> {
        Self::load(&Models::find().ok_or(OcrError::NoModels)?)
    }
}

impl Recognizer for OcrsRecognizer {
    fn id(&self) -> &'static str {
        "ocrs"
    }

    fn available(&self) -> bool {
        self.in_memory || Models::find().is_some()
    }

    fn languages(&self) -> Vec<(String, String)> {
        LANGUAGES.iter().map(|(c, n)| ((*c).to_string(), (*n).to_string())).collect()
    }

    fn recognize(&self, image: &OcrImage, options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        if matches!(options.language.as_str(), "th" | "tha") {
            let Some(thai) = self.thai.get_or_init(|| Ocr::load_for("th").ok()) else {
                return Err(OcrError::NoLanguageModel("th".into()));
            };
            return thai.recognize(image.rgba(), image.width(), image.height());
        }
        self.engine.recognize(image.rgba(), image.width(), image.height())
    }
}

#[cfg(test)]
mod tests {
    use rten_imageproc::{PointF, Vec2};

    use super::*;

    /// A found line is widened by fractions of its word height: every word above and below, the
    /// first and the last word along the line; the box returned is the line's before.
    #[test]
    fn a_line_is_padded_by_its_word_height() {
        let word = |l: f32, r: f32| RotatedRect::from_rect(RectF::from_tlbr(100.0, l, 140.0, r));
        let mut line = vec![word(10.0, 90.0), word(100.0, 200.0), word(210.0, 260.0)];
        let before = pad_line(&mut line).unwrap();
        assert_eq!([before.left(), before.top(), before.right(), before.bottom()], [10.0, 100.0, 260.0, 140.0]);
        let (across, along) = (2.0 * 0.12 * 40.0, 2.0 * 0.1 * 40.0);
        assert!(line.iter().all(|r| (r.height() - (40.0 + across)).abs() < 1e-3));
        assert!((line[0].width() - (80.0 + along)).abs() < 1e-3 && (line[2].width() - (50.0 + along)).abs() < 1e-3);
        assert!((line[1].width() - 100.0).abs() < 1e-3, "a word inside the line keeps its width");
        // A box on its side (up axis horizontal) keeps its height across the line and its width
        // along it.
        let mut side = vec![RotatedRect::new(PointF::from_yx(0.0, 0.0), Vec2::from_yx(0.0, 1.0), 40.0, 80.0)];
        pad_line(&mut side).unwrap();
        assert!((side[0].height() - (80.0 + 2.0 * 0.12 * 80.0)).abs() < 1e-3 && (side[0].width() - (40.0 + 2.0 * 0.1 * 80.0)).abs() < 1e-3);
        assert!(pad_line(&mut []).is_none());
    }

    #[test]
    fn recognised_words_are_clipped_to_the_line_found() {
        let line = Some(RectF::from_tlbr(100.0, 10.0, 140.0, 260.0));
        assert_eq!(clip([4.0, 94.0, 60.0, 146.0], line), [10.0, 100.0, 60.0, 140.0]);
        // A word wholly in the padding keeps its own box rather than none.
        assert_eq!(clip([262.0, 94.0, 270.0, 146.0], line), [262.0, 94.0, 270.0, 146.0]);
        assert_eq!(clip([1.0, 2.0, 3.0, 4.0], None), [1.0, 2.0, 3.0, 4.0]);
    }

    /// Raw dimensions are untrusted (they come from callers, ultimately from documents): zero
    /// sides, sides or pixel totals over the caps, and products that would overflow the naive
    /// `width * height * 4` are errors, never panics, before anything is touched.
    #[test]
    fn raw_dimensions_are_capped_and_checked_not_panicked_at() {
        assert!(matches!(checked_len(0, 10), Err(OcrError::EmptyImage)));
        assert!(matches!(checked_len(10, 0), Err(OcrError::EmptyImage)));
        assert!(matches!(checked_len(MAX_SIDE + 1, 1), Err(OcrError::ImageTooLarge)));
        // 65536² would wrap u32 in the old `width * height * 4`; the side cap refuses it first.
        assert!(matches!(checked_len(u32::MAX, u32::MAX), Err(OcrError::ImageTooLarge)));
        // Legal sides whose product is over the 64 MP area cap.
        assert!(matches!(checked_len(MAX_SIDE, 7000), Err(OcrError::ImageTooLarge)));
        assert_eq!(checked_len(2, 2).unwrap(), 16);
        // Exactly the area cap fits: 8000 × 8000 = 64 MP.
        assert_eq!(checked_len(8000, 8000).unwrap(), 8000usize * 8000 * 4);
    }

    /// The current ONNX pair wins when an installation also retains both legacy models.
    #[test]
    fn model_discovery_prefers_the_complete_onnx_pair() {
        let dir = tempfile::tempdir().unwrap();
        for file in ["text-detection.onnx", "text-recognition.onnx"] {
            std::fs::write(dir.path().join(file), b"model").unwrap();
        }
        let expected = Some(Models { detection: dir.path().join("text-detection.onnx"), recognition: dir.path().join("text-recognition.onnx"), dictionary: None });
        assert_eq!(Models::in_dir(dir.path()), expected, "a complete ONNX pair is sufficient");
        for file in ["text-detection.rten", "text-recognition.rten"] {
            std::fs::write(dir.path().join(file), b"model").unwrap();
        }
        assert_eq!(Models::in_dir(dir.path()), expected, "the complete ONNX pair takes precedence over legacy models");
    }

    /// An interrupted ONNX update still permits an intact legacy pair in the same directory.
    #[test]
    fn model_discovery_falls_back_to_legacy_when_the_onnx_pair_is_incomplete() {
        for current in ["text-detection.onnx", "text-recognition.onnx"] {
            let dir = tempfile::tempdir().unwrap();
            for file in [current, "text-detection.rten", "text-recognition.rten"] {
                std::fs::write(dir.path().join(file), b"model").unwrap();
            }
            assert_eq!(
                Models::in_dir(dir.path()),
                Some(Models { detection: dir.path().join("text-detection.rten"), recognition: dir.path().join("text-recognition.rten"), dictionary: None }),
                "{current} alone must not hide a complete legacy pair"
            );
        }
    }

    /// Detection and recognition must come from one complete format pair, never a mixed update.
    #[test]
    fn model_discovery_refuses_mixed_format_pairs() {
        for pair in [["text-detection.onnx", "text-recognition.rten"], ["text-detection.rten", "text-recognition.onnx"]] {
            let dir = tempfile::tempdir().unwrap();
            for file in pair {
                std::fs::write(dir.path().join(file), b"model").unwrap();
            }
            assert_eq!(Models::in_dir(dir.path()), None, "{pair:?} is not a complete model pair");
        }
    }

    /// Existing source builds and PDFCRAFT_MODELS overrides retain their complete RTEN pair.
    #[test]
    fn model_discovery_keeps_legacy_only_installations() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Models::in_dir(dir.path()), None);
        std::fs::write(dir.path().join("text-detection.rten"), b"model").unwrap();
        assert_eq!(Models::in_dir(dir.path()), None, "one legacy model is incomplete");
        std::fs::write(dir.path().join("text-recognition.rten"), b"model").unwrap();
        assert_eq!(
            Models::in_dir(dir.path()),
            Some(Models { detection: dir.path().join("text-detection.rten"), recognition: dir.path().join("text-recognition.rten"), dictionary: None })
        );
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


    /// Writes `meta` as `model_meta.json` in a fresh per-test directory (unique per name and
    /// process, so parallel tests never share one) and returns the path of a model beside it.
    fn model_with_meta(name: &str, meta: Option<&str>) -> (std::path::PathBuf, std::path::PathBuf) {
        let dir = std::env::temp_dir().join(format!("pdfcraft-ocr-meta-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        if let Some(meta) = meta {
            std::fs::write(dir.join("model_meta.json"), meta).expect("write meta");
        }
        (dir.join("rec.onnx"), dir)
    }

    #[test]
    fn model_meta_width_is_read_from_image_shape() {
        let (model, dir) = model_with_meta("ok", Some(r#"{"name": "th", "image_shape": [3, 48, 800]}"#));
        assert_eq!(parse_model_meta_width(&model), Some(800));
        let _ = std::fs::remove_dir_all(dir);

        let (model, dir) = model_with_meta("multiline", Some("{\n \"image_shape\": [\n  3,\n  48,\n  320\n ]\n}"));
        assert_eq!(parse_model_meta_width(&model), Some(320));
        let _ = std::fs::remove_dir_all(dir);

        // The bounds themselves are accepted.
        let (model, dir) = model_with_meta("bounds-lo", Some(r#"{"image_shape": [3, 48, 16]}"#));
        assert_eq!(parse_model_meta_width(&model), Some(16));
        let _ = std::fs::remove_dir_all(dir);
        let (model, dir) = model_with_meta("bounds-hi", Some(&format!(r#"{{"image_shape": [3, 48, {MAX_CTC_WIDTH}]}}"#)));
        assert_eq!(parse_model_meta_width(&model), Some(MAX_CTC_WIDTH));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn model_meta_width_is_none_without_a_meta_file() {
        let (model, dir) = model_with_meta("missing", None);
        assert_eq!(parse_model_meta_width(&model), None);
        let _ = std::fs::remove_dir_all(dir);
        // A model path with no parent directory has nowhere to look either.
        assert_eq!(parse_model_meta_width(Path::new("")), None);
    }

    #[test]
    fn model_meta_width_rejects_out_of_range_and_malformed_values() {
        let too_wide = format!(r#"{{"image_shape": [3, 48, {}]}}"#, MAX_CTC_WIDTH + 1);
        let cases: [(&str, &str); 9] = [
            ("narrow", r#"{"image_shape": [3, 48, 8]}"#),
            ("just-narrow", r#"{"image_shape": [3, 48, 15]}"#),
            ("huge", r#"{"image_shape": [3, 48, 100000]}"#),
            ("just-wide", &too_wide),
            ("no-bracket-close", r#"{"image_shape": [3, 48, 800"#),
            ("no-bracket-open", r#"{"image_shape": 3, 48, 800}"#),
            ("two-numbers", r#"{"image_shape": [3, 48]}"#),
            ("no-key", r#"{"shape": [3, 48, 800]}"#),
            ("not-numbers", r#"{"image_shape": [3, 48, "wide"]}"#),
        ];
        for (name, meta) in cases {
            let (model, dir) = model_with_meta(name, Some(meta));
            assert_eq!(parse_model_meta_width(&model), None, "{name}: {meta}");
            let _ = std::fs::remove_dir_all(dir);
        }
    }
}
