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

            results.push(QrCodeInfo {
                content: decoded,
                bounds: [min_x, min_y, max_x, max_y],
                corners,
            });
        }
    }

    results
}

/// Scan QR codes on a given page of a document.
pub fn scan_page_qr(
    bytes: std::sync::Arc<Vec<u8>>,
    config: pdfcraft_render::RenderConfig,
    page: usize,
) -> Result<Vec<QrCodeInfo>, String> {
    let mut renderer = PageRenderer::new(bytes, config);
    // Render at 150 DPI for sharp QR code detection
    let r = renderer.render(RenderRequest {
        page,
        scale: 150.0 / 72.0,
        ..Default::default()
    });
    if let Some(err) = r.error {
        return Err(err);
    }
    Ok(scan_qr_from_rgba(r.width as usize, r.height as usize, &r.rgba))
}

/// Determine if a string looks like a web URL (http://, https://, or www.).
pub fn is_url(text: &str) -> bool {
    let t = text.trim();
    t.starts_with("http://") || t.starts_with("https://") || t.starts_with("www.")
}

/// Normalize a URL string so it can be opened directly by a browser.
pub fn normalize_url(text: &str) -> String {
    let t = text.trim();
    if t.starts_with("www.") {
        format!("https://{}", t)
    } else {
        t.to_string()
    }
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
}
