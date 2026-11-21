//! QR Code and 2D barcode detection and decoding for PDF document pages.

use pdfcraft_render::{PageRenderer, RenderRequest};
use rqrr::PreparedImage;

/// A detected and decoded QR code from a page.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QrCodeInfo {
    /// Decoded text or payload.
    pub content: String,
    /// Bounding box in pixel coordinates: [min_x, min_y, max_x, max_y].
    pub bounds: [u32; 4],
    /// Four corner points: [[x0, y0], [x1, y1], [x2, y2], [x3, y3]].
    pub corners: [[u32; 2]; 4],
}

/// Detect and decode all QR codes present in a raw premultiplied RGBA8 pixel buffer.
pub fn scan_qr_from_rgba(width: usize, height: usize, rgba_bytes: &[u8]) -> Vec<QrCodeInfo> {
    if width == 0 || height == 0 || rgba_bytes.len() < width * height * 4 {
        return Vec::new();
    }

    // Convert RGBA to grayscale using standard luminance weights: (77*R + 150*G + 29*B) >> 8
    let mut prepared = PreparedImage::prepare_from_greyscale(width, height, |x, y| {
        let offset = (y * width + x) * 4;
        let r = rgba_bytes[offset] as u32;
        let g = rgba_bytes[offset + 1] as u32;
        let b = rgba_bytes[offset + 2] as u32;
        ((r * 77 + g * 150 + b * 29) >> 8) as u8
    });

    let grids = prepared.detect_grids();
    let mut results = Vec::new();

    for grid in grids {
        if let Ok((_meta, decoded)) = grid.decode() {
            let mut min_x = u32::MAX;
            let mut min_y = u32::MAX;
            let mut max_x = 0u32;
            let mut max_y = 0u32;
            let mut corners = [[0u32; 2]; 4];

            for (i, p) in grid.bounds.iter().enumerate().take(4) {
                let px = (p.x.max(0) as u32).min(width as u32);
                let py = (p.y.max(0) as u32).min(height as u32);
                min_x = min_x.min(px);
                min_y = min_y.min(py);
                max_x = max_x.max(px);
                max_y = max_y.max(py);
                corners[i] = [px, py];
            }

            results.push(QrCodeInfo { content: decoded, bounds: [min_x, min_y, max_x, max_y], corners });
        }
    }

    results
}

/// Scan QR codes on a given page of a document.
pub fn scan_page_qr(bytes: std::sync::Arc<Vec<u8>>, config: pdfcraft_render::RenderConfig, page: usize) -> Result<Vec<QrCodeInfo>, String> {
    let mut renderer = PageRenderer::new(bytes, config);
    // Render at 150 DPI for sharp QR code detection
    let r = renderer.render(RenderRequest { page, scale: QR_SCAN_SCALE, ..Default::default() });
    if let Some(err) = r.error {
        return Err(err);
    }
    Ok(scan_qr_from_rgba(r.width as usize, r.height as usize, &r.rgba))
}

/// A decoded QR code with its position on the page in user space.
#[derive(Clone, Debug, PartialEq)]
pub struct QrRegion {
    /// Decoded text or payload.
    pub content: String,
    /// Bounds in page user space, `[x0, y0, x1, y1]` with `x0 < x1`, `y0 < y1` (PDF y-up).
    pub rect_user: [f64; 4],
}

/// Render scale used for QR scanning: 150 DPI.
const QR_SCAN_SCALE: f32 = 150.0 / 72.0;

/// Smallest side, in user-space units, of a mapped region (keeps `x0 < x1` and `y0 < y1`).
const MIN_REGION_SIDE: f64 = 1.0;

/// Finite `f32` as `f64`; NaN and infinities become `0.0`.
fn finite_f64(v: f32) -> f64 {
    if v.is_finite() { f64::from(v) } else { 0.0 }
}

/// Clamp `lo..hi` into `min..max` and make it a non-empty, ordered span.
fn ordered_span(lo: f64, hi: f64, min: f64, max: f64) -> (f64, f64) {
    let (mut lo, mut hi) = (lo.clamp(min, max), hi.clamp(min, max));
    if lo > hi {
        std::mem::swap(&mut lo, &mut hi);
    }
    if hi - lo >= MIN_REGION_SIDE {
        return (lo, hi);
    }
    if lo + MIN_REGION_SIDE <= max {
        hi = lo + MIN_REGION_SIDE;
    } else if hi - MIN_REGION_SIDE >= min {
        lo = hi - MIN_REGION_SIDE;
    } else {
        // The crop box itself is smaller than the minimum side: use all of it, nudged open.
        lo = min;
        hi = if max > min { max } else { min + MIN_REGION_SIDE };
    }
    (lo, hi)
}

/// Map pixel bounds `[min_x, min_y, max_x, max_y]` of a render `px_size` (`[width, height]` in
/// pixels) to page user space. The render shows the page as displayed, so the pixels are in view
/// space (y down, after `/Rotate`); the mapping is the one `PageInfo::view_to_user` applies, in
/// fractions of the render so the render scale does not matter. `crop` is the crop box
/// `[x0, y0, x1, y1]` and `rotation` the clockwise `/Rotate` in degrees (taken modulo 360; any
/// value that is not a quarter turn counts as 0). The result is unrotated user space (PDF y-up),
/// clamped into the crop box, with `x0 < x1` and `y0 < y1`.
pub fn pixel_bounds_to_user(bounds: [u32; 4], px_size: [u32; 2], crop: [f32; 4], rotation: u16) -> [f64; 4] {
    let (ca, cb) = (finite_f64(crop[0]), finite_f64(crop[2]));
    let (cc, cd) = (finite_f64(crop[1]), finite_f64(crop[3]));
    let (cx0, cx1) = (ca.min(cb), ca.max(cb));
    let (cy0, cy1) = (cc.min(cd), cc.max(cd));
    let (w, h) = (f64::from(px_size[0]).max(1.0), f64::from(px_size[1]).max(1.0));
    let frac = |px: u32, len: f64| (f64::from(px) / len).clamp(0.0, 1.0);
    let (fx0, fy0, fx1, fy1) = (frac(bounds[0], w), frac(bounds[1], h), frac(bounds[2], w), frac(bounds[3], h));

    // View fractions (a across, b down) -> unrotated fractions (u across from the crop's left, v down from its top).
    let to_uv = |a: f64, b: f64| match rotation % 360 {
        90 => (b, 1.0 - a),
        180 => (1.0 - a, 1.0 - b),
        270 => (1.0 - b, a),
        _ => (a, b),
    };
    let (u0, v0) = to_uv(fx0, fy0);
    let (u1, v1) = to_uv(fx1, fy1);
    let to_user = |u: f64, v: f64| (cx0 + u * (cx1 - cx0), cy1 - v * (cy1 - cy0));
    let (xa, ya) = to_user(u0, v0);
    let (xb, yb) = to_user(u1, v1);

    let (x0, x1) = ordered_span(xa.min(xb), xa.max(xb), cx0, cx1);
    let (y0, y1) = ordered_span(ya.min(yb), ya.max(yb), cy0, cy1);
    [x0, y0, x1, y1]
}

/// Scan a page for QR codes and return each with its user-space rectangle. `info` is the page's
/// [`pdfcraft_render::PageInfo`] (crop box and `/Rotate`); the rectangles are in unrotated user
/// space, ready for the same view transform as any other user-space rectangle.
pub fn scan_page_qr_regions(
    bytes: std::sync::Arc<Vec<u8>>,
    config: pdfcraft_render::RenderConfig,
    page: usize,
    info: &pdfcraft_render::PageInfo,
) -> Result<Vec<QrRegion>, String> {
    let mut renderer = PageRenderer::new(bytes, config);
    let r = renderer.render(RenderRequest { page, scale: QR_SCAN_SCALE, ..Default::default() });
    if let Some(err) = r.error {
        return Err(err);
    }
    let found = scan_qr_from_rgba(r.width as usize, r.height as usize, &r.rgba);
    let regions = found
        .into_iter()
        .map(|q| QrRegion { rect_user: pixel_bounds_to_user(q.bounds, [r.width, r.height], info.crop, info.rotation), content: q.content })
        .collect();
    Ok(regions)
}

/// Keep at most `max_chars` characters of `text`, adding `…` when it was cut.
pub fn truncate_chars(text: &str, max_chars: usize) -> String {
    match text.char_indices().nth(max_chars) {
        Some((byte, _)) => {
            let mut out = text.get(..byte).unwrap_or_default().to_string();
            out.push('…');
            out
        }
        None => text.to_string(),
    }
}

/// `text` as one line to show: line breaks, tabs and other whitespace become spaces, and
/// characters that hide or reorder text (controls, zero-width and bidi marks, the ones
/// [`crate::links::document_url`] refuses) are dropped; then at most `max_chars` characters, as
/// [`truncate_chars`] cuts them. For previews of untrusted payloads.
pub fn preview_text(text: &str, max_chars: usize) -> String {
    let flat: String = text
        .chars()
        .filter_map(|c| match c {
            c if c.is_whitespace() => Some(' '),
            c if crate::links::hides_text(c) => None,
            c => Some(c),
        })
        .collect();
    truncate_chars(&flat, max_chars)
}

/// Determine if a string looks like a web URL (http://, https://, or www.).
pub fn is_url(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.")
}

/// Normalize a URL string so it can be opened directly by a browser.
pub fn normalize_url(text: &str) -> String {
    let t = text.trim();
    if t.starts_with("www.") { format!("https://{}", t) } else { t.to_string() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_buffer_returns_no_qr() {
        let results = scan_qr_from_rgba(0, 0, &[]);
        assert!(results.is_empty());
    }

    #[test]
    fn blank_image_returns_no_qr() {
        let width = 100;
        let height = 100;
        let white_rgba = vec![255u8; width * height * 4];
        let results = scan_qr_from_rgba(width, height, &white_rgba);
        assert!(results.is_empty());
    }

    /// Non-square page, non-origin crop box (200 x 400 user units at x 10..210, y 20..420), rendered at 2 px per unit.
    const CROP: [f32; 4] = [10.0, 20.0, 210.0, 420.0];

    fn close(got: [f64; 4], want: [f64; 4]) {
        assert!(got.iter().zip(want).all(|(g, w)| (g - w).abs() < 1e-3), "got {got:?}, want {want:?}");
    }

    #[test]
    fn region_maps_pixel_bounds_to_user_space() {
        // Unrotated 612 x 792 page at 150 DPI.
        let r = pixel_bounds_to_user([150, 150, 300, 300], [1275, 1650], [0.0, 0.0, 612.0, 792.0], 0);
        close(r, [72.0, 792.0 - 144.0, 144.0, 792.0 - 72.0]);
    }

    #[test]
    fn region_honours_page_rotation() {
        // The same QR at view fractions x 0.1..0.3, y 0.5..0.9 of the displayed page.
        close(pixel_bounds_to_user([40, 400, 120, 720], [400, 800], CROP, 0), [30.0, 60.0, 70.0, 220.0]);
        close(pixel_bounds_to_user([80, 200, 240, 360], [800, 400], CROP, 90), [110.0, 60.0, 190.0, 140.0]);
        close(pixel_bounds_to_user([40, 400, 120, 720], [400, 800], CROP, 180), [150.0, 220.0, 190.0, 380.0]);
        close(pixel_bounds_to_user([80, 200, 240, 360], [800, 400], CROP, 270), [30.0, 300.0, 110.0, 380.0]);
        // An odd rotation is treated as unrotated, and 360 wraps.
        close(pixel_bounds_to_user([40, 400, 120, 720], [400, 800], CROP, 45), [30.0, 60.0, 70.0, 220.0]);
        close(pixel_bounds_to_user([80, 200, 240, 360], [800, 400], CROP, 450), [110.0, 60.0, 190.0, 140.0]);
    }

    #[test]
    fn region_round_trips_through_page_info_view_space() {
        for rotation in [0u16, 90, 180, 270] {
            let (w, h) = if rotation % 180 == 0 { (200.0, 400.0) } else { (400.0, 200.0) };
            let info = pdfcraft_render::PageInfo { width: w, height: h, label: String::new(), crop: CROP, rotation };
            let px = [(w * 2.0) as u32, (h * 2.0) as u32];
            let bounds = [px[0] / 10, px[1] / 5, px[0] / 3, px[1] / 2];
            let r = pixel_bounds_to_user(bounds, px, CROP, rotation);
            let (a, b) = (info.user_to_view(r[0] as f32, r[1] as f32), info.user_to_view(r[2] as f32, r[3] as f32));
            let view = [a[0].min(b[0]), a[1].min(b[1]), a[0].max(b[0]), a[1].max(b[1])];
            let want = bounds.map(|v| v as f32 / 2.0);
            assert!(view.iter().zip(want).all(|(g, w)| (g - w).abs() < 0.05), "rotation {rotation}: {view:?} vs {want:?}");
        }
    }

    #[test]
    fn region_is_clamped_and_ordered_for_degenerate_input() {
        let crop = [10.0_f32, 20.0, 110.0, 220.0];
        for rotation in [0u16, 90, 180, 270] {
            let r = pixel_bounds_to_user([5000, 5000, 5000, 5000], [200, 400], crop, rotation);
            assert!(r[0] < r[2] && r[1] < r[3], "{r:?}");
            assert!(r[0] >= 10.0 && r[2] <= 110.0 && r[1] >= 20.0 && r[3] <= 220.0, "{r:?}");
            let bad = pixel_bounds_to_user([0, 0, 10, 10], [0, 0], [f32::NAN, 0.0, f32::INFINITY, 5.0], rotation);
            assert!(bad.iter().all(|v| v.is_finite()) && bad[0] < bad[2] && bad[1] < bad[3], "{bad:?}");
        }
    }

    #[test]
    fn truncate_chars_keeps_whole_characters() {
        let long = "สวัสดี".repeat(30);
        let t = truncate_chars(&long, 10);
        assert_eq!(t.chars().count(), 11);
        assert!(t.ends_with('…'));
        assert_eq!(t.chars().take(10).collect::<String>(), long.chars().take(10).collect::<String>());
        assert_eq!(truncate_chars("short", 10), "short");
        assert_eq!(truncate_chars("", 0), "");
    }

    #[test]
    fn preview_text_is_one_visible_line() {
        let p = preview_text("https://good.example\u{202E}gpj.exe\nline two\u{200B}\ttab", 64);
        assert!(!p.contains('\u{202E}') && !p.contains('\u{200B}') && !p.contains('\n') && !p.contains('\t'), "{p:?}");
        assert_eq!(p, "https://good.examplegpj.exe line two tab");
        assert_eq!(preview_text(&"ทดสอบ".repeat(30), 10).chars().count(), 11);
        assert_eq!(preview_text("", 5), "");
    }
}

#[cfg(test)]
mod encode_tests;
