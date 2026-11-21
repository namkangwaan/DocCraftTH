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
    let r = renderer.render(RenderRequest { page, scale: 150.0 / 72.0, ..Default::default() });
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

/// Map pixel bounds `[min_x, min_y, max_x, max_y]` of a render made at `scale` (pixels per user
/// unit) of a page with crop box `crop` (`[x0, y0, x1, y1]`) to user space. The pixel y axis points
/// down and the user y axis up, so `user_y = crop_y1 - pixel_y / scale`. `px_h` is the render
/// height, used to clamp pixel y. The result is clamped into the crop box and always has
/// `x0 < x1` and `y0 < y1`.
pub fn pixel_bounds_to_user(bounds: [u32; 4], scale: f32, crop: [f32; 4], px_h: u32) -> [f64; 4] {
    let (ca, cb) = (finite_f64(crop[0]), finite_f64(crop[2]));
    let (cc, cd) = (finite_f64(crop[1]), finite_f64(crop[3]));
    let (cx0, cx1) = (ca.min(cb), ca.max(cb));
    let (cy0, cy1) = (cc.min(cd), cc.max(cd));
    let s = if scale.is_finite() && scale > 0.0 { f64::from(scale) } else { 1.0 };
    let h = f64::from(px_h);
    let py_top = f64::from(bounds[1]).min(h);
    let py_bottom = f64::from(bounds[3]).min(h);

    let x0 = cx0 + f64::from(bounds[0]) / s;
    let x1 = cx0 + f64::from(bounds[2]) / s;
    let y_hi = cy1 - py_top / s;
    let y_lo = cy1 - py_bottom / s;

    let (x0, x1) = ordered_span(x0, x1, cx0, cx1);
    let (y0, y1) = ordered_span(y_lo, y_hi, cy0, cy1);
    [x0, y0, x1, y1]
}

/// Scan a page for QR codes and return each with its user-space rectangle. `crop` is the page
/// crop box in user space, `[x0, y0, x1, y1]`.
pub fn scan_page_qr_regions(
    bytes: std::sync::Arc<Vec<u8>>,
    config: pdfcraft_render::RenderConfig,
    page: usize,
    crop: [f32; 4],
) -> Result<Vec<QrRegion>, String> {
    let mut renderer = PageRenderer::new(bytes, config);
    let r = renderer.render(RenderRequest { page, scale: QR_SCAN_SCALE, ..Default::default() });
    if let Some(err) = r.error {
        return Err(err);
    }
    let found = scan_qr_from_rgba(r.width as usize, r.height as usize, &r.rgba);
    Ok(found.into_iter().map(|q| QrRegion { rect_user: pixel_bounds_to_user(q.bounds, QR_SCAN_SCALE, crop, r.height), content: q.content }).collect())
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

    #[test]
    fn region_maps_pixel_bounds_to_user_space() {
        let scale = 150.0_f32 / 72.0;
        let r = pixel_bounds_to_user([150, 150, 300, 300], scale, [0.0, 0.0, 612.0, 792.0], 1650);
        assert!((r[0] - 72.0).abs() < 0.01, "x0 {}", r[0]);
        assert!((r[2] - 144.0).abs() < 0.01, "x1 {}", r[2]);
        assert!((r[3] - (792.0 - 72.0)).abs() < 0.01, "y1 {}", r[3]);
        assert!((r[1] - (792.0 - 144.0)).abs() < 0.01, "y0 {}", r[1]);
    }

    #[test]
    fn region_is_clamped_and_ordered_for_degenerate_input() {
        let crop = [10.0_f32, 20.0, 110.0, 220.0];
        let r = pixel_bounds_to_user([5000, 5000, 5000, 5000], 2.0, crop, 400);
        assert!(r[0] < r[2] && r[1] < r[3], "{r:?}");
        assert!(r[0] >= 10.0 && r[2] <= 110.0 && r[1] >= 20.0 && r[3] <= 220.0, "{r:?}");
        let bad = pixel_bounds_to_user([0, 0, 10, 10], 0.0, crop, 400);
        assert!(bad.iter().all(|v| v.is_finite()) && bad[0] < bad[2] && bad[1] < bad[3]);
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
}
