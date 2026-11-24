//! The scan effect: when a QR code is right-clicked, a bright line sweeps down and back over that
//! code (and only that code) before the menu's answer is read, so it is clear which code the
//! menu is about. It plays once per right-click and lasts [`DURATION`] seconds.

use egui::{Color32, Painter, Pos2, Rect, Stroke};

/// Seconds the sweep takes, down and back.
pub(crate) const DURATION: f64 = 0.9;

/// Where the sweep line is, as a fraction of the code's height from its top, `progress` (0 to 1)
/// of the way through: down to the bottom at the half-way point and back to the top. Anything
/// outside 0 to 1 (or not a number) counts as the end.
pub(crate) fn sweep_position(progress: f32) -> f32 {
    if !progress.is_finite() {
        return 0.0;
    }
    let p = progress.clamp(0.0, 1.0);
    1.0 - (2.0 * p - 1.0).abs()
}

/// The sweep being played, if any: the page, the code's box in user space, when it began.
#[derive(Default)]
pub(crate) struct ScanFx {
    playing: Option<(usize, [f64; 4], f64)>,
}

impl ScanFx {
    /// Begin the sweep over the code at `rect` (user space) on `page`, `now` seconds on egui's
    /// clock. A right-click on another code, or the same one again, starts it over.
    pub(crate) fn start(&mut self, page: usize, rect: [f64; 4], now: f64) {
        self.playing = Some((page, rect, now));
    }

    /// The code to draw the sweep over on `page` at `now` and how far through it is (0 to 1), or
    /// `None` when none is playing there (the sweep is dropped once it has run its length).
    pub(crate) fn step(&mut self, page: usize, now: f64) -> Option<([f64; 4], f32)> {
        let (on, rect, began) = self.playing?;
        let elapsed = now - began;
        if !elapsed.is_finite() || !(0.0..DURATION).contains(&elapsed) {
            // Over, or the clock went backwards: nothing plays.
            self.playing = None;
            return None;
        }
        (on == page).then_some((rect, (elapsed / DURATION) as f32))
    }
}

/// Draw the sweep over `rect` (screen), `progress` of the way through, in `color`: a bright line
/// across the code with a fading trail behind it.
pub(crate) fn paint_sweep(painter: &Painter, rect: Rect, progress: f32, color: Color32) {
    if !rect.is_positive() {
        return;
    }
    let painter = painter.with_clip_rect(rect.expand(3.0));
    let y = rect.top() + rect.height() * sweep_position(progress);
    // The line fades in and out at the ends so it does not flash.
    let edge = (progress.clamp(0.0, 1.0) * 2.0).min((1.0 - progress.clamp(0.0, 1.0)) * 2.0).min(1.0);
    let [r, g, b, _] = color.to_srgba_unmultiplied();
    let going_down = progress < 0.5;
    let (left, right) = (rect.left() - 2.0, rect.right() + 2.0);
    // A band of light behind the line, so it shows on black squares as on white paper: the trail
    // fades away from the line, above it going down and below it coming back.
    let trail = (rect.height() * 0.35).clamp(10.0, 60.0);
    for step in 0..8u8 {
        let (from, to) = (f32::from(step) / 8.0 * trail, f32::from(step + 1) / 8.0 * trail);
        let (a, b_) = if going_down { (y - to, y - from) } else { (y + from, y + to) };
        let alpha = (110.0 * edge * (1.0 - f32::from(step) / 8.0)) as u8;
        painter.rect_filled(Rect::from_min_max(Pos2::new(left, a), Pos2::new(right, b_)), 0.0, Color32::from_rgba_unmultiplied(r, g, b, alpha));
    }
    // The line itself: the colour, thick, with a white core so it stands out on either.
    let line = [Pos2::new(left, y), Pos2::new(right, y)];
    painter.line_segment(line, Stroke::new(5.0, Color32::from_rgba_unmultiplied(r, g, b, (255.0 * edge) as u8)));
    painter.line_segment(line, Stroke::new(2.0, Color32::from_rgba_unmultiplied(255, 255, 255, (255.0 * edge) as u8)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_line_goes_down_and_back() {
        assert_eq!(sweep_position(0.0), 0.0);
        assert_eq!(sweep_position(0.5), 1.0);
        assert_eq!(sweep_position(1.0), 0.0);
        assert!(sweep_position(0.25) > 0.0 && sweep_position(0.25) < 1.0);
        assert!((sweep_position(0.25) - sweep_position(0.75)).abs() < 1e-6, "the way back mirrors the way down");
        let mut last = -1.0;
        for i in 0..=10 {
            let p = sweep_position(i as f32 / 20.0);
            assert!(p >= last, "going down only goes down");
            last = p;
        }
    }

    #[test]
    fn a_progress_that_is_not_on_the_scale_stays_on_the_code() {
        for p in [-1.0, 2.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let y = sweep_position(p);
            assert!((0.0..=1.0).contains(&y), "{p} gave {y}");
        }
    }

    #[test]
    fn the_sweep_plays_once_on_its_page_then_stops() {
        let mut fx = ScanFx::default();
        assert_eq!(fx.step(0, 10.0), None, "nothing was started");
        fx.start(2, [10.0, 20.0, 50.0, 60.0], 10.0);
        let (rect, p) = fx.step(2, 10.0 + DURATION / 2.0).unwrap();
        assert_eq!(rect, [10.0, 20.0, 50.0, 60.0]);
        assert!((p - 0.5).abs() < 1e-6);
        assert_eq!(fx.step(1, 10.2), None, "another page shows nothing");
        assert!(fx.step(2, 10.2).is_some(), "and does not end it for the page it is on");
        assert_eq!(fx.step(2, 10.0 + DURATION), None, "it ends at its length");
        assert_eq!(fx.step(2, 10.1), None, "and does not start again by itself");
    }

    #[test]
    fn a_new_right_click_starts_it_over() {
        let mut fx = ScanFx::default();
        fx.start(0, [0.0, 0.0, 10.0, 10.0], 5.0);
        assert!(fx.step(0, 5.5).is_some());
        fx.start(0, [20.0, 20.0, 30.0, 30.0], 5.6);
        let (rect, p) = fx.step(0, 5.6).unwrap();
        assert_eq!(rect, [20.0, 20.0, 30.0, 30.0]);
        assert!(p.abs() < 1e-6);
    }

    #[test]
    fn a_clock_that_runs_backwards_or_is_not_a_number_plays_nothing() {
        let mut fx = ScanFx::default();
        fx.start(0, [0.0, 0.0, 10.0, 10.0], 5.0);
        assert_eq!(fx.step(0, 4.0), None);
        fx.start(0, [0.0, 0.0, 10.0, 10.0], 5.0);
        assert_eq!(fx.step(0, f64::NAN), None);
    }

    #[test]
    fn an_empty_box_paints_nothing_and_does_not_panic() {
        let ctx = egui::Context::default();
        let painter = Painter::new(ctx, egui::LayerId::background(), Rect::EVERYTHING);
        paint_sweep(&painter, Rect::from_min_max(Pos2::ZERO, Pos2::ZERO), 0.3, Color32::GREEN);
        paint_sweep(&painter, Rect::from_min_max(Pos2::new(f32::NAN, 0.0), Pos2::new(10.0, 10.0)), 0.3, Color32::GREEN);
    }
}
