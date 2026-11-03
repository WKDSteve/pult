//! The level meter: a bar in three zones, a peak-hold line and a latching clip lamp.

use eframe::egui::{CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};

use super::scale;
use crate::theme::Theme;

/// How long the peak line stays put before it falls.
pub const HOLD_SECONDS: f64 = 1.5;
/// How fast it falls afterwards, in dB per second.
pub const FALL_DB_PER_SECOND: f32 = 20.0;
/// A level at or above this counts as clipping.
pub const CLIP_DBFS: f32 = -0.1;
/// Zone limits, in dBFS.
const MID_DBFS: f32 = -18.0;
const HIGH_DBFS: f32 = -6.0;
/// Height of the clip lamp above the bar.
const LAMP_HEIGHT: f32 = 6.0;

/// What a meter remembers between frames: the held peak and the clip latch.
#[derive(Clone, Debug, PartialEq)]
pub struct MeterState {
    held_db: f32,
    held_at: f64,
    clipped: bool,
}

impl Default for MeterState {
    fn default() -> Self {
        Self {
            held_db: f32::NEG_INFINITY,
            held_at: 0.0,
            clipped: false,
        }
    }
}

impl MeterState {
    /// Takes the level of this frame (`None`: no signal) at time `now` in seconds.
    pub fn update(&mut self, level_db: Option<f32>, now: f64) {
        let shown = self.held(now);
        let level = level_db.unwrap_or(f32::NEG_INFINITY);
        if level >= shown {
            self.held_db = level;
            self.held_at = now;
        }
        if level >= CLIP_DBFS {
            self.clipped = true;
        }
    }

    /// The peak line at time `now`: held, then falling.
    #[must_use]
    pub fn held(&self, now: f64) -> f32 {
        let since = now - self.held_at - HOLD_SECONDS;
        if since <= 0.0 || self.held_db == f32::NEG_INFINITY {
            self.held_db
        } else {
            // A meter frame is never long enough for the f64 -> f32 step to matter.
            #[allow(clippy::cast_possible_truncation)]
            let fallen = self.held_db - since as f32 * FALL_DB_PER_SECOND;
            if fallen < scale::METER_FLOOR_DB {
                f32::NEG_INFINITY
            } else {
                fallen
            }
        }
    }

    /// Whether the clip lamp is lit; it stays lit until [`Self::reset_clip`].
    #[must_use]
    pub fn clipped(&self) -> bool {
        self.clipped
    }

    /// Lights the clip lamp for a clip the mixer counted (a bus clips after its meter).
    pub fn mark_clipped(&mut self) {
        self.clipped = true;
    }

    pub fn reset_clip(&mut self) {
        self.clipped = false;
    }
}

/// Draws a meter of `size` showing `level_db`; a click on it resets the clip lamp.
pub fn meter(
    ui: &mut Ui,
    state: &mut MeterState,
    level_db: Option<f32>,
    now: f64,
    size: Vec2,
) -> Response {
    let theme = Theme::current();
    let p = &theme.palette;
    let (rect, response) = ui.allocate_exact_size(size, Sense::click());
    state.update(level_db, now);
    if response.clicked() {
        state.reset_clip();
    }
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter_at(rect);

    let lamp = Rect::from_min_size(rect.min, vec2(rect.width(), LAMP_HEIGHT));
    let bar = Rect::from_min_max(pos2(rect.left(), lamp.bottom() + 3.0), rect.max);
    let lamp_color = if state.clipped() {
        p.meter_high
    } else {
        p.raised
    };
    painter.rect_filled(lamp, CornerRadius::same(1), lamp_color);

    painter.rect_filled(bar, CornerRadius::same(1), p.well);
    let y_of = |db: f32| bar.bottom() - bar.height() * scale::meter_pos(db);
    let level = level_db.unwrap_or(f32::NEG_INFINITY);
    // Each zone is drawn dim over its whole range and lit up to the level.
    for (from, to, color) in [
        (scale::METER_FLOOR_DB, MID_DBFS, p.meter_low),
        (MID_DBFS, HIGH_DBFS, p.meter_mid),
        (HIGH_DBFS, 0.0, p.meter_high),
    ] {
        let zone = Rect::from_x_y_ranges(bar.x_range(), y_of(to)..=y_of(from));
        painter.rect_filled(
            zone.shrink2(vec2(1.0, 0.0)),
            0.0,
            color.gamma_multiply(theme.shades.unlit),
        );
        if level > from {
            let lit = Rect::from_x_y_ranges(bar.x_range(), y_of(level.min(to))..=y_of(from));
            painter.rect_filled(lit.shrink2(vec2(1.0, 0.0)), 0.0, color);
        }
    }
    let held = state.held(now);
    if held > scale::METER_FLOOR_DB {
        let y = y_of(held.min(0.0)).round() - 0.5;
        let color = if held >= HIGH_DBFS {
            p.meter_high
        } else {
            p.text
        };
        painter.line_segment(
            [pos2(bar.left() + 1.0, y), pos2(bar.right() - 1.0, y)],
            (2.0, color),
        );
    }
    painter.rect_stroke(
        bar,
        CornerRadius::same(1),
        Stroke::new(1.0, p.line),
        StrokeKind::Outside,
    );
    response
}

/// A thin horizontal meter without hold or lamp, for lists (the matrix rows): the same zones
/// and scale as the strip meter.
pub fn level_bar(painter: &eframe::egui::Painter, rect: Rect, level_db: Option<f32>) {
    let p = &Theme::current().palette;
    painter.rect_filled(rect, CornerRadius::same(1), p.well);
    let level = level_db.unwrap_or(f32::NEG_INFINITY);
    let x_of = |db: f32| rect.left() + rect.width() * scale::meter_pos(db);
    for (from, to, color) in [
        (scale::METER_FLOOR_DB, MID_DBFS, p.meter_low),
        (MID_DBFS, HIGH_DBFS, p.meter_mid),
        (HIGH_DBFS, 0.0, p.meter_high),
    ] {
        if level > from {
            let lit = Rect::from_x_y_ranges(x_of(from)..=x_of(level.min(to)), rect.y_range());
            painter.rect_filled(lit, 0.0, color);
        }
    }
}

/// Draws the dBFS numbers of a meter into a strip of `size`, aligned with a meter of the same
/// height drawn next to it.
pub fn meter_scale(ui: &mut Ui, size: Vec2) {
    let theme = Theme::current();
    let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
    let bar_top = rect.top() + LAMP_HEIGHT + 3.0;
    let height = rect.bottom() - bar_top;
    let painter = ui.painter_at(rect.expand2(vec2(0.0, 6.0)));
    for mark in scale::METER_MARKS {
        let y = rect.bottom() - height * scale::meter_pos(mark);
        painter.text(
            pos2(rect.left(), y),
            eframe::egui::Align2::LEFT_CENTER,
            format!("{}", mark.abs()),
            Theme::value_font(8.5),
            theme.palette.text_faint,
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn peak_holds_then_falls() {
        let mut meter = MeterState::default();
        meter.update(Some(-6.0), 0.0);
        meter.update(Some(-30.0), 1.0);
        assert_eq!(meter.held(1.0), -6.0);
        assert_eq!(meter.held(HOLD_SECONDS), -6.0);
        let later = meter.held(HOLD_SECONDS + 0.5);
        assert!(
            (later - (-6.0 - 0.5 * FALL_DB_PER_SECOND)).abs() < 1e-4,
            "{later}"
        );
        assert_eq!(meter.held(HOLD_SECONDS + 10.0), f32::NEG_INFINITY);
    }

    #[test]
    fn a_louder_peak_restarts_the_hold() {
        let mut meter = MeterState::default();
        meter.update(Some(-12.0), 0.0);
        meter.update(Some(-3.0), 1.0);
        assert_eq!(meter.held(1.0 + HOLD_SECONDS), -3.0);
        // A level under a falling line does not lift it.
        meter.update(Some(-40.0), 3.0);
        assert!(meter.held(3.0) > -40.0);
    }

    #[test]
    fn falling_line_is_caught_by_the_level() {
        let mut meter = MeterState::default();
        meter.update(Some(-6.0), 0.0);
        let t = HOLD_SECONDS + 0.5; // the line is at -16 dB now
        meter.update(Some(-10.0), t);
        assert_eq!(meter.held(t), -10.0);
    }

    #[test]
    fn clip_latches_until_reset() {
        let mut meter = MeterState::default();
        meter.update(Some(-1.0), 0.0);
        assert!(!meter.clipped());
        meter.update(Some(0.0), 0.1);
        meter.update(None, 5.0);
        assert!(meter.clipped());
        meter.reset_clip();
        assert!(!meter.clipped());
    }
}
