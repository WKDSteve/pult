//! Channel strips: draws a [`Strip`] of the layout, an input strip or a bus (master) strip.
//!
//! Every strip has the same zones from top to bottom, so faders and meters of all strips line
//! up: name, pan (or device on a bus), bus assignment, gate and compressor (or the switch of the
//! equalizer on a bus), mute/solo, fader with meter, readout.
//! The elements of a strip say which control goes into which zone and what it is bound to.

use eframe::egui::{
    Align, Align2, Color32, CornerRadius, Label, Layout, Rect, RichText, Stroke, StrokeKind, Ui,
    UiBuilder, Vec2, pos2, vec2,
};

use super::dial::labelled;
use super::fader::fader;
use super::knob::{format_pan, knob};
use super::meter::{MeterState, meter, meter_scale};
use super::scale::format_db;
use super::toggle::toggle;
use crate::binding::{Binding, Value, Values};
use crate::layout::{Control, Element, Meters, Owner, Place, Strip, StripStyle, Zone};
use crate::theme::Theme;
use pult_core::view::{Action, GAIN_RANGE_DB, MixerView, SourceKind};

const HEADER_HEIGHT: f32 = 38.0;
const PAN_HEIGHT: f32 = 58.0;
const ASSIGN_ROW_HEIGHT: f32 = 22.0;
const ASSIGN_PER_ROW: usize = 3;
const DYNAMICS_HEIGHT: f32 = 64.0;
const DIAL_SIZE: f32 = 26.0;
const MUTE_SOLO_HEIGHT: f32 = 30.0;
const READOUT_HEIGHT: f32 = 42.0;
/// Below this the fader is too short to use. The strip does not scroll up and down: a window too
/// small even for this cuts it off at the bottom.
pub const MIN_FADER_HEIGHT: f32 = 100.0;

/// Zones of one strip, shared by all strips of a console.
#[derive(Clone, Copy, Debug)]
pub struct Zones {
    header: Rect,
    pan: Rect,
    assign: Rect,
    dynamics: Rect,
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
        let fixed = HEADER_HEIGHT
            + PAN_HEIGHT
            + assign_height
            + DYNAMICS_HEIGHT
            + MUTE_SOLO_HEIGHT
            + READOUT_HEIGHT;
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
        let dynamics = take(DYNAMICS_HEIGHT);
        let mute_solo = take(MUTE_SOLO_HEIGHT);
        let fader = take(fader_height);
        let readout = take(READOUT_HEIGHT);
        Self {
            header,
            pan,
            assign,
            dynamics,
            mute_solo,
            fader,
            readout,
        }
    }

    /// Height the console needs for strips over `buses` with the shortest usable fader, with the
    /// gaps above and below them.
    #[must_use]
    pub fn min_height(buses: usize) -> f32 {
        let pad = Theme::current().metrics.strip_pad;
        let gap = Theme::current().metrics.strip_gap;
        let rows = buses.div_ceil(ASSIGN_PER_ROW).max(1);
        #[allow(clippy::cast_precision_loss)]
        let assign = rows as f32 * ASSIGN_ROW_HEIGHT + 6.0;
        2.0 * gap
            + 2.0 * pad
            + HEADER_HEIGHT
            + PAN_HEIGHT
            + assign
            + DYNAMICS_HEIGHT
            + MUTE_SOLO_HEIGHT
            + MIN_FADER_HEIGHT
            + READOUT_HEIGHT
    }

    /// The rect of `zone`.
    #[must_use]
    pub fn rect(&self, zone: Zone) -> Rect {
        match zone {
            Zone::Header => self.header,
            Zone::Pan => self.pan,
            Zone::Assign => self.assign,
            Zone::Dynamics => self.dynamics,
            Zone::MuteSolo => self.mute_solo,
            Zone::Fader => self.fader,
            Zone::Readout => self.readout,
            Zone::Info => self.pan.union(self.assign),
        }
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

/// The owner's `name` over its detail; hovering it tells the `full` name behind a label.
fn header(ui: &mut Ui, zone: Rect, name: &str, full: &str, detail: &str, missing: bool) {
    let p = &Theme::current().palette;
    within(ui, zone, Layout::top_down(Align::Center), |ui| {
        ui.spacing_mut().item_spacing.y = 2.0;
        let shown =
            ui.add(Label::new(RichText::new(name).size(12.5).color(p.text).strong()).truncate());
        if name != full {
            shown.on_hover_text(full);
        }
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

/// Whether the device or program behind `owner` is there; an owner the view lacks is not.
fn present(view: &MixerView, owner: Owner) -> bool {
    match owner {
        Owner::Input(index) => view.inputs.get(index).is_some_and(|i| i.present),
        Owner::Bus(index) => view.buses.get(index).is_some_and(|b| b.present),
    }
}

/// Draws `strip` into `rect` with the values of this frame; returns what the user changed.
pub fn show(
    ui: &mut Ui,
    rect: Rect,
    strip: &Strip,
    values: &mut Values<'_>,
    meters: &mut Meters,
    now: f64,
) -> Vec<Action> {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let view = values.view();
    let zones = Zones::new(rect, view.buses.len());
    let fill = match strip.style {
        StripStyle::Input => p.surface,
        StripStyle::Bus => p.raised.gamma_multiply(theme.shades.bus_body),
    };
    strip_frame(ui, rect, fill, &zones);

    let mut draw = Draw {
        values,
        meters,
        now,
        owner: strip.owner,
        actions: Vec::new(),
    };
    // Neighbours in one zone share its layout, as the buttons of a row do.
    for parts in strip.parts.chunk_by(|a, b| a.place == b.place) {
        if let Place::Zone(zone) = parts[0].place {
            draw.zone(ui, zones.rect(zone), zone, parts);
        }
    }

    if strip.style == StripStyle::Input && !present(view, strip.owner) {
        ui.painter().rect_filled(
            rect,
            CornerRadius::same(m.radius),
            p.base.gamma_multiply(theme.shades.veil),
        );
    }
    draw.actions
}

/// What drawing the elements of one strip needs.
struct Draw<'a, 'v> {
    values: &'a mut Values<'v>,
    meters: &'a mut Meters,
    now: f64,
    owner: Owner,
    actions: Vec<Action>,
}

impl<'v> Draw<'_, 'v> {
    fn view(&self) -> &'v MixerView {
        self.values.view()
    }

    fn write(&mut self, binding: Binding, value: Value) {
        self.actions.extend(self.values.write(binding, value));
    }

    /// Draws `parts`, all in `zone` at `rect`.
    fn zone(&mut self, ui: &mut Ui, rect: Rect, zone: Zone, parts: &[Element]) {
        let m = &Theme::current().metrics;
        match zone {
            Zone::Header => self.header(ui, rect),
            Zone::Pan => within(ui, rect, Layout::top_down(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.add_space(4.0);
                for part in parts {
                    self.pan(ui, part);
                }
            }),
            Zone::Assign => within(
                ui,
                rect.shrink2(vec2(0.0, 3.0)),
                Layout::left_to_right(Align::Min),
                |ui| {
                    let gap = 3.0;
                    #[allow(clippy::cast_precision_loss)]
                    let per_row = ASSIGN_PER_ROW as f32;
                    let width = (rect.width() - gap * (per_row - 1.0)) / per_row;
                    ui.spacing_mut().item_spacing = vec2(gap, 4.0);
                    ui.horizontal_wrapped(|ui| {
                        for part in parts {
                            self.toggle(ui, part, vec2(width, ASSIGN_ROW_HEIGHT - 4.0));
                        }
                    });
                },
            ),
            Zone::Dynamics => within(
                ui,
                rect.shrink2(vec2(0.0, 2.0)),
                Layout::left_to_right(Align::Center),
                |ui| {
                    // Dials stand apart by their own width; buttons need a gap.
                    let buttons = parts
                        .iter()
                        .any(|part| matches!(part.control, Control::Toggle { .. }));
                    let gap = if buttons { 4.0 } else { 0.0 };
                    ui.spacing_mut().item_spacing.x = gap;
                    #[allow(clippy::cast_precision_loss)]
                    let count = parts.len() as f32;
                    let width = (rect.width() - gap * (count - 1.0)) / count;
                    for part in parts {
                        match (&part.control, part.bind) {
                            (Control::Dial { text }, Some(binding)) => {
                                let action =
                                    labelled(ui, self.values, binding, text, DIAL_SIZE, width);
                                self.actions.extend(action);
                            }
                            _ => self.toggle(ui, part, vec2(width, m.button_height + 2.0)),
                        }
                    }
                },
            ),
            Zone::MuteSolo => within(
                ui,
                rect.shrink2(vec2(0.0, 4.0)),
                Layout::left_to_right(Align::Center),
                |ui| {
                    // The buttons of the row share its width.
                    let gap = 4.0;
                    ui.spacing_mut().item_spacing.x = gap;
                    #[allow(clippy::cast_precision_loss)]
                    let count = parts.len() as f32;
                    let width = (rect.width() - gap * (count - 1.0)) / count;
                    for part in parts {
                        self.toggle(ui, part, vec2(width, m.button_height + 2.0));
                    }
                },
            ),
            Zone::Fader => self.fader_zone(ui, rect.shrink2(vec2(0.0, 6.0)), parts),
            Zone::Readout => {
                for part in parts {
                    self.readout(ui, rect, part);
                }
            }
            Zone::Info => self.device(ui, rect.shrink2(vec2(0.0, 6.0))),
        }
    }

    fn header(&self, ui: &mut Ui, rect: Rect) {
        let view = self.view();
        match self.owner {
            Owner::Input(index) => {
                let Some(input) = view.inputs.get(index) else {
                    return;
                };
                let kind = match input.kind {
                    SourceKind::Device => "DEV",
                    SourceKind::Program => "APP",
                };
                let detail = format!("{kind} · {}", format_rate(input.sample_rate));
                header(
                    ui,
                    rect,
                    input.shown_name(),
                    &input.name,
                    &detail,
                    !input.present,
                );
            }
            Owner::Bus(index) => {
                let Some(bus) = view.buses.get(index) else {
                    return;
                };
                header(
                    ui,
                    rect,
                    bus.shown_name(),
                    &bus.name,
                    &format_rate(bus.sample_rate),
                    !bus.present,
                );
            }
        }
    }

    /// A bus has no pan and no assignment; its device takes that room.
    fn device(&self, ui: &mut Ui, rect: Rect) {
        let p = &Theme::current().palette;
        let Owner::Bus(index) = self.owner else {
            return;
        };
        let Some(bus) = self.view().buses.get(index) else {
            return;
        };
        within(ui, rect, Layout::top_down(Align::Center), |ui| {
            ui.label(RichText::new("OUTPUT").size(9.0).color(p.text_faint));
            ui.add(Label::new(RichText::new(&bus.device).size(11.0).color(p.text_dim)).wrap());
        });
    }

    /// A pan knob, or the pan as text under it.
    fn pan(&mut self, ui: &mut Ui, part: &Element) {
        let theme = Theme::current();
        let Some(binding) = part.bind else {
            return;
        };
        let Some(Value::Pan(mut pan)) = self.values.read(binding) else {
            return;
        };
        match part.control {
            Control::Knob => {
                let label = binding.title(self.view());
                if knob(ui, &mut pan, theme.metrics.knob_size, &label).changed() {
                    self.write(binding, Value::Pan(pan));
                }
            }
            Control::PanText => {
                ui.label(
                    RichText::new(format_pan(pan))
                        .font(Theme::value_font(9.5))
                        .color(theme.palette.text_dim),
                );
            }
            _ => {}
        }
    }

    fn toggle(&mut self, ui: &mut Ui, part: &Element, size: Vec2) {
        let (Control::Toggle { text, role }, Some(binding)) = (&part.control, part.bind) else {
            return;
        };
        let Some(Value::Switch(mut on)) = self.values.read(binding) else {
            return;
        };
        let label = binding.title(self.view());
        if toggle(ui, &mut on, text, &label, *role, size).changed() {
            self.write(binding, Value::Switch(on));
        }
    }

    /// The fader zone: fader on the left, meter on the right, and the meter numbers beside it
    /// when the zone has them.
    fn fader_zone(&mut self, ui: &mut Ui, zone: Rect, parts: &[Element]) {
        let m = &Theme::current().metrics;
        let with_numbers = parts.iter().any(|part| part.control == Control::MeterScale);
        let numbers = if with_numbers { 12.0 } else { 0.0 };
        let meter_x = zone.right() - numbers - m.meter_width;
        let fader_rect = Rect::from_x_y_ranges(zone.left()..=meter_x - 4.0, zone.y_range());
        let meter_rect = Rect::from_x_y_ranges(meter_x..=meter_x + m.meter_width, zone.y_range());
        let scale_rect =
            Rect::from_x_y_ranges(meter_rect.right() + 3.0..=zone.right(), zone.y_range());
        for part in parts {
            match (&part.control, part.bind) {
                (Control::Fader, Some(binding)) => {
                    let Some(Value::Gain(mut gain)) = self.values.read(binding) else {
                        continue;
                    };
                    let label = binding.title(self.view());
                    let changed = within(ui, fader_rect, Layout::top_down(Align::Min), |ui| {
                        fader(ui, &mut gain, fader_rect.size(), &label).changed()
                    });
                    if changed {
                        self.write(binding, Value::Gain(gain));
                    }
                }
                (Control::Meter, Some(binding)) => {
                    let Some(Value::Peak(level)) = self.values.read(binding) else {
                        continue;
                    };
                    let now = self.now;
                    let Some(state) = self.meters.get_mut(binding) else {
                        continue;
                    };
                    within(ui, meter_rect, Layout::top_down(Align::Min), |ui| {
                        meter(ui, state, level, now, meter_rect.size());
                    });
                }
                (Control::MeterScale, _) => {
                    within(ui, scale_rect, Layout::top_down(Align::Min), |ui| {
                        meter_scale(ui, scale_rect.size());
                    });
                }
                _ => {}
            }
        }
    }

    /// The gain as text, with a note on the owner under it: dropouts or drift of an input, the
    /// clip lamp or the limiter of a bus.
    fn readout(&mut self, ui: &Ui, zone: Rect, part: &Element) {
        let p = &Theme::current().palette;
        let (Control::Readout, Some(binding)) = (&part.control, part.bind) else {
            return;
        };
        let Some(Value::Gain(gain)) = self.values.read(binding) else {
            return;
        };
        let note = match self.owner {
            Owner::Input(index) => self.view().inputs.get(index).and_then(|input| {
                if input.dropouts > 0 {
                    Some((format!("{} drops", input.dropouts), p.solo))
                } else if input.drift_ppm.abs() >= 1.0 {
                    Some((format!("{:+.0} ppm", input.drift_ppm), p.text_faint))
                } else {
                    None
                }
            }),
            Owner::Bus(index) => {
                let limited_db = self
                    .view()
                    .buses
                    .get(index)
                    .map_or(0.0, |bus| bus.limited_db);
                let now = self.now;
                self.meters.buses_mut().get_mut(index).and_then(|meter| {
                    let limit = meter.held_limit(limited_db, now);
                    clip_note(meter)
                        .map(|text| (text, p.meter_high))
                        .or_else(|| limit_note(limit).map(|text| (text, p.text_faint)))
                })
            }
        };
        readout(ui, zone, shown_gain(gain), note);
    }
}

/// The note under a bus fader: `CLIP` while its clip lamp is lit. The lamp, not the desk's
/// counter, which only grows, so a click on the meter clears both.
#[must_use]
pub fn clip_note(meter: &MeterState) -> Option<String> {
    meter.clipped().then(|| "CLIP".to_owned())
}

/// The note under a bus fader while its limiter turns down: `LIM -3.2`.
#[must_use]
pub fn limit_note(limited_db: f32) -> Option<String> {
    (limited_db >= 0.05).then(|| format!("LIM {:.1}", -limited_db))
}

/// The gain a readout shows: the desk's lowest gain, where an off fader lands, is off.
#[must_use]
pub fn shown_gain(db: f32) -> f32 {
    if db <= GAIN_RANGE_DB.0 {
        f32::NEG_INFINITY
    } else {
        db
    }
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
    fn clip_note_follows_the_lamp() {
        let mut meter = MeterState::default();
        assert_eq!(clip_note(&meter), None);
        meter.mark_clipped();
        assert_eq!(clip_note(&meter).as_deref(), Some("CLIP"));
        meter.reset_clip();
        assert_eq!(clip_note(&meter), None);
    }

    #[test]
    fn a_limit_note_only_while_it_turns_down() {
        assert_eq!(limit_note(0.0), None);
        assert_eq!(limit_note(0.01), None);
        assert_eq!(limit_note(3.24).as_deref(), Some("LIM -3.2"));
    }

    #[test]
    fn the_lowest_gain_reads_as_off() {
        assert_eq!(shown_gain(GAIN_RANGE_DB.0), f32::NEG_INFINITY);
        assert_eq!(shown_gain(f32::NEG_INFINITY), f32::NEG_INFINITY);
        assert_eq!(shown_gain(-70.0), -70.0);
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
        // A strip of the height the console gives it holds all its zones.
        let gap = Theme::current().metrics.strip_gap;
        let height = Zones::min_height(4) - 2.0 * gap;
        let least = Rect::from_min_size(pos2(0.0, 0.0), vec2(88.0, height));
        let zones = Zones::new(least, 4);
        assert!((zones.fader.height() - MIN_FADER_HEIGHT).abs() < 1e-3);
        assert!(zones.readout.bottom() <= least.bottom());
        assert_eq!(zones.rect(Zone::Info), zones.pan.union(zones.assign));
    }
}
