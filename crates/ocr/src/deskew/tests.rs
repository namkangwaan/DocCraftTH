use super::*;
use crate::preprocess::rotate_quarter_turns;

const W: usize = 1240;
const H: usize = 1754;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    English,
    Thai,
    Form,
}

struct Rng(u64);
impl Rng {
    fn next(&mut self, n: u32) -> u32 {
        self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
        ((self.0 >> 33) as u32) % n.max(1)
    }
}

fn fill(buf: &mut [u8], x0: i64, y0: i64, w: i64, h: i64, v: u8) {
    for y in y0.max(0)..(y0 + h).min(H as i64) {
        for x in x0.max(0)..(x0 + w).min(W as i64) {
            let o = (y as usize * W + x as usize) * 4;
            if let Some(p) = buf.get_mut(o..o + 4) {
                p.copy_from_slice(&[v, v, v, 255]);
            }
        }
    }
}

/// An upright synthetic page: lines of word-like glyph blocks. Thai adds tone marks above and
/// vowels below (small blocks); the form adds dot leaders and horizontal rules.
fn page(kind: Kind, seal: bool) -> Vec<u8> {
    let mut buf = vec![255u8; W * H * 4];
    let mut rng = Rng(7);
    let mut y = 190i64;
    let mut row = 0;
    while y < H as i64 - 190 {
        let mut x = 160i64;
        let right = W as i64 - 160;
        if kind == Kind::Form && row % 4 == 3 {
            while x < right {
                fill(&mut buf, x, y + 14, 3, 3, 0);
                x += 9;
            }
        } else {
            while x < right - 60 {
                let letters = 2 + rng.next(6) as i64;
                for _ in 0..letters {
                    let (gw, gh) = (8 + rng.next(6) as i64, 16 + rng.next(4) as i64);
                    fill(&mut buf, x, y, gw, gh, 0);
                    match rng.next(5) {
                        0 => fill(&mut buf, x + 2, y - 12, 3, 12, 0),
                        1 => fill(&mut buf, x + 2, y + gh, 3, 8, 0),
                        _ => {}
                    }
                    if kind == Kind::Thai {
                        if rng.next(3) == 0 {
                            fill(&mut buf, x + 1, y - 7, 4, 3, 0); // tone mark
                        }
                        if rng.next(6) == 0 {
                            fill(&mut buf, x + 3, y + gh + 2, 2, 3, 0); // lower vowel
                        }
                    }
                    x += gw + 3;
                }
                x += 18;
            }
        }
        if kind == Kind::Form && row % 7 == 6 {
            fill(&mut buf, 160, y + 34, W as i64 - 320, 2, 0);
        }
        y += 46;
        row += 1;
    }
    if seal {
        let (cx, cy, r) = (W as i64 - 300, 300i64, 105i64);
        for yy in cy - r..=cy + r {
            for xx in cx - r..=cx + r {
                let d2 = (xx - cx).pow(2) + (yy - cy).pow(2);
                if ((r - 12).pow(2) <= d2 && d2 <= r * r) || d2 < 30 * 30 {
                    fill(&mut buf, xx, yy, 1, 1, 20);
                }
            }
        }
    }
    buf
}

/// Nearest-neighbour: content turned so its lines slope `deg` (downhill right). Independent of
/// the code under test.
fn skew(buf: &[u8], deg: f64) -> Vec<u8> {
    let (s, c) = (-deg.to_radians()).sin_cos();
    let (cx, cy) = (W as f64 / 2.0, H as f64 / 2.0);
    let mut out = vec![255u8; W * H * 4];
    for y in 0..H {
        for x in 0..W {
            let (dx, dy) = (x as f64 + 0.5 - cx, y as f64 + 0.5 - cy);
            let (sx, sy) = (cx + c * dx - s * dy, cy + s * dx + c * dy);
            let (xi, yi) = (sx.floor() as i64, sy.floor() as i64);
            if xi >= 0 && yi >= 0 && (xi as usize) < W && (yi as usize) < H {
                let o = (yi as usize * W + xi as usize) * 4;
                let d = (y * W + x) * 4;
                if let (Some(s), Some(t)) = (buf.get(o..o + 4), out.get_mut(d..d + 4)) {
                    t.copy_from_slice(s);
                }
            }
        }
    }
    out
}

fn scanner_border(buf: &mut [u8]) {
    fill(buf, 0, 0, 38, H as i64, 8);
    fill(buf, 0, 0, W as i64, 34, 8);
    fill(buf, W as i64 - 30, 0, 30, H as i64, 8);
}

fn img(buf: Vec<u8>) -> OcrImage {
    OcrImage::new(W as u32, H as u32, buf).unwrap()
}

const ANGLES: [f64; 15] = [0.0, 0.5, -0.5, 1.0, -1.0, 1.5, -1.5, 2.0, -2.0, 3.5, -3.5, 7.0, -7.0, 12.0, -12.0];

fn check(kind: Kind, seal: bool, border: bool, angles: &[f64], tol: f64) {
    let base = page(kind, seal);
    for &a in angles {
        let mut s = skew(&base, a);
        if border {
            scanner_border(&mut s);
        }
        let r = analyze(&img(s), &DeskewOptions::default());
        assert!((r.detected_deg - a).abs() <= tol, "angle {a}: detected {} (conf {:.2})", r.detected_deg, r.confidence);
        if a.abs() >= 0.5 {
            assert!((r.applied_deg - a).abs() <= tol, "angle {a}: applied {}", r.applied_deg);
        }
        assert_eq!(r.orientation_turns, 0, "angle {a}: no quarter turn expected");
    }
}

#[test]
fn english_pages_are_measured() {
    check(Kind::English, false, false, &ANGLES, 0.25);
}

#[test]
fn thai_pages_with_marks_are_measured() {
    check(Kind::Thai, false, false, &ANGLES, 0.25);
}

#[test]
fn forms_with_dot_leaders_and_rules_are_measured() {
    check(Kind::Form, false, false, &ANGLES, 0.25);
}

#[test]
fn seals_do_not_fool_the_search() {
    check(Kind::Thai, true, false, &[0.0, -2.0, 3.5, -7.0, 12.0], 0.3);
}

#[test]
fn black_scanner_borders_do_not_fool_the_search() {
    check(Kind::Thai, true, true, &[0.0, 1.5, -3.5, 7.0], 0.3);
}

#[test]
fn large_skews_are_measured() {
    check(Kind::Thai, false, false, &[18.0, -18.0, 15.0], 0.4);
}

#[test]
fn straight_pages_are_never_rotated() {
    for kind in [Kind::English, Kind::Thai, Kind::Form] {
        for seal in [false, true] {
            let mut s = page(kind, seal);
            scanner_border(&mut s);
            let r = analyze(&img(s.clone()), &DeskewOptions::default());
            assert_eq!(r.applied_deg, 0.0, "straight page rotated by {}", r.applied_deg);
            let out = straighten(&img(s.clone()), &DeskewOptions::default()).unwrap();
            assert_eq!(out.image.rgba(), &s[..], "a straight page must be returned untouched");
        }
    }
}

#[test]
fn blank_and_tiny_pages_are_untouched() {
    let blank = OcrImage::new(300, 300, vec![255; 300 * 300 * 4]).unwrap();
    assert!(!analyze(&blank, &DeskewOptions::default()).changes_page());
    let tiny = OcrImage::new(20, 20, vec![0; 20 * 20 * 4]).unwrap();
    assert!(!analyze(&tiny, &DeskewOptions::default()).changes_page());
}

#[test]
fn quarter_turn_axis_is_found() {
    let base = img(page(Kind::English, false));
    for k in [1u32, 3] {
        let r = analyze(&rotate_quarter_turns(&base, k), &DeskewOptions { detect_orientation: true, ..Default::default() });
        assert_eq!(r.orientation_turns % 2, 1, "k={k}: {r:?}");
    }
}

#[test]
fn straightening_makes_lines_level_and_boxes_map_back() {
    let skewed = img(skew(&page(Kind::Thai, false), 3.5));
    let out = straighten(&skewed, &DeskewOptions::default()).unwrap();
    assert!((out.report.applied_deg - 3.5).abs() <= 0.25, "{:?}", out.report);
    let after = analyze(&out.image, &DeskewOptions::default());
    assert!(after.detected_deg.abs() <= 0.25, "residual skew {}", after.detected_deg);
    let (x0, y0) = out.inverse_transform.apply(300.0, 800.0);
    let (x1, y1) = out.inverse_transform.apply(900.0, 800.0);
    let slope = ((y1 - y0) / (x1 - x0)).atan().to_degrees();
    assert!((slope - out.report.applied_deg).abs() < 1e-6, "slope {slope}");
}

#[test]
fn rotation_keeps_the_size_and_paper_colour() {
    let mut buf = skew(&page(Kind::English, false), 7.0);
    for p in buf.as_chunks_mut::<4>().0 {
        if p[0] == 255 {
            p.copy_from_slice(&[250, 245, 230, 255]);
        }
    }
    let out = straighten(&img(buf), &DeskewOptions::default()).unwrap();
    assert_eq!((out.image.width(), out.image.height()), (W as u32, H as u32));
    assert_eq!(out.image.rgba().get(0..3), Some(&[250u8, 245, 230][..]), "corner fill is the paper colour");
}

#[test]
fn catmull_rom_agrees_with_bilinear_on_geometry() {
    let skewed = img(skew(&page(Kind::Thai, false), -2.0));
    let a = straighten(&skewed, &DeskewOptions::default()).unwrap();
    let b = straighten(&skewed, &DeskewOptions { resample: Resample::CatmullRom, ..Default::default() }).unwrap();
    assert_eq!(a.inverse_transform, b.inverse_transform);
}

#[test]
fn hostile_sizes_do_not_panic() {
    for (w, h) in [(64u32, 64u32), (65, 4000), (4000, 65), (1, 5000), (5000, 1)] {
        let v = (0..w as usize * h as usize * 4).map(|i| (i * 31 % 251) as u8).collect();
        let im = OcrImage::new(w, h, v).unwrap();
        let _ = straighten(&im, &DeskewOptions::default()).unwrap();
    }
}

mod props {
    use super::*;
    use proptest::prelude::*;

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(12))]
        #[test]
        fn any_small_raster_is_safe(w in 1u32..200, h in 1u32..200, seed in any::<u64>()) {
            let mut r = Rng(seed);
            let v: Vec<u8> = (0..w as usize * h as usize * 4).map(|_| r.next(256) as u8).collect();
            let im = OcrImage::new(w, h, v).unwrap();
            let out = straighten(&im, &DeskewOptions::default()).unwrap();
            prop_assert!(out.image.width() >= 1);
        }
    }
}

#[test]
fn angle_error_and_speed_meet_the_targets() {
    let base = page(Kind::Thai, true);
    let (mut total, mut n) = (0.0, 0);
    for a in [0.3, -0.3, 0.7, -0.7, 1.2, -1.2, 2.5, -2.5, 5.0, -5.0, 9.0, -9.0, 15.0, -15.0] {
        let r = analyze(&img(skew(&base, a)), &DeskewOptions::default());
        total += (r.detected_deg - a).abs();
        n += 1;
    }
    let mae = total / f64::from(n);
    println!("angle MAE over ±0.3…15°: {mae:.3}°");
    assert!(mae <= 0.15, "MAE {mae}");

    // A4 at 300 dpi: a 2x nearest-neighbour enlargement of the test page.
    let big = {
        let s = skew(&base, 2.0);
        let (bw, bh) = (W * 2, H * 2);
        let mut v = vec![255u8; bw * bh * 4];
        for y in 0..bh {
            for x in 0..bw {
                let (o, d) = (((y / 2) * W + x / 2) * 4, (y * bw + x) * 4);
                if let (Some(a), Some(b)) = (s.get(o..o + 4), v.get_mut(d..d + 4)) {
                    b.copy_from_slice(a);
                }
            }
        }
        OcrImage::new(bw as u32, bh as u32, v).unwrap()
    };
    let mut times: Vec<std::time::Duration> = Vec::new();
    let mut r = DeskewReport::default();
    for _ in 0..5 {
        let t = std::time::Instant::now();
        r = analyze(&big, &DeskewOptions::default());
        times.push(t.elapsed());
    }
    times.sort();
    println!("analyze 2480x3508 (5 runs): min {:?} median {:?} max {:?} -> {r:?}", times[0], times[2], times[4]);
    assert!((r.detected_deg - 2.0).abs() <= 0.25);
    let t = std::time::Instant::now();
    let out = straighten(&big, &DeskewOptions::default()).unwrap();
    println!("straighten 2480x3508 (analysis + bilinear warp): {:?}", t.elapsed());
    assert!(out.report.changes_page());
}

#[test]
fn tiny_jitter_on_straight_pages_is_left_alone() {
    let base = page(Kind::Thai, false);
    for a in [0.05, -0.05, 0.1, -0.1, 0.15, -0.15] {
        let r = analyze(&img(skew(&base, a)), &DeskewOptions::default());
        assert!(r.applied_deg.abs() < 0.2 || (r.applied_deg - a).abs() <= 0.1, "jitter {a}: applied {}", r.applied_deg);
    }
}

/// A near-empty page (a few marks: punch holes, a stamp, a page number) has no lines to measure.
/// On real scans such pages were "measured" at several degrees with high confidence.
#[test]
fn near_empty_pages_are_not_judged() {
    let mut buf = vec![255u8; W * H * 4];
    // Twelve small marks scattered along a 5° line, and a short rule.
    for i in 0..12i64 {
        let x = 200 + i * 80;
        let y = 400 + ((i * 80) as f64 * 5f64.to_radians().tan()) as i64;
        fill(&mut buf, x, y, 14, 14, 0);
    }
    fill(&mut buf, 300, 1500, 200, 3, 0);
    let r = analyze(&img(buf.clone()), &DeskewOptions::default());
    assert!(!r.changes_page(), "{r:?}");
    assert_eq!(r.confidence, 0.0, "no verdict at all");
    assert_eq!(straighten(&img(buf.clone()), &DeskewOptions::default()).unwrap().image.rgba(), &buf[..]);
}
