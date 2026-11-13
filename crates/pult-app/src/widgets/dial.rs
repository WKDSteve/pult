//! A small rotary control for a number between two ends: the one knob of a gate or a compressor,
//! the gain, frequency and width of a band of an equalizer, the ceiling and the lookahead of a
//! limiter. What it does comes from the parameter
//! it is bound to.

use std::f32::consts::PI;

use eframe::egui::{
    Align, Layout, Response, RichText, Sense, Shape, Stroke, Ui, Vec2, WidgetInfo, vec2,
};
use pult_core::eq::{Band, EQ_FREQ_HZ, EQ_GAIN_DB, EQ_Q};
use pult_core::limiter::{
    DEFAULT_CEILING_DB, DEFAULT_LOOKAHEAD_MS, LIMIT_CEILING_DB, LIMIT_LOOKAHEAD_MS,
};
use pult_core::multiband::{HIGH_SPLIT_HZ, LOW_SPLIT_HZ, MultibandParam};
use pult_core::scale::{FADER_FLOOR_DB, FADER_TOP_DB, Span};
use pult_core::strip::{HPF_RANGE_HZ, LPF_RANGE_HZ, MAX_DELAY_MS, WIDTH_RANGE};
use pult_core::view::{AMOUNT_RANGE, Action, MixerView};

use crate::binding::{BandParam, Binding, BusParam, InputParam, Value, Values};
use crate::theme::Theme;

/// Vertical drag distance for the whole way, in points.
const DRAG_RANGE: f32 = 160.0;
/// Shift slows a drag down by this factor.
const FINE_DRAG: f32 = 0.1;
/// The part of the way a wheel notch or a step of a screen reader goes.
const STEP: f32 = 1.0 / 48.0;
/// It turns through 270 degrees, with the gap at the bottom.
const SWEEP: f32 = 1.5 * PI;

/// How a dial behaves.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Dial {
    /// Its two ends and how the way between them is divided.
    pub span: Span,
    /// Where a double click puts it.
    pub home: f32,
    /// Its arc starts at `home` instead of at the lower end: for what goes both ways.
    pub both_ways: bool,
}

impl Dial {
    /// The dial of `binding`; `None` for what is no number between two ends.
    #[must_use]
    pub fn of(binding: Binding) -> Option<Self> {
        match binding {
            Binding::Input(_, InputParam::Gate | InputParam::Comp)
            | Binding::Bus(_, BusParam::Comp | BusParam::Multiband(MultibandParam::Band(_))) => {
                Some(Self {
                    span: Span::even(AMOUNT_RANGE),
                    home: 0.0,
                    both_ways: false,
                })
            }
            Binding::Input(_, InputParam::Hpf) => Some(Self {
                span: Span::octaves(HPF_RANGE_HZ),
                home: HPF_RANGE_HZ.0,
                both_ways: false,
            }),
            Binding::Input(_, InputParam::Lpf) => Some(Self {
                span: Span::octaves(LPF_RANGE_HZ),
                home: LPF_RANGE_HZ.1,
                // Its arc grows down from the open end, as the filter closes.
                both_ways: true,
            }),
            Binding::Input(_, InputParam::Delay) => Some(Self {
                span: Span::even((0.0, MAX_DELAY_MS)),
                home: 0.0,
                both_ways: false,
            }),
            Binding::Input(_, InputParam::Width) => Some(Self {
                span: Span::even(WIDTH_RANGE),
                home: 100.0,
                both_ways: true,
            }),
            // The level of a crossing: the range of a fader, 0 dB at home.
            Binding::Send { .. } => Some(Self {
                span: Span::even((FADER_FLOOR_DB, FADER_TOP_DB)),
                home: 0.0,
                both_ways: true,
            }),
            Binding::Bus(_, BusParam::CompMix) => Some(Self {
                span: Span::even((0.0, 100.0)),
                home: 100.0,
                both_ways: false,
            }),
            Binding::Bus(_, BusParam::Multiband(MultibandParam::LowSplit)) => Some(Self {
                span: Span::octaves(LOW_SPLIT_HZ),
                home: 200.0,
                both_ways: true,
            }),
            Binding::Bus(_, BusParam::Multiband(MultibandParam::HighSplit)) => Some(Self {
                span: Span::octaves(HIGH_SPLIT_HZ),
                home: 2000.0,
                both_ways: true,
            }),
            Binding::Bus(_, BusParam::DimLevel) => Some(Self {
                span: Span::even(pult_core::view::DIM_RANGE_DB),
                home: pult_core::view::DEFAULT_DIM_DB,
                both_ways: false,
            }),
            Binding::Bus(_, BusParam::CompHpf) => Some(Self {
                span: Span::octaves(pult_core::buscomp::HPF_HZ),
                home: pult_core::buscomp::HPF_HZ.0,
                both_ways: false,
            }),
            Binding::Bus(_, BusParam::LimitCeiling)
            | Binding::Input(_, InputParam::LimitCeiling) => Some(Self {
                span: Span::even(LIMIT_CEILING_DB),
                home: DEFAULT_CEILING_DB,
                both_ways: false,
            }),
            Binding::Bus(_, BusParam::LimitLookahead) => Some(Self {
                span: Span::even(LIMIT_LOOKAHEAD_MS),
                home: DEFAULT_LOOKAHEAD_MS,
                both_ways: false,
            }),
            Binding::Bus(_, BusParam::EqBand(band, param))
            | Binding::Input(_, InputParam::EqBand(band, param)) => {
                let start = Band::at(band);
                match param {
                    BandParam::Gain => Some(Self {
                        span: Span::even(EQ_GAIN_DB),
                        home: start.gain_db,
                        both_ways: true,
                    }),
                    BandParam::Freq => Some(Self {
                        span: Span::octaves(EQ_FREQ_HZ),
                        home: start.freq_hz,
                        both_ways: false,
                    }),
                    BandParam::Q => Some(Self {
                        span: Span::octaves(EQ_Q),
                        home: start.q,
                        both_ways: false,
                    }),
                    BandParam::Kind => None,
                }
            }
            _ => None,
        }
    }

    /// The value after dragging by `dy` points (up is negative, turns up).
    #[must_use]
    pub fn dragged(self, value: f32, dy: f32, fine: bool) -> f32 {
        let speed = if fine { FINE_DRAG } else { 1.0 };
        self.span
            .value(self.span.pos(value) - dy / DRAG_RANGE * speed)
    }

    /// The value after `steps` steps, up positive.
    #[must_use]
    pub fn stepped(self, value: f32, steps: f32) -> f32 {
        self.span.value(self.span.pos(value) + steps * STEP)
    }

    /// How much a step from `value` changes it. By octaves that depends on where it stands; at the
    /// upper end, the step down.
    #[must_use]
    pub fn step_at(self, value: f32) -> f32 {
        let up = self.stepped(value, 1.0) - value;
        if up > 0.0 {
            up
        } else {
            value - self.stepped(value, -1.0)
        }
    }
}

/// The number in `value`; `None` for what is no number of a dial.
#[must_use]
pub fn number(value: Value) -> Option<f32> {
    match value {
        Value::Amount(number)
        | Value::Boost(number)
        | Value::Freq(number)
        | Value::Width(number)
        | Value::Ceiling(number)
        | Value::Lookahead(number)
        | Value::Percent(number)
        | Value::Delay(number) => Some(number),
        _ => None,
    }
}

/// `value` with another `number` in it.
#[must_use]
fn with_number(value: Value, number: f32) -> Value {
    match value {
        Value::Amount(_) => Value::Amount(number),
        Value::Boost(_) => Value::Boost(number),
        Value::Freq(_) => Value::Freq(number),
        Value::Width(_) => Value::Width(number),
        Value::Ceiling(_) => Value::Ceiling(number),
        Value::Lookahead(_) => Value::Lookahead(number),
        Value::Percent(_) => Value::Percent(number),
        Value::Delay(_) => Value::Delay(number),
        other => other,
    }
}

/// `value` as a dial prints it: `off`, `3.5`; `+6.0`; `500`, `2.5k`; `0.70`.
#[must_use]
pub fn format_value(value: Value) -> String {
    match value {
        Value::Amount(amount) if amount > 0.0 => format!("{amount:.1}"),
        Value::Amount(_) => "off".to_owned(),
        Value::Boost(db) if db.abs() < 0.05 => "0 dB".to_owned(),
        Value::Boost(db) => format!("{db:+.1} dB"),
        Value::Freq(hz) if hz >= 1000.0 => format!("{:.1}k", hz / 1000.0),
        Value::Freq(hz) => format!("{hz:.0}"),
        Value::Width(q) => format!("{q:.2}"),
        Value::Ceiling(db) => format!("{db:.1} dB"),
        Value::Lookahead(ms) | Value::Delay(ms) => format!("{ms:.1} ms"),
        Value::Percent(percent) => format!("{percent:.0} %"),
        _ => String::new(),
    }
}

/// `value` as the dial of `binding` prints it: a filter at its open end is `off`.
#[must_use]
pub fn format_for(binding: Binding, value: Value) -> String {
    match (binding, value) {
        (Binding::Input(_, InputParam::Hpf), Value::Freq(hz)) if hz <= HPF_RANGE_HZ.0 => {
            "off".to_owned()
        }
        (Binding::Input(_, InputParam::Lpf), Value::Freq(hz)) if hz >= LPF_RANGE_HZ.1 => {
            "off".to_owned()
        }
        _ => format_value(value),
    }
}

/// Angle of a position from 0 to 1, in radians, clockwise from straight up.
fn angle(pos: f32) -> f32 {
    (pos - 0.5) * SWEEP
}

/// Draws a dial of diameter `size`; `changed()` tells a move. `label` names it for screen readers.
pub fn dial(ui: &mut Ui, value: &mut f32, dial: Dial, size: f32, label: &str) -> Response {
    let theme = Theme::current();
    let p = &theme.palette;
    let (rect, mut response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click_and_drag());
    let before = *value;
    // Only a real movement: through a position and back, a number would change by its rounding.
    let dy = response.drag_delta().y;
    if response.dragged() && dy != 0.0 {
        let fine = ui.input(|i| i.modifiers.shift);
        *value = dial.dragged(*value, dy, fine);
    }
    if response.double_clicked() {
        *value = dial.home;
    }
    if response.hovered() {
        let wheel = super::wheel_notches(ui);
        if wheel != 0.0 {
            *value = dial.stepped(*value, wheel);
        }
        // The wheel is this dial's now: a page that scrolls would carry it away under the pointer.
        ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
    }
    if let Some(asked) = super::assisted(ui, response.id) {
        *value = match asked {
            #[allow(clippy::cast_possible_truncation)]
            super::Assist::Set(set) => (set as f32).clamp(dial.span.low(), dial.span.high()),
            super::Assist::Steps(steps) => dial.stepped(*value, steps),
        };
    }
    if value.to_bits() != before.to_bits() {
        response.mark_changed();
    }
    response.widget_info(|| WidgetInfo::slider(true, f64::from(*value), label));
    let (low, high) = (f64::from(dial.span.low()), f64::from(dial.span.high()));
    super::offer_range(ui, response.id, low..=high, f64::from(dial.step_at(*value)));

    if !ui.is_rect_visible(rect) {
        return response;
    }
    let painter = ui.painter();
    let center = rect.center();
    let radius = size / 2.0 - 2.0;
    let point = |a: f32, r: f32| center + vec2(a.sin(), -a.cos()) * r;
    let arc = |from: f32, to: f32| -> Vec<_> {
        let steps = 24;
        (0..=steps)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = i as f32 / steps as f32;
                point(from + (to - from) * t, radius)
            })
            .collect()
    };
    let at = angle(dial.span.pos(*value));
    let start = if dial.both_ways {
        angle(dial.span.pos(dial.home))
    } else {
        -SWEEP / 2.0
    };
    painter.add(Shape::line(
        arc(-SWEEP / 2.0, SWEEP / 2.0),
        Stroke::new(2.0, p.well),
    ));
    if (at - start).abs() > 1e-3 {
        painter.add(Shape::line(arc(start, at), Stroke::new(2.0, p.accent)));
    }
    let active = response.dragged() || response.has_focus();
    let body = if response.hovered() || active {
        p.raised_hover
    } else {
        p.raised
    };
    painter.circle(center, radius - 3.0, body, Stroke::new(1.0, p.line_strong));
    painter.line_segment(
        [point(at, 2.0), point(at, radius - 4.0)],
        Stroke::new(1.5, if active { p.accent } else { p.text }),
    );
    response
}

/// A dial bound to `binding` of width `width`: `text` over it, its value under it. Returns what the
/// user changed.
pub fn labelled(
    ui: &mut Ui,
    values: &mut Values<'_>,
    binding: Binding,
    text: &str,
    size: f32,
    width: f32,
) -> Option<Action> {
    let p = &Theme::current().palette;
    let the_dial = Dial::of(binding)?;
    let value = values.read(binding)?;
    let mut set = number(value)?;
    let title = binding.title(values.view());
    let mut action = None;
    ui.allocate_ui_with_layout(
        vec2(width, ui.available_height()),
        Layout::top_down(Align::Center),
        |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = 1.0;
            ui.label(RichText::new(text).size(8.5).color(p.text_faint));
            let changed = dial(ui, &mut set, the_dial, size, &title).changed();
            let shown = with_number(value, set);
            if changed {
                action = values.write(binding, shown);
            }
            ui.label(
                RichText::new(format_for(binding, shown))
                    .font(Theme::value_font(9.0))
                    .color(p.text_dim),
            );
            if let Some((text, active)) = reduction(values.view(), binding) {
                let color = if active { p.accent } else { p.text_faint };
                ui.label(
                    RichText::new(text)
                        .font(Theme::value_font(8.5))
                        .color(color),
                );
            }
        },
    );
    action
}

/// What a gate or compressor of an input takes away now, as the line under its dial, and
/// whether it is at work: a gate says `open` or how much its closed state takes, a compressor how
/// many dB it takes. `None` for any other dial, and for one that is off.
#[must_use]
pub fn reduction(view: &MixerView, binding: Binding) -> Option<(String, bool)> {
    match binding {
        Binding::Input(index, InputParam::Gate) => {
            let input = view.inputs.get(index)?;
            if input.gate <= 0.0 {
                return None;
            }
            Some(if input.gate_open && input.gate_reduction_db < 0.5 {
                ("open".to_owned(), false)
            } else {
                (format!("shut -{:.0}", input.gate_reduction_db), true)
            })
        }
        Binding::Input(index, InputParam::Comp) => {
            let input = view.inputs.get(index)?;
            if input.comp <= 0.0 {
                return None;
            }
            let db = input.comp_reduction_db;
            Some((format!("-{db:.1} dB"), db >= 0.5))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GATE: Binding = Binding::Input(0, InputParam::Gate);

    fn band(param: BandParam) -> Binding {
        Binding::Bus(0, BusParam::EqBand(1, param))
    }

    #[test]
    fn a_dial_stays_between_its_ends() {
        let dial = Dial::of(GATE).expect("a dial");
        assert_eq!(dial.dragged(0.0, 1000.0, false), 0.0);
        assert_eq!(dial.dragged(0.0, -1000.0, false), 10.0);
        assert!((dial.dragged(0.0, -80.0, false) - 5.0).abs() < 1e-4);
        assert!((dial.dragged(0.0, -80.0, true) - 0.5).abs() < 1e-4);
        assert_eq!(dial.stepped(10.0, 3.0), 10.0);
    }

    #[test]
    fn a_frequency_turns_by_octaves() {
        let dial = Dial::of(band(BandParam::Freq)).expect("a dial");
        assert_eq!(dial.home, 500.0);
        // Half the way up from 20 Hz: where 20 kHz has as many octaves as 20 Hz.
        let middle = dial.dragged(20.0, -80.0, false);
        assert!((middle - 632.5).abs() < 1.0, "{middle}");
    }

    #[test]
    fn a_step_says_what_it_comes_to_where_the_dial_stands() {
        let even = Dial::of(GATE).expect("a dial");
        assert!((even.step_at(0.0) - 10.0 / 48.0).abs() < 1e-4);
        assert!(
            (even.step_at(10.0) - 10.0 / 48.0).abs() < 1e-4,
            "at the top"
        );
        let octaves = Dial::of(band(BandParam::Freq)).expect("a dial");
        let (low, high) = (octaves.step_at(20.0), octaves.step_at(500.0));
        assert!((low - 3.1).abs() < 0.1, "{low}");
        assert!((high - 77.0).abs() < 2.0, "{high}");
    }

    #[test]
    fn only_what_is_a_number_between_two_ends_has_a_dial() {
        assert!(Dial::of(band(BandParam::Gain)).is_some_and(|dial| dial.both_ways));
        assert!(Dial::of(band(BandParam::Q)).is_some());
        assert_eq!(Dial::of(band(BandParam::Kind)), None);
        let ceiling = Dial::of(Binding::Bus(0, BusParam::LimitCeiling)).expect("a dial");
        assert_eq!(ceiling.home, DEFAULT_CEILING_DB);
        let lookahead = Dial::of(Binding::Bus(0, BusParam::LimitLookahead)).expect("a dial");
        assert_eq!(lookahead.home, DEFAULT_LOOKAHEAD_MS);
        assert_eq!(Dial::of(Binding::Bus(0, BusParam::Limit)), None);
        assert_eq!(Dial::of(Binding::Input(0, InputParam::Pan)), None);
    }

    #[test]
    fn formats() {
        assert_eq!(format_value(Value::Amount(0.0)), "off");
        assert_eq!(format_value(Value::Amount(3.5)), "3.5");
        assert_eq!(format_value(Value::Boost(0.0)), "0 dB");
        assert_eq!(format_value(Value::Boost(-4.5)), "-4.5 dB");
        assert_eq!(format_value(Value::Boost(6.0)), "+6.0 dB");
        assert_eq!(format_value(Value::Freq(500.0)), "500");
        assert_eq!(format_value(Value::Freq(2500.0)), "2.5k");
        assert_eq!(format_value(Value::Width(0.7)), "0.70");
    }

    #[test]
    fn a_new_number_keeps_the_kind_of_value() {
        assert_eq!(with_number(Value::Freq(1.0), 2.0), Value::Freq(2.0));
        assert_eq!(number(with_number(Value::Boost(1.0), 2.0)), Some(2.0));
        assert_eq!(number(Value::Switch(true)), None);
    }
}
