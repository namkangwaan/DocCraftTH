//! Auto-deskew and orientation rectification for scanned pages, before recognition.
//!
//! Pipeline (pure Rust, no OpenCV): the page is box-downsampled to about [`ANALYSIS_SIDE`] px,
//! ink is extracted with an adaptive (local-mean) threshold, and everything that is not text is
//! suppressed: an edge margin, black scanner borders, and large connected blobs (seals such as
//! the Garuda, frames, vertical rules). Dot leaders and horizontal rules are *kept*: they run
//! along the text lines and sharpen the estimate. The surviving ink pixels are scored with a
//! projection-profile sharpness (Σ Δ² of a sheared row histogram) in three stages, ±20° at 1°,
//! ±1.5° at 0.2°, ±0.3° at 0.05° with a parabolic refinement. Quarter turns are decided first,
//! skew-robustly, by comparing the best score of the upright and the 90°-turned ink.
//!
//! A confidence gate (profile prominence over the coarse curve) and a deadband keep straight or
//! textless pages untouched. The warp keeps the page size, fills with the paper colour, shrinks
//! the content only as far as needed to keep its ink inside the canvas, and returns an exact
//! f64 [`InverseTransform`] so word boxes map back to the input raster.

use crate::preprocess::{InverseTransform, band_bottom_heaviness, compose, inverse_quarter_turns, rotate_quarter_turns};
use crate::{OcrError, OcrImage};

/// Long side, in pixels, of the raster the skew search runs on.
pub const ANALYSIS_SIDE: f64 = 1100.0;

/// Fewest text-like connected components (after blob suppression) a page needs to be judged.
const MIN_COMPONENTS: usize = 30;

/// Fewest ink points in the scoring area a page needs to be judged.
const MIN_POINTS: usize = 300;

/// Cap on the ink points the search scores (more are strided away).
const MAX_POINTS: usize = 160_000;

/// How pixels are resampled by the warp.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Resample {
    /// Bilinear: no overshoot, so thin Thai tone marks never ring.
    #[default]
    Bilinear,
    /// Catmull-Rom bicubic: sharper, with slight overshoot (clamped).
    CatmullRom,
}

/// Tunables; [`DeskewOptions::default`] is the shipped configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DeskewOptions {
    /// Largest skew searched, in degrees (clamped to 1…45).
    pub max_angle_deg: f64,
    /// Smallest skew that is corrected, in degrees: anything under is left as scanned.
    pub deadband_deg: f64,
    /// Smallest confidence (0–1) at which a detected angle is applied.
    pub min_confidence: f64,
    /// Also look for 90°/180°/270° pages. Off by default: on 7,000 real scans the ink-only cue
    /// turned upright numeric tables and wide forms on their side (3 of 8 mid-range cases checked by
    /// eye; no ratio threshold separated them) and flipped upright pages upside down, so a turn is
    /// only a candidate until a recogniser confirms it (as the 180° retry in the engine does).
    pub detect_orientation: bool,
    /// How much sharper the 90°-turned ink must score than the upright ink to call a page sideways.
    pub axis_ratio_min: f64,
    pub resample: Resample,
}

impl Default for DeskewOptions {
    fn default() -> Self {
        DeskewOptions {
            max_angle_deg: 20.0,
            deadband_deg: 0.2,
            min_confidence: 0.2,
            detect_orientation: false,
            axis_ratio_min: 1.3,
            resample: Resample::Bilinear,
        }
    }
}

/// What the detector found, and what it decided to do about it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct DeskewReport {
    /// The measured skew in degrees (positive: lines run downhill to the right), after any
    /// quarter turn. `0.0` when nothing could be measured.
    pub detected_deg: f64,
    /// The rotation actually applied (`-detected_deg` is undone; this is the magnitude/sign of the
    /// skew that was corrected): `detected_deg` when the gate passed, else `0.0`.
    pub applied_deg: f64,
    /// 0–1: how clearly the line structure stands out; low values mean "do not trust".
    pub confidence: f64,
    /// Clockwise quarter turns that bring the page upright (0–3).
    pub orientation_turns: u32,
    /// 0–1 certainty of `orientation_turns`. The 0°/180° choice is a weak ink-distribution cue
    /// (a Latin assumption that Thai does not share), so it only fires on a wide margin; a caller
    /// with a recogniser should verify it by comparing recognition scores.
    pub orientation_confidence: f64,
    /// Score of the 90°-turned ink over that of the upright ink (0 when orientation was not
    /// searched): above [`DeskewOptions::axis_ratio_min`] the page is judged to lie on its side.
    pub axis_ratio: f64,
}

impl DeskewReport {
    /// Whether applying this report changes the raster at all.
    pub fn changes_page(&self) -> bool {
        self.applied_deg != 0.0 || self.orientation_turns != 0
    }
}

/// A straightened page with the exact way back.
#[derive(Debug)]
pub struct Straightened {
    pub image: OcrImage,
    /// Maps pixel-edge coordinates in `image` back to the input raster's.
    pub inverse_transform: InverseTransform,
    pub report: DeskewReport,
}

/// A point of ink, relative to the page centre, in analysis pixels.
type Pt = (f32, f32);

struct Analysis {
    points: Vec<Pt>,
    /// Bounding box `[x0, y0, x1, y1]` of the text-like ink (edge margin included), relative to
    /// the centre, in analysis pixels.
    ink_bbox: Option<[f32; 4]>,
    /// Analysis pixels per source pixel.
    scale: f64,
    paper: [u8; 3],
}

/// Detect the skew and orientation of `image` without changing it.
pub fn analyze(image: &OcrImage, options: &DeskewOptions) -> DeskewReport {
    match build_analysis(image) {
        Some(a) => decide(&a, options).0,
        None => DeskewReport::default(),
    }
}

/// Detect and correct: quarter turns (an exact permutation), then the rotation by the detected
/// angle. The page size is kept (swapped for an odd turn). A page the gate does not pass comes back
/// as an identical copy with the identity transform.
pub fn straighten(image: &OcrImage, options: &DeskewOptions) -> Result<Straightened, OcrError> {
    let Some(analysis) = build_analysis(image) else {
        return Ok(Straightened { image: image.clone(), inverse_transform: InverseTransform::identity(), report: DeskewReport::default() });
    };
    let (report, turned_bbox) = decide(&analysis, options);
    apply(image, &analysis, report, turned_bbox, options.resample)
}

/// Apply an already-made decision (what [`straighten`] does after [`analyze`]); `report`'s
/// `applied_deg` and `orientation_turns` are the only fields read.
pub fn apply_report(image: &OcrImage, report: &DeskewReport, resample: Resample) -> Result<Straightened, OcrError> {
    let Some(analysis) = build_analysis(image) else {
        return Ok(Straightened { image: image.clone(), inverse_transform: InverseTransform::identity(), report: *report });
    };
    let bbox = turn_bbox(analysis.ink_bbox, report.orientation_turns);
    apply(image, &analysis, *report, bbox, resample)
}

fn apply(
    image: &OcrImage,
    analysis: &Analysis,
    report: DeskewReport,
    turned_bbox: Option<[f32; 4]>,
    resample: Resample,
) -> Result<Straightened, OcrError> {
    if !report.changes_page() {
        return Ok(Straightened { image: image.clone(), inverse_transform: InverseTransform::identity(), report });
    }
    let (mut current, mut inverse) = (image.clone(), InverseTransform::identity());
    let turns = report.orientation_turns % 4;
    if turns != 0 {
        let (w, h) = (current.width(), current.height());
        current = rotate_quarter_turns(&current, turns);
        inverse = compose(&inverse, &inverse_quarter_turns(turns, w, h));
    }
    if report.applied_deg != 0.0 {
        let fit = fit_scale(current.width(), current.height(), report.applied_deg, turned_bbox, analysis.scale);
        let (rotated, back) = rotate_warp(&current, report.applied_deg, fit, analysis.paper, resample);
        current = rotated;
        inverse = compose(&inverse, &back);
    }
    Ok(Straightened { image: current, inverse_transform: inverse, report })
}

// ---------------------------------------------------------------------------------------------
// Analysis: downsample, ink, suppression
// ---------------------------------------------------------------------------------------------

fn build_analysis(image: &OcrImage) -> Option<Analysis> {
    if image.min_side() < 64 {
        return None;
    }
    let (sw, sh) = (image.width() as usize, image.height() as usize);
    let scale = (ANALYSIS_SIDE / sw.max(sh) as f64).min(1.0);
    let (w, h) = (((sw as f64 * scale).round() as usize).max(1), ((sh as f64 * scale).round() as usize).max(1));
    let gray = downsample_gray(image.rgba(), sw, sh, w, h);
    let paper_gray = percentile(&gray, 0.90);
    if paper_gray < 110 {
        return None; // a dark page: no ink/paper contrast to speak of
    }
    let mut ink = adaptive_ink(&gray, w, h, paper_gray);
    if !ink.iter().any(|&i| i) {
        return None;
    }
    let zone = edge_zone(&gray, w, h);
    let components = suppress_blobs(&mut ink, &zone, w, h);
    // A page needs enough text-like marks to have lines at all: a punch hole, a rubber stamp or a page
    // number on an otherwise empty sheet is not evidence (on real scans such pages were "measured" at
    // several degrees with high confidence).
    if components < MIN_COMPONENTS {
        return None;
    }
    let (cx, cy) = (w as f32 / 2.0, h as f32 / 2.0);
    let mut all = Vec::new();
    let mut bbox: Option<[f32; 4]> = None;
    for (y, row) in ink.chunks_exact(w).enumerate() {
        for (x, &on) in row.iter().enumerate() {
            if !on {
                continue;
            }
            let p = (x as f32 + 0.5 - cx, y as f32 + 0.5 - cy);
            bbox = Some(match bbox {
                None => [p.0, p.1, p.0, p.1],
                Some(b) => [b[0].min(p.0), b[1].min(p.1), b[2].max(p.0), b[3].max(p.1)],
            });
            if !zone.get(y * w + x).copied().unwrap_or(true) {
                all.push(p);
            }
        }
    }
    if all.len() < MIN_POINTS {
        return None;
    }
    let stride = all.len().div_ceil(MAX_POINTS);
    let points: Vec<Pt> = if stride > 1 { all.into_iter().step_by(stride).collect() } else { all };
    Some(Analysis { points, ink_bbox: bbox, scale, paper: paper_colour(image) })
}

/// Luma (Rec.601) of an RGBA raster, area-averaged into a `w`×`h` grid in one pass (every source
/// pixel lands in exactly one cell; the column map is computed once, not per pixel).
fn downsample_gray(rgba: &[u8], sw: usize, sh: usize, w: usize, h: usize) -> Vec<u8> {
    let cols: Vec<usize> = (0..sw).map(|x| (x * w / sw).min(w - 1)).collect();
    let mut sum = vec![0u32; w * h];
    let mut cnt = vec![0u32; w * h];
    for (y, row) in rgba.chunks_exact(sw * 4).enumerate() {
        let base = (y * h / sh).min(h - 1) * w;
        for (px, &cx) in row.as_chunks::<4>().0.iter().zip(&cols) {
            let g = (299 * u32::from(px[0]) + 587 * u32::from(px[1]) + 114 * u32::from(px[2])) / 1000;
            if let (Some(s), Some(c)) = (sum.get_mut(base + cx), cnt.get_mut(base + cx)) {
                *s += g;
                *c += 1;
            }
        }
    }
    sum.iter().zip(&cnt).map(|(&s, &c)| s.checked_div(c).map_or(255, |v| v as u8)).collect()
}

fn percentile(gray: &[u8], q: f64) -> u8 {
    let mut hist = [0usize; 256];
    for &g in gray {
        hist[g as usize] += 1;
    }
    let target = (gray.len() as f64 * q) as usize;
    let mut acc = 0;
    for (v, &n) in hist.iter().enumerate() {
        acc += n;
        if acc > target {
            return v as u8;
        }
    }
    255
}

/// Ink = darker than the local mean by a contrast margin (Bradley-style, integral image).
fn adaptive_ink(gray: &[u8], w: usize, h: usize, paper: u8) -> Vec<bool> {
    let r = (w.max(h) / 90).max(6);
    let stride = w + 1;
    let mut integral = vec![0u32; stride * (h + 1)];
    for y in 0..h {
        let mut row = 0u32;
        for x in 0..w {
            row += u32::from(gray.get(y * w + x).copied().unwrap_or(255));
            let above = integral.get(y * stride + x + 1).copied().unwrap_or(0);
            if let Some(d) = integral.get_mut((y + 1) * stride + x + 1) {
                *d = above + row;
            }
        }
    }
    let margin = (f64::from(paper) * 0.07).max(12.0) as i64;
    let at = |x: usize, y: usize| i64::from(integral.get(y * stride + x).copied().unwrap_or(0));
    let mut ink = vec![false; w * h];
    for y in 0..h {
        let (y0, y1) = (y.saturating_sub(r), (y + r + 1).min(h));
        for x in 0..w {
            let (x0, x1) = (x.saturating_sub(r), (x + r + 1).min(w));
            let area = ((x1 - x0) * (y1 - y0)) as i64;
            let mean = (at(x1, y1) - at(x0, y1) - at(x1, y0) + at(x0, y0)) / area.max(1);
            if let (Some(g), Some(o)) = (gray.get(y * w + x), ink.get_mut(y * w + x)) {
                *o = i64::from(*g) + margin < mean;
            }
        }
    }
    ink
}

/// The pixels scoring must ignore: a margin along every edge, widened past any black scanner
/// border that starts at the edge.
fn edge_zone(gray: &[u8], w: usize, h: usize) -> Vec<bool> {
    let base_x = (w as f64 * 0.03).ceil() as usize;
    let base_y = (h as f64 * 0.03).ceil() as usize;
    let dark_row = |y: usize| gray.chunks_exact(w).nth(y).map_or(0.0, |r| r.iter().filter(|&&g| g < 60).count() as f64 / w as f64);
    let dark_col = |x: usize| (0..h).filter(|&y| gray.get(y * w + x).is_some_and(|&g| g < 60)).count() as f64 / h as f64;
    let reach = |len: usize, cap: usize, dark: &dyn Fn(usize) -> f64, rev: bool| -> usize {
        let mut n = 0;
        while n < cap && dark(if rev { len - 1 - n } else { n }) > 0.3 {
            n += 1;
        }
        if n == 0 { 0 } else { n + (len as f64 * 0.01).ceil() as usize }
    };
    let (capx, capy) = ((w as f64 * 0.2) as usize, (h as f64 * 0.2) as usize);
    let left = base_x.max(reach(w, capx, &dark_col, false));
    let right = base_x.max(reach(w, capx, &dark_col, true));
    let top = base_y.max(reach(h, capy, &dark_row, false));
    let bottom = base_y.max(reach(h, capy, &dark_row, true));
    let mut zone = vec![false; w * h];
    for (y, row) in zone.chunks_exact_mut(w).enumerate() {
        for (x, z) in row.iter_mut().enumerate() {
            *z = x < left || x + right >= w || y < top || y + bottom >= h;
        }
    }
    zone
}

struct Comp {
    min: (u32, u32),
    max: (u32, u32),
    area: u32,
    touches_zone: bool,
}

/// Remove ink that is not text: connected blobs (8-connectivity) that are large in both
/// directions (seals, frames), very tall ones (vertical rules), and large ones touching the edge
/// zone (scanner-border edges).
fn suppress_blobs(ink: &mut [bool], zone: &[bool], w: usize, h: usize) -> usize {
    // parent[0] is unused so label 0 means background.
    let mut parent: Vec<u32> = vec![0];
    let mut labels = vec![0u32; w * h];
    fn find(parent: &mut [u32], mut a: u32) -> u32 {
        loop {
            let p = parent.get(a as usize).copied().unwrap_or(a);
            if p == a {
                return a;
            }
            let gp = parent.get(p as usize).copied().unwrap_or(p);
            if let Some(slot) = parent.get_mut(a as usize) {
                *slot = gp;
            }
            a = gp;
        }
    }
    for y in 0..h {
        for x in 0..w {
            if !ink.get(y * w + x).copied().unwrap_or(false) {
                continue;
            }
            let mut mine = 0u32;
            let neighbours = [(x.wrapping_sub(1), y), (x.wrapping_sub(1), y.wrapping_sub(1)), (x, y.wrapping_sub(1)), (x + 1, y.wrapping_sub(1))];
            for (nx, ny) in neighbours {
                if nx >= w || ny >= h {
                    continue;
                }
                let l = labels.get(ny * w + nx).copied().unwrap_or(0);
                if l == 0 {
                    continue;
                }
                if mine == 0 {
                    mine = find(&mut parent, l);
                } else {
                    let other = find(&mut parent, l);
                    if other != mine {
                        let (lo, hi) = (mine.min(other), mine.max(other));
                        if let Some(slot) = parent.get_mut(hi as usize) {
                            *slot = lo;
                        }
                        mine = lo;
                    }
                }
            }
            if mine == 0 {
                mine = parent.len() as u32;
                parent.push(mine);
            }
            if let Some(l) = labels.get_mut(y * w + x) {
                *l = mine;
            }
        }
    }
    let mut comps: Vec<Option<Comp>> = (0..parent.len()).map(|_| None).collect();
    for y in 0..h {
        for x in 0..w {
            let l = labels.get(y * w + x).copied().unwrap_or(0);
            if l == 0 {
                continue;
            }
            let root = find(&mut parent, l) as usize;
            let (xu, yu) = (x as u32, y as u32);
            let z = zone.get(y * w + x).copied().unwrap_or(false);
            if let Some(slot) = comps.get_mut(root) {
                match slot {
                    Some(c) => {
                        c.min = (c.min.0.min(xu), c.min.1.min(yu));
                        c.max = (c.max.0.max(xu), c.max.1.max(yu));
                        c.area += 1;
                        c.touches_zone |= z;
                    }
                    None => *slot = Some(Comp { min: (xu, yu), max: (xu, yu), area: 1, touches_zone: z }),
                }
            }
        }
    }
    let short = w.min(h) as f64;
    let remove = |c: &Comp| -> bool {
        let (bw, bh) = (f64::from(c.max.0 - c.min.0 + 1), f64::from(c.max.1 - c.min.1 + 1));
        let big_both = bw > 0.06 * short && bh > 0.06 * short;
        let tall = bh > 0.25 * h as f64 && bw < bh * 0.25;
        let big_edge = c.touches_zone && (bw > 0.2 * w as f64 || bh > 0.2 * h as f64);
        let huge_fill = bw * bh > 0.01 * (w * h) as f64 && f64::from(c.area) > 0.5 * bw * bh;
        big_both || tall || big_edge || huge_fill
    };
    let doomed: Vec<bool> = comps.iter().map(|c| c.as_ref().is_some_and(remove)).collect();
    for (i, slot) in ink.iter_mut().enumerate() {
        if *slot {
            let l = labels.get(i).copied().unwrap_or(0);
            let root = find(&mut parent, l) as usize;
            if doomed.get(root).copied().unwrap_or(false) {
                *slot = false;
            }
        }
    }
    // The text-like components that remain.
    comps.iter().zip(&doomed).filter(|(c, d)| c.is_some() && !**d).count()
}

/// Paper colour: the per-channel median of the brightest tenth of (strided) pixels.
fn paper_colour(image: &OcrImage) -> [u8; 3] {
    let px = image.rgba().as_chunks::<4>().0;
    let step = (px.len() / 20_000).max(1);
    let mut lum: Vec<(u32, [u8; 3])> =
        px.iter().step_by(step).map(|p| (299 * u32::from(p[0]) + 587 * u32::from(p[1]) + 114 * u32::from(p[2]), [p[0], p[1], p[2]])).collect();
    lum.sort_unstable_by_key(|v| std::cmp::Reverse(v.0));
    let top = &lum[..(lum.len() / 10).max(1).min(lum.len())];
    let mut out = [255u8; 3];
    for (c, o) in out.iter_mut().enumerate() {
        let mut v: Vec<u8> = top.iter().map(|t| t.1[c]).collect();
        v.sort_unstable();
        *o = v.get(v.len() / 2).copied().unwrap_or(255);
    }
    out
}

// ---------------------------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------------------------

/// Turn points `k` quarter turns clockwise about the centre: (x, y) → (−y, x) per turn.
fn turn_points(points: &[Pt], k: u32) -> Vec<Pt> {
    points
        .iter()
        .map(|&(x, y)| match k % 4 {
            1 => (-y, x),
            2 => (-x, -y),
            3 => (y, -x),
            _ => (x, y),
        })
        .collect()
}

fn turn_bbox(b: Option<[f32; 4]>, k: u32) -> Option<[f32; 4]> {
    let b = b?;
    let corners = [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])];
    let t = turn_points(&corners, k);
    let mut o = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    for (x, y) in t {
        o = [o[0].min(x), o[1].min(y), o[2].max(x), o[3].max(y)];
    }
    Some(o)
}

/// Row-histogram sharpness of the points sheared by `deg`: Σ (Δ bin)².
fn score(points: &[Pt], half_extent: f32, deg: f64) -> f64 {
    let (s, c) = (deg.to_radians().sin() as f32, deg.to_radians().cos() as f32);
    let bins = (2.0 * half_extent).ceil() as usize + 4;
    let mut hist = vec![0f32; bins];
    for &(x, y) in points {
        let r = y * c - x * s + half_extent + 1.0;
        let i = r.floor();
        let f = r - i;
        let i = i as isize;
        if i >= 0 && (i as usize) + 1 < bins {
            if let Some(b) = hist.get_mut(i as usize) {
                *b += 1.0 - f;
            }
            if let Some(b) = hist.get_mut(i as usize + 1) {
                *b += f;
            }
        }
    }
    hist.windows(2).map(|p| f64::from(p[1] - p[0]).powi(2)).sum()
}

/// The coarse sweep (±`max_deg` at 1°): where the sharpest rows are, how sharp the "no structure"
/// level is, and the score of the page as it is.
struct Coarse {
    angle: f64,
    best: f64,
    /// Median of the coarse curve (the "no structure" level).
    median: f64,
    at_zero: f64,
}

struct Search {
    angle: f64,
    best: f64,
    median: f64,
    at_zero: f64,
}

fn half_extent(points: &[Pt]) -> f32 {
    points.iter().fold(0f32, |m, p| m.max(p.0.abs()).max(p.1.abs())) * std::f32::consts::SQRT_2 + 2.0
}

fn coarse(points: &[Pt], max_deg: f64) -> Coarse {
    let half = half_extent(points);
    let n = max_deg.floor() as i32;
    let curve: Vec<(f64, f64)> = (-n..=n).map(|i| (f64::from(i), score(points, half, f64::from(i)))).collect();
    let mut sorted: Vec<f64> = curve.iter().map(|c| c.1).collect();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted.get(sorted.len() / 2).copied().unwrap_or(0.0);
    let (angle, best) = curve.iter().copied().fold((0.0, f64::MIN), |m, c| if c.1 > m.1 { c } else { m });
    let at_zero = curve.iter().find(|c| c.0 == 0.0).map_or_else(|| score(points, half, 0.0), |c| c.1);
    Coarse { angle, best, median, at_zero }
}

/// The fine stages (±1.5° at 0.2°, ±0.3° at 0.05° with a parabolic refinement) around the coarse
/// winner.
fn refine(points: &[Pt], c: Coarse, max_deg: f64) -> Search {
    let half = half_extent(points);
    let sc = |d: f64| score(points, half, d);
    let (mut angle, mut best) = (c.angle, c.best);
    for (half_range, step) in [(1.5f64, 0.2f64), (0.3, 0.05)] {
        let centre = angle;
        let steps = (half_range / step).round() as i32;
        let fine: Vec<(f64, f64)> = (-steps..=steps).map(|i| (centre + f64::from(i) * step, sc(centre + f64::from(i) * step))).collect();
        let (idx, &(a, s)) = fine.iter().enumerate().fold((0, &(centre, f64::MIN)), |m, (i, c)| if c.1 > m.1.1 { (i, c) } else { m });
        angle = a;
        best = s;
        if step < 0.1
            && idx > 0
            && let (Some(l), Some(r)) = (fine.get(idx - 1), fine.get(idx + 1))
        {
            let denom = l.1 - 2.0 * s + r.1;
            if denom < 0.0 {
                angle += (0.5 * (l.1 - r.1) / denom * step).clamp(-step, step);
            }
        }
    }
    Search { angle: angle.clamp(-max_deg, max_deg), best, median: c.median, at_zero: c.at_zero }
}

/// Decide the report for an analysis; also returns the ink box after the chosen turn.
fn decide(a: &Analysis, o: &DeskewOptions) -> (DeskewReport, Option<[f32; 4]>) {
    let max_deg = o.max_angle_deg.clamp(1.0, 45.0);
    // Both axes get the coarse sweep (to choose between them); only the winner is refined.
    let upright = coarse(&a.points, max_deg);
    let (mut chosen, mut k_axis) = (&a.points, 0u32);
    let turned_points;
    let mut picked = upright;
    let mut axis_ratio = 0.0;
    if o.detect_orientation {
        turned_points = turn_points(&a.points, 1);
        let sideways = coarse(&turned_points, max_deg);
        axis_ratio = sideways.best / picked.best.max(f64::MIN_POSITIVE);
        if axis_ratio > o.axis_ratio_min {
            chosen = &turned_points;
            k_axis = 1;
            picked = sideways;
        }
    }
    let found = refine(chosen, picked, max_deg);
    let prominence = if found.best + found.median > 0.0 { ((found.best - found.median) / (found.best + found.median)).clamp(0.0, 1.0) } else { 0.0 };
    // Orientation within the axis: bottom-heaviness of the sheared line bands (weak for Thai).
    let (mut turns, mut orient_conf) = (k_axis, 0.0);
    if o.detect_orientation {
        let (flip, conf) = flip_margin(chosen, found.angle);
        orient_conf = conf;
        if flip {
            turns = (k_axis + 2) % 4;
        }
        if k_axis == 1 {
            orient_conf = orient_conf.min(prominence);
        }
    }
    // A turn is only trusted when the axis evidence was decisive; the 0/180 flip needs margin.
    let angle =
        if prominence >= o.min_confidence && (found.angle.abs() >= o.deadband_deg) && found.best > 1.03 * found.at_zero { found.angle } else { 0.0 };
    let applied = if turns != 0 && turns % 2 == 0 && orient_conf < 0.08 { 0.0 } else { angle };
    let report = DeskewReport {
        detected_deg: if prominence >= o.min_confidence { found.angle } else { 0.0 },
        applied_deg: applied,
        confidence: prominence,
        orientation_turns: if prominence >= o.min_confidence { turns } else { 0 },
        orientation_confidence: orient_conf,
        axis_ratio,
    };
    let report = if report.orientation_turns == 2 && orient_conf < 0.08 { DeskewReport { orientation_turns: 0, ..report } } else { report };
    (report, turn_bbox(a.ink_bbox, report.orientation_turns))
}

/// (flip, certainty): does the sheared, line-banded ink sit mostly in the lower half of its
/// bands (upright, Latin-style) or the upper half (upside down)?
fn flip_margin(points: &[Pt], deg: f64) -> (bool, f64) {
    let half = points.iter().fold(0f32, |m, p| m.max(p.0.abs()).max(p.1.abs())) * std::f32::consts::SQRT_2 + 2.0;
    let (s, c) = (deg.to_radians().sin() as f32, deg.to_radians().cos() as f32);
    let bins = (2.0 * half).ceil() as usize + 4;
    let mut hist = vec![0u64; bins];
    for &(x, y) in points {
        let i = (y * c - x * s + half + 1.0).round();
        if i >= 0.0
            && let Some(b) = hist.get_mut(i as usize)
        {
            *b += 1;
        }
    }
    let total: u64 = hist.iter().sum();
    if total == 0 {
        return (false, 0.0);
    }
    let asym = band_bottom_heaviness(&hist) / total as f64;
    (asym < 0.0, asym.abs().min(1.0))
}

// ---------------------------------------------------------------------------------------------
// Warp
// ---------------------------------------------------------------------------------------------

/// The content scale (≤ 1) that keeps the rotated ink box inside the canvas; 1 when it fits.
fn fit_scale(w: u32, h: u32, deg: f64, bbox: Option<[f32; 4]>, analysis_scale: f64) -> f64 {
    let Some(b) = bbox else { return 1.0 };
    let (sin, cos) = deg.to_radians().sin_cos();
    let (hw, hh) = (f64::from(w) / 2.0 - 1.0, f64::from(h) / 2.0 - 1.0);
    let mut s: f64 = 1.0;
    for (x, y) in [(b[0], b[1]), (b[2], b[1]), (b[0], b[3]), (b[2], b[3])] {
        let (x, y) = (f64::from(x) / analysis_scale, f64::from(y) / analysis_scale);
        // The correction rotates the content by −deg about the centre.
        let (rx, ry) = (cos * x + sin * y, -sin * x + cos * y);
        if rx.abs() > 1e-9 {
            s = s.min(hw / rx.abs());
        }
        if ry.abs() > 1e-9 {
            s = s.min(hh / ry.abs());
        }
    }
    s.clamp(0.5, 1.0)
}

/// Rotate the content by −`deg` about the centre and scale it by `fit`; the exact dest→src map
/// is the returned transform. Pixel-edge coordinates throughout (pixel `i` spans `i..i+1`).
fn rotate_warp(image: &OcrImage, deg: f64, fit: f64, paper: [u8; 3], resample: Resample) -> (OcrImage, InverseTransform) {
    let phi = deg.to_radians();
    let (sin, cos) = phi.sin_cos();
    let (w, h) = (image.width() as usize, image.height() as usize);
    let (cx, cy) = (w as f64 / 2.0, h as f64 / 2.0);
    let k = 1.0 / fit;
    let (a, b, c, d) = (cos * k, sin * k, -sin * k, cos * k);
    let back = InverseTransform::from_parts(a, b, c, d, cx - a * cx - c * cy, cy - b * cx - d * cy);
    let src = image.rgba();
    let fetch = |x: i64, y: i64, ch: usize| -> f64 {
        if x < 0 || y < 0 || x >= w as i64 || y >= h as i64 {
            return f64::from(paper.get(ch).copied().unwrap_or(255));
        }
        f64::from(src.get((y as usize * w + x as usize) * 4 + ch).copied().unwrap_or(255))
    };
    let mut out = vec![255u8; w * h * 4];
    for (y, row) in out.chunks_exact_mut(w * 4).enumerate() {
        for (x, px) in row.as_chunks_mut::<4>().0.iter_mut().enumerate() {
            let (sx, sy) = back.apply(x as f64 + 0.5, y as f64 + 0.5);
            let (fx, fy) = (sx - 0.5, sy - 0.5);
            let (x0, y0) = (fx.floor(), fy.floor());
            let (tx, ty) = (fx - x0, fy - y0);
            let (xi, yi) = (x0 as i64, y0 as i64);
            // Fast path: a bilinear sample with all four neighbours inside the raster.
            if resample == Resample::Bilinear && xi >= 0 && yi >= 0 && (xi as usize) + 1 < w && (yi as usize) + 1 < h {
                let o = (yi as usize * w + xi as usize) * 4;
                if let (Some(r0), Some(r1)) = (src.get(o..o + 8), src.get(o + w * 4..o + w * 4 + 8))
                    && let (Ok(r0), Ok(r1)) = (<&[u8; 8]>::try_from(r0), <&[u8; 8]>::try_from(r1))
                {
                    let (w00, w10, w01, w11) = ((1.0 - tx) * (1.0 - ty), tx * (1.0 - ty), (1.0 - tx) * ty, tx * ty);
                    for ch in 0..3 {
                        let v = f64::from(r0[ch]) * w00 + f64::from(r0[4 + ch]) * w10 + f64::from(r1[ch]) * w01 + f64::from(r1[4 + ch]) * w11;
                        px[ch] = (v + 0.5) as u8;
                    }
                    px[3] = 255;
                    continue;
                }
            }
            for (ch, o) in px.iter_mut().enumerate() {
                let v = match resample {
                    Resample::Bilinear => {
                        let top = fetch(xi, yi, ch) * (1.0 - tx) + fetch(xi + 1, yi, ch) * tx;
                        let bot = fetch(xi, yi + 1, ch) * (1.0 - tx) + fetch(xi + 1, yi + 1, ch) * tx;
                        top * (1.0 - ty) + bot * ty
                    }
                    Resample::CatmullRom => {
                        let (wx, wy) = (cubic_weights(tx), cubic_weights(ty));
                        let mut acc = 0.0;
                        for (j, wyj) in wy.iter().enumerate() {
                            let mut line = 0.0;
                            for (i, wxi) in wx.iter().enumerate() {
                                line += wxi * fetch(xi - 1 + i as i64, yi - 1 + j as i64, ch);
                            }
                            acc += wyj * line;
                        }
                        acc
                    }
                };
                *o = v.round().clamp(0.0, 255.0) as u8;
            }
            px[3] = 255;
        }
    }
    (OcrImage::new(image.width(), image.height(), out).unwrap_or_else(|_| image.clone()), back)
}

fn cubic_weights(t: f64) -> [f64; 4] {
    let (t2, t3) = (t * t, t * t * t);
    [(-t3 + 2.0 * t2 - t) / 2.0, (3.0 * t3 - 5.0 * t2 + 2.0) / 2.0, (-3.0 * t3 + 4.0 * t2 + t) / 2.0, (t3 - t2) / 2.0]
}

#[cfg(test)]
mod tests;
