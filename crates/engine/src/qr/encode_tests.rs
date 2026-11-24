//! End to end: a page with QR codes drawn as vector rectangles (`re f`) is rendered and scanned,
//! and each code comes back with its payload and the user-space box it was drawn in.
//!
//! No QR encoder is a dependency, so a minimal one is here: version 1 (21 × 21), error
//! correction L, byte mode, mask 0, up to 17 bytes. `encoder_round_trips_through_rqrr` checks it
//! against the decoder itself before any page is involved.

use super::*;

const SIZE: usize = 21;
/// Version 1-L: data codewords, error-correction codewords, byte-mode capacity.
const DATA_CODEWORDS: usize = 19;
const EC_CODEWORDS: usize = 7;
const MAX_BYTES: usize = 17;

/// Multiply in GF(2^8) modulo x^8 + x^4 + x^3 + x^2 + 1 (0x11D).
fn gf_mul(x: u8, y: u8) -> u8 {
    let mut z: u16 = 0;
    for i in (0..8).rev() {
        z = (z << 1) ^ ((z >> 7) * 0x11D);
        z ^= u16::from((y >> i) & 1) * u16::from(x);
    }
    z as u8
}

/// The Reed-Solomon generator polynomial of `degree`, highest coefficient first (leading 1 dropped).
fn rs_divisor(degree: usize) -> Vec<u8> {
    let mut result = vec![0u8; degree];
    result[degree - 1] = 1;
    let mut root = 1u8;
    for _ in 0..degree {
        for j in 0..degree {
            result[j] = gf_mul(result[j], root);
            if j + 1 < degree {
                result[j] ^= result[j + 1];
            }
        }
        root = gf_mul(root, 2);
    }
    result
}

fn rs_remainder(data: &[u8], divisor: &[u8]) -> Vec<u8> {
    let mut result = vec![0u8; divisor.len()];
    for &b in data {
        let factor = b ^ result.remove(0);
        result.push(0);
        for (r, &d) in result.iter_mut().zip(divisor) {
            *r ^= gf_mul(d, factor);
        }
    }
    result
}

fn push_bits(bits: &mut Vec<bool>, value: u32, count: usize) {
    for i in (0..count).rev() {
        bits.push((value >> i) & 1 == 1);
    }
}

struct Grid {
    dark: [[bool; SIZE]; SIZE],
    function: [[bool; SIZE]; SIZE],
}

impl Grid {
    /// Set a function module at column `x`, row `y`.
    fn set(&mut self, x: usize, y: usize, dark: bool) {
        self.dark[y][x] = dark;
        self.function[y][x] = true;
    }
}

/// The modules of a version-1-L QR code carrying `payload`, `[row][column]`, `true` for dark.
fn encode(payload: &[u8]) -> [[bool; SIZE]; SIZE] {
    assert!(payload.len() <= MAX_BYTES, "version 1-L holds {MAX_BYTES} bytes");
    let mut bits = Vec::new();
    push_bits(&mut bits, 0b0100, 4);
    push_bits(&mut bits, payload.len() as u32, 8);
    for &b in payload {
        push_bits(&mut bits, u32::from(b), 8);
    }
    let terminator = (DATA_CODEWORDS * 8 - bits.len()).min(4);
    push_bits(&mut bits, 0, terminator);
    while bits.len() % 8 != 0 {
        bits.push(false);
    }
    let mut data: Vec<u8> = bits.chunks(8).map(|c| c.iter().fold(0u8, |a, &b| (a << 1) | u8::from(b))).collect();
    for pad in [0xEC, 0x11].into_iter().cycle() {
        if data.len() >= DATA_CODEWORDS {
            break;
        }
        data.push(pad);
    }
    let ecc = rs_remainder(&data, &rs_divisor(EC_CODEWORDS));
    data.extend(ecc);

    let mut g = Grid { dark: [[false; SIZE]; SIZE], function: [[false; SIZE]; SIZE] };
    for i in 0..SIZE {
        g.set(6, i, i % 2 == 0);
        g.set(i, 6, i % 2 == 0);
    }
    for (cx, cy) in [(3i32, 3i32), (SIZE as i32 - 4, 3), (3, SIZE as i32 - 4)] {
        for dy in -4i32..=4 {
            for dx in -4i32..=4 {
                let (x, y) = (cx + dx, cy + dy);
                if (0..SIZE as i32).contains(&x) && (0..SIZE as i32).contains(&y) {
                    let dist = dx.abs().max(dy.abs());
                    g.set(x as usize, y as usize, dist != 2 && dist != 4);
                }
            }
        }
    }
    // Format: error correction L (01) and mask 0, BCH-coded and masked.
    let format: u32 = 0b01 << 3;
    let mut rem = format;
    for _ in 0..10 {
        rem = (rem << 1) ^ ((rem >> 9) * 0x537);
    }
    let format = ((format << 10) | rem) ^ 0x5412;
    let bit = |i: usize| (format >> i) & 1 == 1;
    for i in 0..=5 {
        g.set(8, i, bit(i));
    }
    g.set(8, 7, bit(6));
    g.set(8, 8, bit(7));
    g.set(7, 8, bit(8));
    for i in 9..15 {
        g.set(14 - i, 8, bit(i));
    }
    for i in 0..8 {
        g.set(SIZE - 1 - i, 8, bit(i));
    }
    for i in 8..15 {
        g.set(8, SIZE - 15 + i, bit(i));
    }
    g.set(8, SIZE - 8, true);

    // Codewords in the zigzag, two columns at a time from the right, skipping the timing column.
    let mut i = 0usize;
    let mut right = SIZE as i32 - 1;
    while right >= 1 {
        if right == 6 {
            right = 5;
        }
        for vert in 0..SIZE {
            for j in 0..2 {
                let x = (right - j) as usize;
                let upward = ((right + 1) & 2) == 0;
                let y = if upward { SIZE - 1 - vert } else { vert };
                if !g.function[y][x] && i < data.len() * 8 {
                    g.dark[y][x] = (data[i >> 3] >> (7 - (i & 7))) & 1 == 1;
                    i += 1;
                }
            }
        }
        right -= 2;
    }
    for y in 0..SIZE {
        for x in 0..SIZE {
            if !g.function[y][x] && (x + y) % 2 == 0 {
                g.dark[y][x] = !g.dark[y][x];
            }
        }
    }
    g.dark
}

/// Content-stream operators drawing `payload`'s QR code with `module` points per module, its
/// top-left corner at (`left`, `top`) in user space (y up).
fn qr_ops(payload: &str, left: f64, top: f64, module: f64) -> String {
    let mut ops = String::from("0 g\n");
    for (row, cols) in encode(payload.as_bytes()).iter().enumerate() {
        for (col, &dark) in cols.iter().enumerate() {
            if dark {
                let (x, y) = (left + col as f64 * module, top - (row as f64 + 1.0) * module);
                ops.push_str(&format!("{x} {y} {module} {module} re\n"));
            }
        }
    }
    ops.push_str("f\n");
    ops
}

/// A one-page PDF of `width` × `height` points, turned `rotate` degrees, drawing `content`.
fn page_pdf(width: f64, height: f64, rotate: u16, content: &str) -> std::sync::Arc<Vec<u8>> {
    let bytes = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!("<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Rotate {rotate} /Contents 4 0 R >>"))
        .stream("", content)
        .build();
    std::sync::Arc::new(bytes)
}

const MODULE: f64 = 4.0;
/// A symbol's side in points.
const SIDE: f64 = SIZE as f64 * MODULE;

/// `got` is the drawn box `[left, top - SIDE, left + SIDE, top]` within a module and a half on
/// every edge. (The decoder's corners run about one module past the symbol on its far sides, so
/// a found box is about a module larger than the drawn one there.)
fn assert_box(got: [f64; 4], left: f64, top: f64) {
    let want = [left, top - SIDE, left + SIDE, top];
    assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= 1.5 * MODULE), "got {got:?}, drawn {want:?}");
}

#[test]
fn encoder_round_trips_through_rqrr() {
    for payload in ["QR-ONE", "https://qr.test/2", "a"] {
        let scale = 8;
        let quiet = 4;
        let side = (SIZE + 2 * quiet) * scale;
        let mut rgba = vec![255u8; side * side * 4];
        for (row, cols) in encode(payload.as_bytes()).iter().enumerate() {
            for (col, &dark) in cols.iter().enumerate() {
                if !dark {
                    continue;
                }
                for py in 0..scale {
                    for px in 0..scale {
                        let (x, y) = ((col + quiet) * scale + px, (row + quiet) * scale + py);
                        let o = (y * side + x) * 4;
                        rgba[o..o + 3].fill(0);
                    }
                }
            }
        }
        let found = scan_qr_from_rgba(side, side, &rgba);
        assert_eq!(found.iter().map(|q| q.content.as_str()).collect::<Vec<_>>(), vec![payload]);
    }
}

#[test]
fn vector_qr_on_a_page_is_found_where_it_is_drawn() {
    let (left, top) = (100.0, 700.0);
    let bytes = page_pdf(612.0, 792.0, 0, &qr_ops("https://qr.test/2", left, top, MODULE));
    let info = pdfcraft_render::PageInfo { width: 612.0, height: 792.0, label: String::new(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 };
    let regions = scan_page_qr_regions(bytes, pdfcraft_render::RenderConfig::default(), 0, &info).expect("page renders");
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert_eq!(regions[0].content, "https://qr.test/2");
    assert_box(regions[0].rect_user, left, top);
}

#[test]
fn two_qr_codes_on_a_page_each_keep_their_own_box() {
    let codes = [("QR-ONE", 60.0, 740.0), ("https://qr.test/2", 330.0, 360.0)];
    let content: String = codes.iter().map(|(p, l, t)| qr_ops(p, *l, *t, MODULE)).collect();
    let bytes = page_pdf(612.0, 792.0, 0, &content);
    let info = pdfcraft_render::PageInfo { width: 612.0, height: 792.0, label: String::new(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 };
    let regions = scan_page_qr_regions(bytes, pdfcraft_render::RenderConfig::default(), 0, &info).expect("page renders");
    assert_eq!(regions.len(), 2, "{regions:?}");
    for (payload, left, top) in codes {
        let region = regions.iter().find(|r| r.content == payload).unwrap_or_else(|| panic!("{payload} not found in {regions:?}"));
        assert_box(region.rect_user, left, top);
    }
    let (a, b) = (regions[0].rect_user, regions[1].rect_user);
    assert!(a[2] < b[0] || b[2] < a[0] || a[3] < b[1] || b[3] < a[1], "boxes overlap: {a:?} {b:?}");
}

#[test]
fn vector_qr_on_a_turned_page_is_found_in_user_space() {
    // A portrait 400 × 600 page with /Rotate 90 shows 600 wide and 400 high.
    let (left, top) = (100.0, 500.0);
    let bytes = page_pdf(400.0, 600.0, 90, &qr_ops("QR-ONE", left, top, MODULE));
    let info = pdfcraft_render::PageInfo { width: 600.0, height: 400.0, label: String::new(), crop: [0.0, 0.0, 400.0, 600.0], rotation: 90 };
    let regions = scan_page_qr_regions(bytes, pdfcraft_render::RenderConfig::default(), 0, &info).expect("page renders");
    assert_eq!(regions.len(), 1, "{regions:?}");
    assert_eq!(regions[0].content, "QR-ONE");
    assert_box(regions[0].rect_user, left, top);
}

/// A deterministic pseudo-random sequence in 0..1 (a linear congruential generator).
fn lcg(seed: &mut u64) -> f64 {
    *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
    (*seed >> 11) as f64 / (1u64 << 53) as f64
}

/// A greyscale raster of `payload`'s QR code as a scan prints it: `px` pixels a module, a
/// 4-module quiet zone, ink at grey 30 and paper at 225 (not black and white), and every pixel
/// moved by up to ±`jitter` levels.
fn grey_raster(payload: &str, px: usize, jitter: f64, seed: u64) -> (usize, Vec<u8>) {
    let side = (SIZE + 8) * px;
    let modules = encode(payload.as_bytes());
    let mut seed = seed;
    let mut grey = Vec::with_capacity(side * side);
    for y in 0..side {
        for x in 0..side {
            let (row, col) = ((y / px).wrapping_sub(4), (x / px).wrapping_sub(4));
            let dark = modules.get(row).and_then(|r| r.get(col)).copied().unwrap_or(false);
            let base = if dark { 30.0 } else { 225.0 };
            grey.push((base + (lcg(&mut seed) * 2.0 - 1.0) * jitter).clamp(0.0, 255.0) as u8);
        }
    }
    (side, grey)
}

/// A one-page PDF of `width` × `height` points, turned `rotate` degrees, showing a square
/// greyscale image (`side` pixels wide, its pixels) at 150 DPI with its top-left corner at (`left`, `top`),
/// then drawing `extra`.
fn image_page_pdf(
    width: f64,
    height: f64,
    rotate: u16,
    (side, grey): (usize, &[u8]),
    (left, top): (f64, f64),
    extra: &str,
) -> std::sync::Arc<Vec<u8>> {
    let pt = side as f64 * 72.0 / 150.0;
    let content = format!("q {pt} 0 0 {pt} {left} {} cm /Im0 Do Q\n{extra}", top - pt);
    let bytes = pdfcraft_testkit::Pdf::new()
        .obj("<< /Type /Catalog /Pages 2 0 R >>")
        .obj("<< /Type /Pages /Kids [3 0 R] /Count 1 >>")
        .obj(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {width} {height}] /Rotate {rotate} /Resources << /XObject << /Im0 5 0 R >> >> /Contents 4 0 R >>"
        ))
        .stream("", content)
        .stream(&format!("/Type /XObject /Subtype /Image /Width {side} /Height {side} /ColorSpace /DeviceGray /BitsPerComponent 8"), grey)
        .build();
    std::sync::Arc::new(bytes)
}

fn letter() -> pdfcraft_render::PageInfo {
    pdfcraft_render::PageInfo { width: 612.0, height: 792.0, label: String::new(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 0 }
}

/// The box a raster from [`grey_raster`] with `px`-pixel modules shows its symbol in (without the
/// quiet zone), drawn at 150 DPI from (`left`, `top`), and the tolerance a found box gets.
fn raster_symbol(px: usize, left: f64, top: f64) -> ([f64; 4], f64) {
    let module = px as f64 * 72.0 / 150.0;
    let (sx, sy) = (left + 4.0 * module, top - 4.0 * module);
    ([sx, sy - SIZE as f64 * module, sx + SIZE as f64 * module, sy], (1.5 * module).max(2.0))
}

fn assert_raster_box(got: [f64; 4], (want, tolerance): ([f64; 4], f64)) {
    assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() <= tolerance), "got {got:?}, drawn {want:?}");
}

/// The first pass alone (the scan before the second pass existed) on page 0 of `bytes`.
fn first_pass(bytes: &std::sync::Arc<Vec<u8>>) -> Vec<QrCodeInfo> {
    let mut renderer = PageRenderer::new(bytes.clone(), pdfcraft_render::RenderConfig::default());
    let r = renderer.render(RenderRequest { page: 0, scale: QR_SCAN_SCALE, ..Default::default() });
    scan_qr_from_rgba(r.width as usize, r.height as usize, &r.rgba)
}

/// A grey 30/225 raster QR (6 pixels a module, ±40 jitter) that the plain 150 DPI pass misses is
/// found by the second pass, and mapped back from its larger render to where it was drawn. This
/// shows the second pass runs, decodes and maps; it does not show noise is what defeats the
/// first pass, which misses this raster even without the jitter.
#[test]
fn grey_raster_the_plain_pass_misses_is_found() {
    let (left, top) = (100.0, 700.0);
    let (side, grey) = grey_raster("QR-ONE", 6, 40.0, 1);
    let bytes = image_page_pdf(612.0, 792.0, 0, (side, &grey), (left, top), "");
    assert!(first_pass(&bytes).is_empty(), "the plain pass finds this one: the test proves nothing");
    let regions = scan_page_qr_regions(bytes.clone(), pdfcraft_render::RenderConfig::default(), 0, &letter()).expect("page renders");
    assert_eq!(regions.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(), vec!["QR-ONE"]);
    assert_raster_box(regions[0].rect_user, raster_symbol(6, left, top));
    // The Scan QR Code dialog's scan runs the same passes.
    let found = scan_page_qr(bytes, pdfcraft_render::RenderConfig::default(), 0).expect("page renders");
    assert_eq!(found.iter().map(|q| q.content.as_str()).collect::<Vec<_>>(), vec!["QR-ONE"]);
}

/// The same raster on a page turned a quarter: the second pass's code lands where it was drawn
/// in unrotated user space.
#[test]
fn grey_raster_on_a_turned_page_is_found_in_user_space() {
    let (left, top) = (100.0, 700.0);
    let (side, grey) = grey_raster("QR-ONE", 6, 40.0, 1);
    let bytes = image_page_pdf(612.0, 792.0, 90, (side, &grey), (left, top), "");
    let info = pdfcraft_render::PageInfo { width: 792.0, height: 612.0, label: String::new(), crop: [0.0, 0.0, 612.0, 792.0], rotation: 90 };
    let regions = scan_page_qr_regions(bytes, pdfcraft_render::RenderConfig::default(), 0, &info).expect("page renders");
    assert_eq!(regions.iter().map(|r| r.content.as_str()).collect::<Vec<_>>(), vec!["QR-ONE"]);
    assert_raster_box(regions[0].rect_user, raster_symbol(6, left, top));
}

/// A code both passes find is reported once, where the first pass found it.
#[test]
fn a_code_both_passes_find_is_reported_once() {
    // A vector QR, and a raster elsewhere so the page draws an image and the second pass runs.
    let (left, top) = (300.0, 500.0);
    let (side, grey) = grey_raster("other", 2, 0.0, 1);
    let bytes = image_page_pdf(612.0, 792.0, 0, (side, &grey), (40.0, 780.0), &qr_ops("QR-ONE", left, top, MODULE));
    let first = first_pass(&bytes);
    assert_eq!(first.iter().map(|q| q.content.as_str()).collect::<Vec<_>>(), vec!["QR-ONE"]);
    // The second pass on its own finds it too.
    let scale = fine_scale(612.0, 792.0).expect("Letter gets a second pass");
    let mut renderer = PageRenderer::new(bytes.clone(), pdfcraft_render::RenderConfig::default());
    let fine = renderer.render(RenderRequest { page: 0, scale, ..Default::default() });
    let grey_page = grey_of(fine.width as usize, fine.height as usize, &fine.rgba);
    let second = scan_qr_binarized(fine.width as usize, fine.height as usize, grey_page);
    assert!(second.iter().any(|q| q.content == "QR-ONE"), "{second:?}");
    let regions = scan_page_qr_regions(bytes, pdfcraft_render::RenderConfig::default(), 0, &letter()).expect("page renders");
    assert_eq!(regions.iter().filter(|r| r.content == "QR-ONE").count(), 1, "{regions:?}");
    assert_box(regions.iter().find(|r| r.content == "QR-ONE").map(|r| r.rect_user).unwrap_or_default(), left, top);
}

#[test]
fn second_pass_runs_only_on_pages_that_draw_images() {
    let vector = page_pdf(612.0, 792.0, 0, &qr_ops("QR-ONE", 100.0, 700.0, MODULE));
    assert!(!page_may_draw_images(vector, None, 0));
    let (side, grey) = grey_raster("QR-ONE", 2, 0.0, 1);
    assert!(page_may_draw_images(image_page_pdf(612.0, 792.0, 0, (side, &grey), (100.0, 700.0), ""), None, 0));
    let inline = page_pdf(612.0, 792.0, 0, "q 10 0 0 10 50 50 cm BI /W 1 /H 1 /CS /G /BPC 8 ID \u{0} EI Q\n");
    assert!(page_may_draw_images(inline, None, 0));
    // What can't be read is scanned rather than skipped.
    assert!(page_may_draw_images(std::sync::Arc::new(b"not a pdf".to_vec()), None, 0));
    let vector = page_pdf(612.0, 792.0, 0, "0 0 10 10 re f\n");
    assert!(page_may_draw_images(vector, None, 3));
}

#[test]
fn second_pass_scale_is_capped_and_total() {
    // A4 and Letter scan at 300 DPI.
    assert_eq!(fine_scale(612.0, 792.0), Some((300.0f64 / 72.0) as f32));
    assert_eq!(fine_scale(595.0, 842.0), Some((300.0f64 / 72.0) as f32));
    // A poster is scanned at fewer than 300 DPI, within the pixel cap.
    let s = fine_scale(1200.0, 1600.0).expect("finer than 150 DPI");
    assert!(s < 300.0 / 72.0 && f64::from(s) * f64::from(s) * 1.92e6 <= QR_FINE_MAX_PIXELS * 1.0001, "{s}");
    // So big that the cap is at or below 150 DPI: no second pass. Nonsense sizes: none.
    assert_eq!(fine_scale(14400.0, 14400.0), None);
    for (w, h) in [(0.0, 792.0), (f32::NAN, 792.0), (f32::INFINITY, 1.0), (-612.0, 792.0)] {
        assert!(fine_scale(w, h).is_none_or(|s| s.is_finite() && s > 0.0), "{w} x {h}");
    }
    assert_eq!(fine_scale(0.0, 792.0), None);
    assert_eq!(fine_scale(f32::NAN, 792.0), None);
}

/// The 3 × 3 average of every pixel, window clipped at the edges, the plain way.
fn naive_blur(width: usize, height: usize, grey: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    for y in 0..height {
        for x in 0..width {
            let (mut sum, mut n) = (0u32, 0u32);
            for yy in y.saturating_sub(1)..(y + 2).min(height) {
                for xx in x.saturating_sub(1)..(x + 2).min(width) {
                    sum += u32::from(grey[yy * width + xx]);
                    n += 1;
                }
            }
            out.push((sum / n) as u8);
        }
    }
    out
}

#[test]
fn binarize_helpers_take_any_buffer() {
    assert!(box_blur(0, &[1, 2, 3]).is_empty());
    assert_eq!(box_blur(3, &[0, 90, 0]), vec![45, 30, 45]);
    let mut seed = 7;
    for (w, h) in [(1, 1), (1, 5), (5, 1), (2, 2), (7, 5), (16, 9)] {
        let grey: Vec<u8> = (0..w * h).map(|_| (lcg(&mut seed) * 256.0) as u8).collect();
        assert_eq!(box_blur(w, &grey), naive_blur(w, h, &grey), "{w} x {h}");
    }
    // A trailing partial row is left out, never read past.
    assert_eq!(box_blur(3, &[3, 3, 3, 9]).len(), 3);
    assert_eq!(otsu_threshold(&[]), 128);
    assert_eq!(otsu_threshold(&[7; 10]), 128);
    let t = otsu_threshold(&[10, 12, 11, 240, 245, 250]);
    assert!((12..240).contains(&t), "{t}");
    assert!(scan_qr_binarized(5, 5, vec![0; 7]).is_empty());
    assert!(scan_qr_binarized(usize::MAX, 2, Vec::new()).is_empty());
    assert!(scan_qr_from_rgba(usize::MAX, usize::MAX, &[0; 16]).is_empty());
    let q = QrCodeInfo { content: "x".into(), bounds: [10, 20, 30, 40], corners: [[10, 20], [30, 20], [30, 40], [10, 40]] };
    assert_eq!(rescaled(q.clone(), [200, 400], [100, 200]).bounds, [5, 10, 15, 20]);
    assert_eq!(rescaled(q, [0, 0], [100, 200]).bounds, [100, 200, 100, 200]);
}
