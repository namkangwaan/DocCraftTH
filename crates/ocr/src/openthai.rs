//! OpenThai 2.1 Text: a line-level Thai (and Latin) reader that runs on rten, offline.
//!
//! The model set is iApp's `OpenThai2.1-OCR-Text` (Apache-2.0; see ATTRIBUTION.toml): a PP-OCRv6
//! tiny DB text detector, a CTC line recognizer and a second recognizer that re-reads lines in
//! which the first one mixed Latin look-alikes into Thai words. The ONNX files are converted to
//! `.rten` (`docs/thai-ocr-model.md`). Reading a page is:
//!
//! 1. detect: scale the page so its long side is 1280 px, run the detector, threshold the
//!    probability map, take each connected component's minimum-area rectangle, score it, grow it
//!    ("unclip") and map it back to page pixels;
//! 2. recognize: cut each box out at 48 px height, remove the background column by column
//!    ("flatten"), run the CTC recognizer and decode greedily with a confidence;
//! 3. clean: re-read mixed-script lines with the second recognizer, normalise Thai spelling
//!    ([`canonical`]), drop low-confidence and too-short readings, and order the lines.
//!
//! Page deskew, column ordering and detaching floating marks are not done here: the caller's
//! preprocessing ([`crate::preprocess`]) straightens the page first.
//!
//! Every input is untrusted: nothing here indexes with a computed position, and every size is
//! bounded (the image by [`OcrImage`], the detector map by the 1280 px working size).

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use rten::{Model, Value};

use super::{Line, OcrError, OcrImage, RecognizeOptions, Recognizer, Word};

/// The files of the model set, as named in ATTRIBUTION.toml.
pub const DET_MODEL: &str = "openthai-det.rten";
pub const REC_MODEL: &str = "openthai-rec.rten";
pub const REC2_MODEL: &str = "openthai-rec2.rten";
pub const DICT: &str = "openthai-dict.txt";

/// The languages the recognizer reads.
pub const LANGUAGES: &[(&str, &str)] = &[("th", "ไทย"), ("en", "English")];

/// Long side of the detector's working image, in pixels.
const LIMIT: f32 = 1280.0;
/// Largest scale-up of a small image (the detector finds nothing in a tiny crop otherwise).
const MAX_SCALE: f32 = 4.0;
/// DB post-processing, as the reference runtime applies it (its defaults, not the values in the
/// model's `det.yml`, which it does not read as numbers).
const THRESH: f32 = 0.3;
const BOX_THRESH: f32 = 0.6;
const UNCLIP: f32 = 1.5;
const MIN_SIZE: f32 = 3.0;
const MAX_COMPONENTS: usize = 3000;
/// Recognizer input height and widest line, in pixels.
const REC_H: usize = 48;
const MAX_W: usize = 3200;
/// Line filter: a reading under this mean character confidence, or with fewer characters, is
/// dropped as a stamp, seal or mark. A lone digit or Latin letter is kept when its box is at
/// least [`SHORT_HEIGHT`] of the page's typical line height.
const DROP_BELOW: f32 = 0.8;
const MIN_CHARS: usize = 2;
const SHORT_HEIGHT: f32 = 0.62;
const MEAN: [f32; 3] = [0.485, 0.456, 0.406];
const STD: [f32; 3] = [0.229, 0.224, 0.225];

/// An RGB raster (alpha dropped).
struct Rgb {
    w: usize,
    h: usize,
    d: Vec<u8>,
}

impl Rgb {
    fn from_image(image: &OcrImage) -> Rgb {
        let d = image.rgba().as_chunks::<4>().0.iter().flat_map(|p| p.iter().take(3).copied()).collect();
        Rgb { w: image.width() as usize, h: image.height() as usize, d }
    }

    /// The channel `c` of pixel (`x`, `y`); white outside the image.
    fn at(&self, x: usize, y: usize, c: usize) -> f32 {
        if x >= self.w || y >= self.h {
            return 255.0;
        }
        y.checked_mul(self.w)
            .and_then(|r| r.checked_add(x))
            .and_then(|p| p.checked_mul(3))
            .and_then(|i| i.checked_add(c))
            .and_then(|i| self.d.get(i))
            .map_or(255.0, |&v| f32::from(v))
    }

    /// Bilinear sample at a real position (centres at integers), clamped to the image.
    fn sample(&self, x: f32, y: f32) -> [f32; 3] {
        let x = x.clamp(0.0, self.w.saturating_sub(1) as f32);
        let y = y.clamp(0.0, self.h.saturating_sub(1) as f32);
        let (x0, y0) = (x.floor() as usize, y.floor() as usize);
        let (x1, y1) = (x0.saturating_add(1).min(self.w.saturating_sub(1)), y0.saturating_add(1).min(self.h.saturating_sub(1)));
        let (tx, ty) = (x - x0 as f32, y - y0 as f32);
        std::array::from_fn(|c| {
            let top = self.at(x0, y0, c) + (self.at(x1, y0, c) - self.at(x0, y0, c)) * tx;
            let bot = self.at(x0, y1, c) + (self.at(x1, y1, c) - self.at(x0, y1, c)) * tx;
            top + (bot - top) * ty
        })
    }

    /// This image with a white border of `pad` pixels on every side.
    fn padded(&self, pad: usize) -> Rgb {
        let w = self.w.saturating_add(pad.saturating_mul(2));
        let h = self.h.saturating_add(pad.saturating_mul(2));
        let mut d = vec![255u8; w.saturating_mul(h).saturating_mul(3)];
        for (y, row) in self.d.chunks_exact(self.w.saturating_mul(3).max(1)).enumerate() {
            let start = y.saturating_add(pad).saturating_mul(w).saturating_add(pad).saturating_mul(3);
            if let Some(dst) = d.get_mut(start..start.saturating_add(row.len())) {
                dst.copy_from_slice(row);
            }
        }
        Rgb { w, h, d }
    }
}

/// A detected text box in page pixels: its centre, the unit vector along the line (`right`, with
/// a non-negative x) and across it (`down`, with a non-negative y), and its extents along each.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Quad {
    c: [f32; 2],
    right: [f32; 2],
    down: [f32; 2],
    len: f32,
    hgt: f32,
}

impl Quad {
    /// The axis-aligned box of the quad, `[left, top, right, bottom]`.
    fn bounds(&self) -> [f32; 4] {
        let (hl, hh) = (self.len / 2.0, self.hgt / 2.0);
        let xs = [-1.0f32, 1.0].map(|a| [-1.0f32, 1.0].map(|b| self.c[0] + a * hl * self.right[0] + b * hh * self.down[0]));
        let ys = [-1.0f32, 1.0].map(|a| [-1.0f32, 1.0].map(|b| self.c[1] + a * hl * self.right[1] + b * hh * self.down[1]));
        let fold = |v: [[f32; 2]; 2], init: f32, f: fn(f32, f32) -> f32| v.iter().flatten().fold(init, |m, &x| f(m, x));
        [fold(xs, f32::MAX, f32::min), fold(ys, f32::MAX, f32::min), fold(xs, f32::MIN, f32::max), fold(ys, f32::MIN, f32::max)]
    }
}

/// The convex hull of `points` (Andrew's monotone chain), counter-clockwise, without repeats.
fn convex_hull(points: &[[f32; 2]]) -> Vec<[f32; 2]> {
    let mut p: Vec<[f32; 2]> = points.iter().copied().filter(|q| q[0].is_finite() && q[1].is_finite()).collect();
    p.sort_by(|a, b| a[0].total_cmp(&b[0]).then(a[1].total_cmp(&b[1])));
    p.dedup();
    if p.len() < 3 {
        return p;
    }
    let cross = |o: [f32; 2], a: [f32; 2], b: [f32; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let half = |iter: &mut dyn Iterator<Item = &[f32; 2]>| {
        let mut h: Vec<[f32; 2]> = Vec::new();
        for &q in iter {
            while let (Some(&b), Some(&a)) = (h.last(), h.len().checked_sub(2).and_then(|i| h.get(i))) {
                if cross(a, b, q) <= 0.0 {
                    h.pop();
                } else {
                    break;
                }
            }
            h.push(q);
        }
        h.pop();
        h
    };
    let mut hull = half(&mut p.iter());
    hull.extend(half(&mut p.iter().rev()));
    hull
}

/// The minimum-area rectangle of a point set: some side lies along a hull edge, so each edge
/// direction is tried. (`rten-imageproc`'s `min_area_rect` returned rectangles under a pixel
/// thick for some wide hulls, which dropped a third of a page's lines.)
#[derive(Clone, Copy, Debug)]
struct Rect {
    u: [f32; 2],
    v: [f32; 2],
    u0: f32,
    u1: f32,
    v0: f32,
    v1: f32,
}

fn min_area_rect(points: &[[f32; 2]]) -> Option<Rect> {
    let hull = convex_hull(points);
    let mut best: Option<(f32, Rect)> = None;
    for (i, a) in hull.iter().enumerate() {
        let b = hull.get((i + 1) % hull.len())?;
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt();
        if l < 1e-6 {
            continue;
        }
        let (u, v) = ([dx / l, dy / l], [-dy / l, dx / l]);
        let project =
            |axis: [f32; 2]| hull.iter().map(|p| p[0] * axis[0] + p[1] * axis[1]).fold((f32::MAX, f32::MIN), |(lo, hi), x| (lo.min(x), hi.max(x)));
        let ((u0, u1), (v0, v1)) = (project(u), project(v));
        let area = (u1 - u0) * (v1 - v0);
        if best.is_none_or(|(a, _)| area < a) {
            best = Some((area, Rect { u, v, u0, u1, v0, v1 }));
        }
    }
    best.map(|(_, r)| r)
}

/// The scaled, normalised detector input for `img`: BGR planes with ImageNet statistics, as
/// PaddleOCR feeds its detectors. Returns the tensor data and its `(height, width)`.
fn det_input(padded: &Rgb) -> (Vec<f32>, usize, usize) {
    let long = padded.w.max(padded.h).max(1) as f32;
    let s = (LIMIT / long).min(MAX_SCALE);
    let round32 = |v: usize| ((v as f32 * s / 32.0).round() as usize).saturating_mul(32).max(32);
    let (rw, rh) = (round32(padded.w), round32(padded.h));
    let plane = rw.saturating_mul(rh);
    let mut x = vec![0f32; plane.saturating_mul(3)];
    let (sx, sy) = (padded.w as f32 / rw as f32, padded.h as f32 / rh as f32);
    for y in 0..rh {
        let fy = ((y as f32 + 0.5) * sy - 0.5).max(0.0);
        for xx in 0..rw {
            let fx = ((xx as f32 + 0.5) * sx - 0.5).max(0.0);
            let rgb = padded.sample(fx, fy);
            for (c, slot) in [2usize, 1, 0].into_iter().enumerate() {
                let v = (rgb.get(slot).copied().unwrap_or(255.0) / 255.0 - MEAN[c]) / STD[c];
                if let Some(o) = x.get_mut(c * plane + y * rw + xx) {
                    *o = v;
                }
            }
        }
    }
    (x, rh, rw)
}

/// The 8-connected components of `prob > THRESH`, each as its boundary pixels `[x, y]`.
fn components(prob: &[f32], w: usize, h: usize) -> Vec<Vec<[f32; 2]>> {
    let on = |x: isize, y: isize| -> bool {
        x >= 0 && y >= 0 && (x as usize) < w && (y as usize) < h && prob.get(y as usize * w + x as usize).is_some_and(|&p| p > THRESH)
    };
    let mut seen = vec![false; prob.len()];
    let mut comps = Vec::new();
    for start in 0..prob.len() {
        let (sx, sy) = ((start % w.max(1)) as isize, (start / w.max(1)) as isize);
        if !on(sx, sy) || seen.get(start).copied().unwrap_or(true) {
            continue;
        }
        let mut stack = vec![(sx, sy)];
        if let Some(s) = seen.get_mut(start) {
            *s = true;
        }
        let mut edge = Vec::new();
        while let Some((x, y)) = stack.pop() {
            if [(0, 1), (0, -1), (1, 0), (-1, 0)].iter().any(|&(dx, dy)| !on(x + dx, y + dy)) {
                edge.push([x as f32, y as f32]);
            }
            for (dx, dy) in (-1isize..=1).flat_map(|dx| (-1isize..=1).map(move |dy| (dx, dy))) {
                let (nx, ny) = (x + dx, y + dy);
                let idx = (ny >= 0 && nx >= 0).then(|| ny as usize * w + nx as usize);
                if let Some(slot) = idx.and_then(|i| seen.get_mut(i)).filter(|s| !**s)
                    && on(nx, ny)
                {
                    *slot = true;
                    stack.push((nx, ny));
                }
            }
        }
        comps.push(edge);
        if comps.len() >= MAX_COMPONENTS {
            break;
        }
    }
    comps
}

/// The mean probability inside `rect` (pixels whose centre lies in it).
fn box_score(prob: &[f32], w: usize, h: usize, rect: &Rect) -> f32 {
    let corners = [(rect.u0, rect.v0), (rect.u1, rect.v0), (rect.u0, rect.v1), (rect.u1, rect.v1)]
        .map(|(a, b)| [a * rect.u[0] + b * rect.v[0], a * rect.u[1] + b * rect.v[1]]);
    let lim = |f: fn(f32, f32) -> f32, init: f32, k: usize| corners.iter().fold(init, |m, c| f(m, c.get(k).copied().unwrap_or(0.0)));
    let (x0, x1) =
        (lim(f32::min, f32::MAX, 0).floor().max(0.0) as usize, (lim(f32::max, f32::MIN, 0).ceil().max(0.0) as usize).min(w.saturating_sub(1)));
    let (y0, y1) =
        (lim(f32::min, f32::MAX, 1).floor().max(0.0) as usize, (lim(f32::max, f32::MIN, 1).ceil().max(0.0) as usize).min(h.saturating_sub(1)));
    let (mut sum, mut n) = (0f32, 0f32);
    for y in y0..=y1 {
        for x in x0..=x1 {
            let (pu, pv) = (x as f32 * rect.u[0] + y as f32 * rect.u[1], x as f32 * rect.v[0] + y as f32 * rect.v[1]);
            if pu >= rect.u0 - 0.5 && pu <= rect.u1 + 0.5 && pv >= rect.v0 - 0.5 && pv <= rect.v1 + 0.5 {
                sum += prob.get(y * w + x).copied().unwrap_or(0.0);
                n += 1.0;
            }
        }
    }
    if n > 0.0 { sum / n } else { 0.0 }
}

/// The page-pixel [`Quad`] of one detector component, or `None` when it is too small, too
/// faint or degenerate. `map` takes detector-map coordinates to page coordinates.
fn quad_of(points: &[[f32; 2]], prob: &[f32], (w, h): (usize, usize), scale: [f32; 2], offset: f32) -> Option<Quad> {
    let rect = min_area_rect(points)?;
    let (lu, lv) = (rect.u1 - rect.u0, rect.v1 - rect.v0);
    if lu.min(lv) < MIN_SIZE || box_score(prob, w, h, &rect) < BOX_THRESH {
        return None;
    }
    // Unclip: DB grows the box by area * ratio / perimeter on every side.
    let d = lu * lv * UNCLIP / (2.0 * (lu + lv));
    let (bu, bv) = (lu + 2.0 * d, lv + 2.0 * d);
    if bu.min(bv) < MIN_SIZE + 2.0 {
        return None;
    }
    let (cu, cv) = ((rect.u0 + rect.u1) / 2.0, (rect.v0 + rect.v1) / 2.0);
    let c = [cu * rect.u[0] + cv * rect.v[0], cu * rect.u[1] + cv * rect.v[1]];
    // The long side is the line; point it right (or down for a vertical run) and `down` below it.
    let (mut right, len, hgt) = if bu >= bv { (rect.u, bu, bv) } else { (rect.v, bv, bu) };
    if right[0] < 0.0 || (right[0].abs() < 1e-6 && right[1] < 0.0) {
        right = [-right[0], -right[1]];
    }
    let mut down = [-right[1], right[0]];
    if down[1] < 0.0 {
        down = [-down[0], -down[1]];
    }
    // The map is anisotropic only by rounding to multiples of 32: scale each axis.
    let (sx, sy) = (scale[0], scale[1]);
    let len_scale = (right[0] * sx).hypot(right[1] * sy);
    let hgt_scale = (down[0] * sx).hypot(down[1] * sy);
    Some(Quad { c: [c[0] * sx - offset, c[1] * sy - offset], right, down, len: len * len_scale, hgt: hgt * hgt_scale })
}

/// A box cut out and flattened to [`REC_H`] rows of grey: dark text on white whatever the page
/// looked like. Each column's background is the median of the crop's top and bottom margin rows,
/// smoothed along the line; the text is whatever differs from it, light or dark.
fn crop_flat(img: &Rgb, q: &Quad) -> Option<(Vec<u8>, usize)> {
    if q.hgt < 1.0 || q.len < 1.0 {
        return None;
    }
    let w = ((REC_H as f32 * q.len / q.hgt).ceil() as usize).clamp(16, MAX_W);
    let mut grey = Vec::with_capacity(w * REC_H);
    for v in 0..REC_H {
        for u in 0..w {
            let a = ((u as f32 + 0.5) / w as f32 - 0.5) * q.len;
            let b = ((v as f32 + 0.5) / REC_H as f32 - 0.5) * q.hgt;
            let p = img.sample(q.c[0] + a * q.right[0] + b * q.down[0] - 0.5, q.c[1] + a * q.right[1] + b * q.down[1] - 0.5);
            grey.push((0.299 * p[0] + 0.587 * p[1] + 0.114 * p[2]).round().clamp(0.0, 255.0) as u8);
        }
    }
    Some((flatten(&grey, w), w))
}

fn median(v: &mut [u8]) -> u8 {
    v.sort_unstable();
    v.get(v.len() / 2).copied().unwrap_or(255)
}

/// The flatten step of [`crop_flat`] on `grey` (`REC_H` rows of `w`).
fn flatten(grey: &[u8], w: usize) -> Vec<u8> {
    let k = (REC_H / 6).max(2);
    let col = |x: usize, rows: std::ops::Range<usize>| rows.filter_map(move |y| grey.get(y * w + x).copied());
    let bg0: Vec<u8> = (0..w).map(|x| median(&mut col(x, 0..k).chain(col(x, REC_H - k..REC_H)).collect::<Vec<u8>>())).collect();
    let win = REC_H | 1;
    let half = win / 2;
    let bg: Vec<f32> = (0..w)
        .map(|x| {
            let mut v: Vec<u8> = (0..win).filter_map(|i| bg0.get((x + i).saturating_sub(half).min(w.saturating_sub(1))).copied()).collect();
            f32::from(median(&mut v))
        })
        .collect();
    let diff: Vec<f32> = grey.iter().enumerate().map(|(i, &g)| (f32::from(g) - bg.get(i % w.max(1)).copied().unwrap_or(255.0)).abs()).collect();
    let mut hist = [0u32; 256];
    for &d in &diff {
        if let Some(c) = hist.get_mut((d as usize).min(255)) {
            *c += 1;
        }
    }
    let total = diff.len() as f32;
    let (mut acc, mut hi) = (0f32, 255usize);
    for (i, &c) in hist.iter().enumerate() {
        acc += c as f32;
        if acc >= 0.98 * total {
            hi = i;
            break;
        }
    }
    let hi = (hi as f32).max(24.0);
    diff.iter().map(|&d| (255.0 - d * (255.0 / hi)).clamp(0.0, 255.0) as u8).collect()
}

/// Greedy CTC decoding of `probs` (`classes` per time step, class 0 the blank): the text and the
/// mean probability of the characters kept (0 when there are none). Class `k` is `chars[k]`.
fn ctc_decode(probs: &[f32], classes: usize, chars: &[char]) -> (String, f32) {
    let (mut text, mut conf, mut kept, mut last) = (String::new(), 0f32, 0u32, 0usize);
    for row in probs.chunks_exact(classes.max(1)) {
        let (best, p) = row.iter().enumerate().filter(|(_, p)| p.is_finite()).fold((0usize, f32::MIN), |a, (i, &v)| if v > a.1 { (i, v) } else { a });
        if best != 0
            && best != last
            && let Some(&c) = chars.get(best)
        {
            text.push(c);
            conf += p;
            kept += 1;
        }
        last = best;
    }
    (text, if kept == 0 { 0.0 } else { conf / kept as f32 })
}

fn is_thai(c: char) -> bool {
    ('\u{0e01}'..='\u{0e5b}').contains(&c)
}

/// Latin letters inside words that also hold Thai letters: the look-alike misreads (บ as U) the
/// second recognizer exists to fix.
fn mixed(text: &str) -> usize {
    text.split_whitespace().filter(|w| w.chars().any(is_thai)).map(|w| w.chars().filter(char::is_ascii_alphabetic).count()).sum()
}

/// One canonical spelling of Thai text: zero-width characters dropped, the marks after a base in
/// one order (vowel, tone, then nikhahit), nikhahit + sara aa composed to sara am (the
/// recognizer emits the two), repeated marks and doubled sara e collapsed. Thai digits stay Thai.
fn canonical(text: &str) -> String {
    const VOWELS: &str = "\u{0e31}\u{0e34}\u{0e35}\u{0e36}\u{0e37}\u{0e3a}\u{0e38}\u{0e39}\u{0e47}";
    const TONES: &str = "\u{0e48}\u{0e49}\u{0e4a}\u{0e4b}\u{0e4c}\u{0e4e}";
    const NIKHAHIT: char = '\u{0e4d}';
    let rank = |c: char| {
        if VOWELS.contains(c) {
            0
        } else if TONES.contains(c) {
            1
        } else {
            2
        }
    };
    let is_mark = |c: char| VOWELS.contains(c) || TONES.contains(c) || c == NIKHAHIT;
    let chars: Vec<char> = text
        .chars()
        .filter(|c| !"\u{200b}\u{200c}\u{200d}\u{2060}\u{ad}\u{feff}".contains(*c))
        .map(|c| if c == '\u{a0}' { ' ' } else { c })
        .collect();
    let mut out: Vec<char> = Vec::with_capacity(chars.len());
    let mut i = 0;
    while let Some(&c) = chars.get(i) {
        if !is_mark(c) {
            out.push(c);
            i += 1;
            continue;
        }
        let end = i + chars.iter().skip(i).take_while(|&&m| is_mark(m)).count();
        let mut run: Vec<char> = chars.get(i..end).unwrap_or_default().to_vec();
        run.sort_by_key(|&m| rank(m));
        run.dedup();
        let sara_aa = chars.get(end) == Some(&'\u{0e32}');
        if run.contains(&NIKHAHIT) && sara_aa {
            run.retain(|&m| m != NIKHAHIT);
            out.extend(run);
            out.push('\u{0e33}');
            i = end + 1;
        } else {
            out.extend(run);
            i = end;
        }
    }
    let s: String = out.into_iter().collect();
    s.replace("\u{0e40}\u{0e40}", "\u{0e41}").replace("\u{0e33}\u{0e33}", "\u{0e33}")
}

/// One recognised box before it becomes a [`Line`].
struct Read {
    text: String,
    conf: f32,
    bounds: [f32; 4],
    centre_y: f32,
    left: f32,
}

/// The loaded model set. Loading takes a moment; keep one and reuse it.
pub struct OpenThai {
    det: Model,
    rec: Model,
    rec2: Option<Model>,
    chars: Vec<char>,
}

fn load_model(path: &Path) -> Result<Model, OcrError> {
    Model::load_file(path).map_err(|e| OcrError::Load(path.display().to_string(), e.to_string()))
}

/// The class table: the blank, the dictionary's characters (one per line), then the space the
/// training appended.
fn class_table(dictionary: &str) -> Vec<char> {
    let mut chars = vec!['\0'];
    chars.extend(dictionary.lines().filter_map(|l| l.chars().next()));
    chars.push(' ');
    chars
}

impl OpenThai {
    /// The model set in `dir`; the second recognizer is optional.
    pub fn load(dir: &Path) -> Result<OpenThai, OcrError> {
        let dict_path = dir.join(DICT);
        let dictionary = std::fs::read_to_string(&dict_path).map_err(|e| OcrError::Load(dict_path.display().to_string(), e.to_string()))?;
        let rec2_path = dir.join(REC2_MODEL);
        Ok(OpenThai {
            det: load_model(&dir.join(DET_MODEL))?,
            rec: load_model(&dir.join(REC_MODEL))?,
            rec2: if rec2_path.is_file() { Some(load_model(&rec2_path)?) } else { None },
            chars: class_table(&dictionary),
        })
    }

    /// Whether `dir` holds the required files.
    pub fn in_dir(dir: &Path) -> bool {
        [DET_MODEL, REC_MODEL, DICT].iter().all(|f| dir.join(f).is_file())
    }

    /// The first directory of [`crate::Models::search_dirs`] that holds the model set.
    pub fn find_dir() -> Option<PathBuf> {
        crate::Models::search_dirs().into_iter().find(|d| Self::in_dir(d))
    }

    /// Load the model set found by [`OpenThai::find_dir`].
    pub fn find() -> Result<OpenThai, OcrError> {
        Self::load(&Self::find_dir().ok_or_else(|| OcrError::NoLanguageModel("th".into()))?)
    }

    /// Read the lines of `image`, in reading order.
    pub fn read(&self, image: &OcrImage) -> Result<Vec<Line>, OcrError> {
        let page = Rgb::from_image(image);
        let quads = self.detect(&page)?;
        let median_h = typical_height(&quads);
        let mut reads = Vec::new();
        for q in &quads {
            if let Some(read) = self.read_box(&page, q, median_h)? {
                reads.push(read);
            }
        }
        Ok(into_lines(reads, median_h))
    }

    fn detect(&self, page: &Rgb) -> Result<Vec<Quad>, OcrError> {
        let pad = ((0.05 * page.w.min(page.h) as f32) as usize).max(8);
        let padded = page.padded(pad);
        let (x, rh, rw) = det_input(&padded);
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let input = Value::from_shape([1, 3, rh, rw], x).map_err(|e| err(&e))?;
        let out = self.det.run_one(input.into(), None).map_err(|e| err(&e))?;
        let (shape, prob): ([usize; 4], Vec<f32>) = out.into_shape_vec().map_err(|e| err(&e))?;
        let (ph, pw) = (shape.get(2).copied().unwrap_or(0), shape.get(3).copied().unwrap_or(0));
        if ph == 0 || pw == 0 || prob.len() != ph.saturating_mul(pw) {
            return Err(OcrError::Recognize("the text detector returned an unexpected shape".into()));
        }
        let scale = [padded.w as f32 / pw as f32, padded.h as f32 / ph as f32];
        Ok(components(&prob, pw, ph).iter().filter_map(|pts| quad_of(pts, &prob, (pw, ph), scale, pad as f32)).collect())
    }

    /// Run one recognizer on a flattened crop.
    fn run(model: &Model, grey: &[u8], w: usize, chars: &[char]) -> Result<(String, f32), OcrError> {
        let err = |e: &dyn std::fmt::Display| OcrError::Recognize(e.to_string());
        let plane = REC_H * w;
        let mut x = Vec::with_capacity(3 * plane);
        for _ in 0..3 {
            x.extend(grey.iter().map(|&g| (f32::from(g) / 255.0 - 0.5) / 0.5));
        }
        let input = Value::from_shape([1, 3, REC_H, w], x).map_err(|e| err(&e))?;
        let out = model.run_one(input.into(), None).map_err(|e| err(&e))?;
        let (shape, probs): ([usize; 3], Vec<f32>) = out.into_shape_vec().map_err(|e| err(&e))?;
        Ok(ctc_decode(&probs, shape.get(2).copied().unwrap_or(0), chars))
    }

    /// Recognise one box; `None` when the reading is dropped by the line filter.
    fn read_box(&self, page: &Rgb, q: &Quad, median_h: f32) -> Result<Option<Read>, OcrError> {
        let Some((grey, w)) = crop_flat(page, q) else { return Ok(None) };
        let (mut text, mut conf) = Self::run(&self.rec, &grey, w, &self.chars)?;
        if let Some(rec2) = self.rec2.as_ref().filter(|_| mixed(&text) > 0) {
            let (t2, c2) = Self::run(rec2, &grey, w, &self.chars)?;
            if mixed(&t2) < mixed(&text) {
                (text, conf) = (t2, c2);
            }
        }
        let text = canonical(&text);
        if !keep(&text, conf, q.hgt / median_h) {
            return Ok(None);
        }
        let bounds = q.bounds();
        Ok(Some(Read { text, conf, bounds, centre_y: q.c[1], left: bounds[0] }))
    }
}

/// The page's typical line height: the median over boxes at least twice as wide as tall.
fn typical_height(quads: &[Quad]) -> f32 {
    let mut hs: Vec<f32> = quads.iter().filter(|q| q.len >= 2.0 * q.hgt).map(|q| q.hgt).collect();
    if hs.len() < 3 {
        hs = quads.iter().map(|q| q.hgt).collect();
    }
    hs.sort_by(f32::total_cmp);
    hs.get(hs.len() / 2).copied().filter(|h| *h > 0.0).unwrap_or(1.0)
}

/// The line filter: a reading is kept when it is confident and long enough; a lone digit or
/// Latin letter needs a box about as tall as the page's lines (marks and ornaments are shorter).
fn keep(text: &str, conf: f32, height_ratio: f32) -> bool {
    let bare: Vec<char> = text.chars().filter(|c| *c != ' ').collect();
    if bare.is_empty() || conf < DROP_BELOW {
        return false;
    }
    let digit = |c: &char| c.is_ascii_digit() || ('\u{0e50}'..='\u{0e59}').contains(c);
    let short_ok = bare.iter().all(|c| digit(c) || c.is_ascii_alphabetic()) && height_ratio >= SHORT_HEIGHT;
    bare.len() >= MIN_CHARS || short_ok
}

/// Reading order: rows by vertical proximity (half a line height), each left to right; one
/// [`Line`] per box, its words sharing the box's width in proportion to their length.
fn into_lines(mut reads: Vec<Read>, median_h: f32) -> Vec<Line> {
    reads.sort_by(|a, b| a.centre_y.total_cmp(&b.centre_y));
    let mut rows: Vec<Vec<Read>> = Vec::new();
    for r in reads {
        match rows.last_mut() {
            Some(row) if row.first().is_some_and(|f| (r.centre_y - f.centre_y).abs() < 0.5 * median_h) => row.push(r),
            _ => rows.push(vec![r]),
        }
    }
    rows.into_iter()
        .flat_map(|mut row| {
            row.sort_by(|a, b| a.left.total_cmp(&b.left));
            row.into_iter().map(|r| line_of(&r))
        })
        .collect()
}

fn line_of(r: &Read) -> Line {
    let [left, top, right, bottom] = r.bounds;
    let parts: Vec<&str> = r.text.split_whitespace().collect();
    let total: usize = parts.iter().map(|p| p.chars().count().max(1)).sum();
    let mut x = left;
    let words = parts
        .iter()
        .map(|p| {
            let span = (right - left) * (p.chars().count().max(1) as f32 / total.max(1) as f32);
            let word = Word::new(*p, [x, top, x + span, bottom], "openthai").with_confidence(r.conf * 100.0);
            x += span;
            word
        })
        .collect();
    Line { words }
}

/// The OpenThai engine as a [`Recognizer`], loading its models on first use.
#[derive(Default)]
pub struct OpenThaiRecognizer {
    engine: OnceLock<Option<OpenThai>>,
}

impl OpenThaiRecognizer {
    pub fn new() -> OpenThaiRecognizer {
        OpenThaiRecognizer::default()
    }

    /// The loaded model set, or `None` when it is not installed.
    pub fn engine(&self) -> Option<&OpenThai> {
        self.engine.get_or_init(|| OpenThai::find().ok()).as_ref()
    }
}

impl Recognizer for OpenThaiRecognizer {
    fn id(&self) -> &'static str {
        "openthai"
    }

    fn available(&self) -> bool {
        self.engine.get().map_or_else(|| OpenThai::find_dir().is_some(), Option::is_some)
    }

    fn languages(&self) -> Vec<(String, String)> {
        LANGUAGES.iter().map(|(c, n)| ((*c).to_string(), (*n).to_string())).collect()
    }

    fn recognize(&self, image: &OcrImage, _options: &RecognizeOptions) -> Result<Vec<Line>, OcrError> {
        self.engine().ok_or_else(|| OcrError::NoLanguageModel("th".into()))?.read(image)
    }
}

#[cfg(test)]
mod tests {
    use proptest::prelude::*;

    use super::*;

    /// The thin wide hull that `rten-imageproc`'s `min_area_rect` measured as under a pixel
    /// tall: its true minimum rectangle is 631 × 9.
    #[test]
    fn a_wide_line_component_has_its_true_minimum_rectangle() {
        let pts: Vec<[f32; 2]> = (0..=630).flat_map(|x| [[x as f32, 0.0], [x as f32, 9.0]]).collect();
        let r = min_area_rect(&pts).unwrap_or(Rect { u: [1.0, 0.0], v: [0.0, 1.0], u0: 0.0, u1: 0.0, v0: 0.0, v1: 0.0 });
        let (a, b) = ((r.u1 - r.u0).max(r.v1 - r.v0), (r.u1 - r.u0).min(r.v1 - r.v0));
        assert!((a - 630.0).abs() < 1e-3 && (b - 9.0).abs() < 1e-3, "{a} x {b}");
    }

    #[test]
    fn a_tilted_rectangle_is_found_at_its_angle() {
        let (c, s) = (0.2f32.cos(), 0.2f32.sin());
        let pts: Vec<[f32; 2]> =
            [[0.0, 0.0], [100.0, 0.0], [100.0, 10.0], [0.0, 10.0]].iter().map(|p| [p[0] * c - p[1] * s, p[0] * s + p[1] * c]).collect();
        let r = min_area_rect(&pts).unwrap_or(Rect { u: [1.0, 0.0], v: [0.0, 1.0], u0: 0.0, u1: 0.0, v0: 0.0, v1: 0.0 });
        let (a, b) = ((r.u1 - r.u0).max(r.v1 - r.v0), (r.u1 - r.u0).min(r.v1 - r.v0));
        assert!((a - 100.0).abs() < 0.01 && (b - 10.0).abs() < 0.01, "{a} x {b}");
    }

    #[test]
    fn degenerate_point_sets_have_no_rectangle_and_never_panic() {
        assert!(min_area_rect(&[]).is_none());
        assert!(min_area_rect(&[[1.0, 1.0]]).is_none());
        assert!(min_area_rect(&[[1.0, 1.0], [1.0, 1.0], [1.0, 1.0]]).is_none());
        let _ = min_area_rect(&[[0.0, 0.0], [5.0, 0.0], [10.0, 0.0]]);
        let _ = min_area_rect(&[[f32::NAN, 0.0], [f32::INFINITY, 1.0], [2.0, 3.0]]);
    }

    #[test]
    fn components_are_found_with_eight_connectivity() {
        // Two blobs: a 3 x 2 block and, diagonally joined, one pixel; a third blob far away.
        let (w, h) = (8usize, 4usize);
        let mut prob = vec![0.0f32; w * h];
        for (x, y) in [(0, 0), (1, 0), (2, 0), (0, 1), (1, 1), (2, 1), (3, 2), (7, 3)] {
            if let Some(p) = prob.get_mut(y * w + x) {
                *p = 0.9;
            }
        }
        assert_eq!(components(&prob, w, h).len(), 2);
    }

    #[test]
    fn a_blank_page_has_no_components() {
        assert!(components(&vec![0.0; 64], 8, 8).is_empty());
        assert!(components(&[], 0, 0).is_empty());
    }

    #[test]
    fn ctc_collapses_repeats_and_blanks_and_reports_confidence() {
        let chars = ['\0', 'ก', 'า', ' '];
        // classes: blank, ก, า, space; steps: ก ก blank า
        let probs = [0.0, 0.9, 0.0, 0.0, 0.0, 0.8, 0.0, 0.0, 0.9, 0.0, 0.0, 0.0, 0.0, 0.0, 0.7, 0.0];
        let (text, conf) = ctc_decode(&probs, 4, &chars);
        assert_eq!(text, "กา");
        assert!((conf - 0.8).abs() < 1e-6, "{conf}");
        assert_eq!(ctc_decode(&[], 4, &chars), (String::new(), 0.0));
        assert_eq!(ctc_decode(&[1.0], 0, &chars).0, "");
    }

    #[test]
    fn class_table_matches_the_models_class_count() {
        let t = class_table("ก\nข\n");
        assert_eq!(t, vec!['\0', 'ก', 'ข', ' ']);
    }

    #[test]
    fn nikhahit_and_sara_aa_become_sara_am_and_marks_are_ordered() {
        assert_eq!(canonical("นํา"), "นำ");
        assert_eq!(canonical("น\u{0e4d}\u{0e49}า"), "น\u{0e49}\u{0e33}");
        assert_eq!(canonical("ก\u{0e48}\u{0e34}"), "ก\u{0e34}\u{0e48}", "vowel before tone");
        assert_eq!(canonical("ก\u{0e48}\u{0e48}"), "ก\u{0e48}", "a doubled mark is one");
        assert_eq!(canonical("เเก\u{200b}"), "แก");
        assert_eq!(canonical("๑๒๓"), "๑๒๓", "Thai digits stay Thai");
    }

    #[test]
    fn mixed_script_words_are_counted() {
        assert_eq!(mixed("Uไทย hello"), 1);
        assert_eq!(mixed("hello world"), 0);
        assert_eq!(mixed("ไทย"), 0);
    }

    #[test]
    fn the_line_filter_drops_faint_and_short_readings() {
        assert!(keep("สวัสดี", 0.95, 1.0));
        assert!(!keep("สวัสดี", 0.5, 1.0), "faint");
        assert!(!keep("ก", 0.99, 1.0), "one Thai letter is a mark");
        assert!(keep("2", 0.99, 0.9), "a tall lone digit is a table cell");
        assert!(!keep("2", 0.99, 0.3), "a short lone digit is an ornament");
        assert!(!keep("  ", 0.99, 1.0));
    }

    #[test]
    fn lines_are_ordered_in_rows_then_left_to_right() {
        let read = |t: &str, x: f32, y: f32| Read { text: t.into(), conf: 0.9, bounds: [x, y - 10.0, x + 50.0, y + 10.0], centre_y: y, left: x };
        let lines = into_lines(vec![read("c", 10.0, 100.0), read("b", 200.0, 52.0), read("a", 10.0, 50.0)], 20.0);
        assert_eq!(lines.iter().map(Line::text).collect::<Vec<_>>(), ["a", "b", "c"]);
    }

    #[test]
    fn words_share_the_boxs_width_by_length() {
        let r = Read { text: "ab cdef".into(), conf: 0.9, bounds: [0.0, 0.0, 60.0, 10.0], centre_y: 5.0, left: 0.0 };
        let l = line_of(&r);
        assert_eq!(l.words.len(), 2);
        assert_eq!(l.words.first().map(|w| w.rect), Some([0.0, 0.0, 20.0, 10.0]));
        assert_eq!(l.words.get(1).map(|w| w.rect), Some([20.0, 0.0, 60.0, 10.0]));
        assert_eq!(l.words.first().and_then(|w| w.confidence), Some(90.0));
    }

    #[test]
    fn flatten_turns_a_flat_crop_white() {
        let w = 64;
        let flat = flatten(&vec![200u8; w * REC_H], w);
        assert!(flat.iter().all(|&v| v == 255), "no ink, no difference from the background");
    }

    #[test]
    fn flatten_keeps_dark_and_light_ink_dark_on_white() {
        let w = 64;
        for (bg, ink) in [(255u8, 0u8), (0u8, 255u8)] {
            let mut g = vec![bg; w * REC_H];
            for y in 16..32 {
                for x in 20..40 {
                    if let Some(p) = g.get_mut(y * w + x) {
                        *p = ink;
                    }
                }
            }
            let f = flatten(&g, w);
            assert_eq!(f.get(24 * w + 30).copied(), Some(0), "ink on bg {bg} is dark");
            assert_eq!(f.get(2 * w + 2).copied(), Some(255));
        }
    }

    #[test]
    fn missing_models_are_not_available_and_error_cleanly() {
        let dir = std::env::temp_dir().join("pdfcraft-openthai-none");
        assert!(!OpenThai::in_dir(&dir));
        assert!(matches!(OpenThai::load(&dir), Err(OcrError::Load(..))));
    }

    proptest! {
        #[test]
        fn canonical_never_panics_and_is_stable(s in "\\PC{0,60}|[\u{0e01}-\u{0e5b}]{0,40}") {
            let once = canonical(&s);
            prop_assert_eq!(canonical(&once), once);
        }

        #[test]
        fn ctc_decode_never_panics(probs in proptest::collection::vec(-1.0f32..2.0, 0..200), classes in 0usize..12) {
            let chars: Vec<char> = "\0กขค ".chars().collect();
            let _ = ctc_decode(&probs, classes, &chars);
        }

        #[test]
        fn min_area_rect_never_panics_and_covers_its_points(pts in proptest::collection::vec((0.0f32..500.0, 0.0f32..500.0), 0..60)) {
            let pts: Vec<[f32; 2]> = pts.into_iter().map(|(x, y)| [x, y]).collect();
            if let Some(r) = min_area_rect(&pts) {
                for p in &pts {
                    let (pu, pv) = (p[0] * r.u[0] + p[1] * r.u[1], p[0] * r.v[0] + p[1] * r.v[1]);
                    prop_assert!(pu >= r.u0 - 1e-2 && pu <= r.u1 + 1e-2 && pv >= r.v0 - 1e-2 && pv <= r.v1 + 1e-2);
                }
            }
        }

        #[test]
        fn components_never_panic(w in 0usize..12, h in 0usize..12, bits in proptest::collection::vec(0u8..2, 0..150)) {
            let prob: Vec<f32> = bits.iter().map(|&b| f32::from(b)).collect();
            let _ = components(&prob, w, h);
        }

        #[test]
        fn the_flatten_step_never_panics(w in 1usize..40, fill in 0u8..=255) {
            let out = flatten(&vec![fill; w * REC_H], w);
            prop_assert_eq!(out.len(), w * REC_H);
        }
    }
}
