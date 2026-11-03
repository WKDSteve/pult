//! Channel strips: an input strip and a bus (master) strip, built from the controls.
//!
//! Every strip has the same zones from top to bottom, so faders and meters of all strips line
//! up: name, pan (or device on a bus), bus assignment, mute/solo, fader with meter, readout.

use eframe::egui::{
    Align, Align2, Color32, CornerRadius, Label, Layout, Rect, RichText, Stroke, StrokeKind, Ui,
    UiBuilder, pos2, vec2,
};

use super::fader::fader;
use super::knob::{format_pan, knob};
use super::meter::{MeterState, meter, meter_scale};
use super::scale::format_db;
use super::toggle::{Role, toggle};
use crate::theme::Theme;
use pult_core::view::{Action, BusView, GAIN_RANGE_DB, InputView, SourceKind};

const HEADER_HEIGHT: f32 = 38.0;
const PAN_HEIGHT: f32 = 58.0;
const ASSIGN_ROW_HEIGHT: f32 = 22.0;
const ASSIGN_PER_ROW: usize = 3;
const MUTE_SOLO_HEIGHT: f32 = 30.0;
const READOUT_HEIGHT: f32 = 42.0;
/// Below this the fader is too short to use; the strip scrolls instead.
pub const MIN_FADER_HEIGHT: f32 = 160.0;

/// Zones of one strip, shared by all strips of a console.
#[derive(Clone, Copy, Debug)]
pub struct Zones {
    header: Rect,
    pan: Rect,
    assign: Rect,
    mute_solo: Rect,
    fader: Rect,
    readout: Rect,
}

impl Zones {
    /// Cuts a strip `rect` into zones; `buses` sets the rows of the assignment zone.
    #[must_use]
    pub fn new(rect: Rect, buses: usize) -> Self {
        let pad = Theme::current().metrics.strip_pad;
        let inner = rect.shrink(pad);
        let rows = buses.div_ceil(ASSIGN_PER_ROW).max(1);
        #[allow(clippy::cast_precision_loss)]
        let assign_height = rows as f32 * ASSIGN_ROW_HEIGHT + 6.0;
        let fixed = HEADER_HEIGHT + PAN_HEIGHT + assign_height + MUTE_SOLO_HEIGHT + READOUT_HEIGHT;
        let fader_height = (inner.height() - fixed).max(MIN_FADER_HEIGHT);
        let mut y = inner.top();
        let mut take = |height: f32| {
            let zone = Rect::from_x_y_ranges(inner.x_range(), y..=y + height);
            y += height;
            zone
        };
        let header = take(HEADER_HEIGHT);
        let pan = take(PAN_HEIGHT);
        let assign = take(assign_height);
        let mute_solo = take(MUTE_SOLO_HEIGHT);
        let fader = take(fader_height);
        let readout = take(READOUT_HEIGHT);
        Self {
            header,
            pan,
            assign,
            mute_solo,
            fader,
            readout,
        }
    }

    /// Height a strip needs for `buses` with the shortest usable fader.
    #[must_use]
    pub fn min_height(buses: usize) -> f32 {
        let pad = Theme::current().metrics.strip_pad;
        let rows = buses.div_ceil(ASSIGN_PER_ROW).max(1);
        #[allow(clippy::cast_precision_loss)]
        let assign = rows as f32 * ASSIGN_ROW_HEIGHT + 6.0;
        2.0 * pad
            + HEADER_HEIGHT
            + PAN_HEIGHT
            + assign
            + MUTE_SOLO_HEIGHT
            + MIN_FADER_HEIGHT
            + READOUT_HEIGHT
    }
}

/// Runs `add` in a child of `ui` confined to `rect`.
fn within<R>(ui: &mut Ui, rect: Rect, layout: Layout, add: impl FnOnce(&mut Ui) -> R) -> R {
    // A child, not a scope: the strip has its rect already, nothing more is allocated in `ui`.
    add(&mut ui.new_child(UiBuilder::new().max_rect(rect).layout(layout)))
}

fn strip_frame(ui: &Ui, rect: Rect, fill: Color32, zones: &Zones) {
    let theme = Theme::current();
    let p = &theme.palette;
    let painter = ui.painter();
    let radius = CornerRadius::same(theme.metrics.radius);
    painter.rect_filled(rect, radius, fill);
    painter.rect_stroke(rect, radius, Stroke::new(1.0, p.line), StrokeKind::Inside);
    // Hairlines between the zones above the fader.
    for y in [zones.header.bottom(), zones.mute_solo.bottom()] {
        let y = y.round() + 0.5;
        painter.line_segment(
            [pos2(rect.left() + 1.0, y), pos2(rect.right() - 1.0, y)],
            (1.0, p.line),
        );
    }
}

fn header(ui: &mut Ui, zone: Rect, name: &str, detail: &str, missing: bool) {
    let p = &Theme::current().palette;
    within(ui, zone, Layout::top_down(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add(Label::new(RichText::new(name).size(12.5).color(p.text).strong()).truncate());
        let (text, color) = if missing {
            ("missing", p.mute)
        } else {
            (detail, p.text_dim)
        };
        ui.add(Label::new(RichText::new(text).size(9.5).color(color)).truncate());
    });
}

fn readout(ui: &Ui, zone: Rect, gain_db: f32, note: Option<(String, Color32)>) {
    let theme = Theme::current();
    let p = &theme.palette;
    let painter = ui.painter();
    let field = Rect::from_min_size(zone.min + vec2(0.0, 6.0), vec2(zone.width(), 18.0));
    painter.rect_filled(
        field,
        CornerRadius::same(theme.metrics.radius_small),
        p.well,
    );
    painter.text(
        field.center(),
        Align2::CENTER_CENTER,
        format!("{} dB", format_db(gain_db)),
        Theme::value_font(11.0),
        p.text,
    );
    if let Some((text, color)) = note {
        painter.text(
            pos2(field.center().x, field.bottom() + 9.0),
            Align2::CENTER_CENTER,
            text,
            Theme::value_font(9.0),
            color,
        );
    }
}

/// The fader zone: fader on the left, meter on the right, optionally with meter numbers.
/// The meter comes as its state, the level of this frame and the time.
fn fader_and_meter(
    ui: &mut Ui,
    zone: Rect,
    gain_db: &mut f32,
    label: &str,
    (meter_state, level, now): (&mut MeterState, Option<f32>, f64),
    with_numbers: bool,
) -> bool {
    let m = &Theme::current().metrics;
    let numbers = if with_numbers { 12.0 } else { 0.0 };
    let meter_x = zone.right() - numbers - m.meter_width;
    let fader_rect = Rect::from_x_y_ranges(zone.left()..=meter_x - 4.0, zone.y_range());
    let meter_rect = Rect::from_x_y_ranges(meter_x..=meter_x + m.meter_width, zone.y_range());
    let changed = within(ui, fader_rect, Layout::top_down(Align::Min), |ui| {
        fader(ui, gain_db, fader_rect.size(), label).changed()
    });
    within(ui, meter_rect, Layout::top_down(Align::Min), |ui| {
        meter(ui, meter_state, level, now, meter_rect.size());
    });
    if with_numbers {
        let rect = Rect::from_x_y_ranges(meter_rect.right() + 3.0..=zone.right(), zone.y_range());
        within(ui, rect, Layout::top_down(Align::Min), |ui| {
            meter_scale(ui, rect.size());
        });
    }
    changed
}

/// The bus assignment buttons of input `index`, three to a row.
fn assign_row(
    ui: &mut Ui,
    zone: Rect,
    index: usize,
    input: &InputView,
    bus_names: &[String],
    actions: &mut Vec<Action>,
) {
    within(
        ui,
        zone.shrink2(vec2(0.0, 3.0)),
        Layout::left_to_right(Align::Min),
        |ui| {
            let gap = 3.0;
            #[allow(clippy::cast_precision_loss)]
            let per_row = ASSIGN_PER_ROW as f32;
            let width = (zone.width() - gap * (per_row - 1.0)) / per_row;
            ui.spacing_mut().item_spacing = vec2(gap, 4.0);
            ui.horizontal_wrapped(|ui| {
                for (bus, name) in bus_names.iter().enumerate() {
                    let mut on = input.buses.contains(&bus);
                    let size = vec2(width, ASSIGN_ROW_HEIGHT - 4.0);
                    if toggle(
                        ui,
                        &mut on,
                        name,
                        &format!("{} to {name}", input.name),
                        Role::Assign,
                        size,
                    )
                    .changed()
                    {
                        actions.push(Action::Route {
                            input: index,
                            bus,
                            on,
                        });
                    }
                }
            });
        },
    );
}

/// Draws the strip of input `index` into `rect`; returns what the user changed.
pub fn input_strip(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    input: &InputView,
    bus_names: &[String],
    meter_state: &mut MeterState,
    now: f64,
) -> Vec<Action> {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let zones = Zones::new(rect, bus_names.len());
    let mut actions = Vec::new();
    strip_frame(ui, rect, p.surface, &zones);

    let kind = match input.kind {
        SourceKind::Device => "DEV",
        SourceKind::Program => "APP",
    };
    let detail = format!("{kind} · {}", format_rate(input.sample_rate));
    header(ui, zones.header, &input.name, &detail, !input.present);

    within(ui, zones.pan, Layout::top_down(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        ui.add_space(4.0);
        let mut pan = input.pan;
        if knob(ui, &mut pan, m.knob_size, &input.name).changed() {
            actions.push(Action::InputPan { input: index, pan });
        }
        ui.label(
            RichText::new(format_pan(pan))
                .font(Theme::value_font(9.5))
                .color(p.text_dim),
        );
    });

    assign_row(ui, zones.assign, index, input, bus_names, &mut actions);

    within(
        ui,
        zones.mute_solo.shrink2(vec2(0.0, 4.0)),
        Layout::left_to_right(Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            let size = vec2((zones.mute_solo.width() - 4.0) / 2.0, m.button_height + 2.0);
            let mut mute = input.muted;
            if toggle(
                ui,
                &mut mute,
                "M",
                &format!("{} mute", input.name),
                Role::Mute,
                size,
            )
            .changed()
            {
                actions.push(Action::InputMute {
                    input: index,
                    muted: mute,
                });
            }
            let mut solo = input.solo;
            if toggle(
                ui,
                &mut solo,
                "S",
                &format!("{} solo", input.name),
                Role::Solo,
                size,
            )
            .changed()
            {
                actions.push(Action::InputSolo { input: index, solo });
            }
        },
    );

    let mut gain = input.gain_db;
    let fader_zone = zones.fader.shrink2(vec2(0.0, 6.0));
    if fader_and_meter(
        ui,
        fader_zone,
        &mut gain,
        &input.name,
        (meter_state, input.peak_db, now),
        false,
    ) {
        actions.push(Action::InputGain {
            input: index,
            db: action_gain(gain),
        });
    }

    let note = if input.dropouts > 0 {
        Some((format!("{} drops", input.dropouts), p.solo))
    } else if input.drift_ppm.abs() >= 1.0 {
        Some((format!("{:+.0} ppm", input.drift_ppm), p.text_faint))
    } else {
        None
    };
    readout(ui, zones.readout, gain, note);

    if !input.present {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(m.radius),
            p.base.gamma_multiply(0.55),
        );
    }
    actions
}

/// Draws the strip of bus `index` into `rect`; returns what the user changed.
pub fn bus_strip(
    ui: &mut Ui,
    rect: Rect,
    index: usize,
    bus: &BusView,
    buses: usize,
    meter_state: &mut MeterState,
    now: f64,
) -> Vec<Action> {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let zones = Zones::new(rect, buses);
    let mut actions = Vec::new();
    strip_frame(ui, rect, p.raised.gamma_multiply(0.55), &zones);

    header(
        ui,
        zones.header,
        &bus.name,
        &format_rate(bus.sample_rate),
        !bus.present,
    );

    // A bus has no pan and no assignment; its device takes that room.
    let info = zones.pan.union(zones.assign);
    within(
        ui,
        info.shrink2(vec2(0.0, 6.0)),
        Layout::top_down(Align::Center),
        |ui| {
            ui.label(RichText::new("OUTPUT").size(9.0).color(p.text_faint));
            ui.add(Label::new(RichText::new(&bus.device).size(11.0).color(p.text_dim)).wrap());
        },
    );

    within(
        ui,
        zones.mute_solo.shrink2(vec2(0.0, 4.0)),
        Layout::left_to_right(Align::Center),
        |ui| {
            let mut mute = bus.muted;
            let size = vec2(zones.mute_solo.width(), m.button_height + 2.0);
            if toggle(
                ui,
                &mut mute,
                "MUTE",
                &format!("{} mute", bus.name),
                Role::Mute,
                size,
            )
            .changed()
            {
                actions.push(Action::BusMute {
                    bus: index,
                    muted: mute,
                });
            }
        },
    );

    let mut gain = bus.gain_db;
    let fader_zone = zones.fader.shrink2(vec2(0.0, 6.0));
    if fader_and_meter(
        ui,
        fader_zone,
        &mut gain,
        &bus.name,
        (&mut *meter_state, bus.peak_db, now),
        true,
    ) {
        actions.push(Action::BusGain {
            bus: index,
            db: action_gain(gain),
        });
    }
    let note = clip_note(meter_state).map(|text| (text, p.meter_high));
    readout(ui, zones.readout, gain, note);
    actions
}

/// The note under a bus fader: `CLIP` while its clip lamp is lit. The lamp, not the desk's
/// counter, which only grows, so a click on the meter clears both.
#[must_use]
pub fn clip_note(meter: &MeterState) -> Option<String> {
    meter.clipped().then(|| "CLIP".to_owned())
}

/// The gain an action asks for: the fader's off (-inf) becomes the lowest gain the mixer takes.
#[must_use]
pub fn action_gain(db: f32) -> f32 {
    db.clamp(GAIN_RANGE_DB.0, GAIN_RANGE_DB.1)
}

/// A sample rate as a strip prints it: `48k`, `44.1k`.
#[must_use]
pub fn format_rate(rate: u32) -> String {
    if rate.is_multiple_of(1000) {
        format!("{}k", rate / 1000)
    } else {
        format!("{:.1}k", f64::from(rate) / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn off_is_sent_as_the_lowest_gain() {
        assert_eq!(action_gain(f32::NEG_INFINITY), GAIN_RANGE_DB.0);
        assert_eq!(action_gain(-6.5), -6.5);
        assert_eq!(action_gain(40.0), GAIN_RANGE_DB.1);
    }

    #[test]
    fn clip_note_follows_the_lamp() {
        let mut meter = MeterState::default();
        assert_eq!(clip_note(&meter), None);
        meter.mark_clipped();
        assert_eq!(clip_note(&meter).as_deref(), Some("CLIP"));
        meter.reset_clip();
        assert_eq!(clip_note(&meter), None);
    }

    #[test]
    fn rates() {
        assert_eq!(format_rate(48_000), "48k");
        assert_eq!(format_rate(44_100), "44.1k");
        assert_eq!(format_rate(96_000), "96k");
    }

    #[test]
    fn zones_stack_and_keep_a_usable_fader() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(88.0, 600.0));
        let zones = Zones::new(rect, 3);
        assert!(zones.header.bottom() <= zones.pan.top());
        assert!(zones.mute_solo.bottom() <= zones.fader.top());
        assert!(
            (zones.readout.bottom() - (600.0 - Theme::current().metrics.strip_pad)).abs() < 1e-3
        );
        let short = Zones::new(Rect::from_min_size(pos2(0.0, 0.0), vec2(88.0, 100.0)), 3);
        assert!(short.fader.height() >= MIN_FADER_HEIGHT);
        assert!(Zones::min_height(4) > Zones::min_height(3));
    }
}
