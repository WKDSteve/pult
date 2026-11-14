//! The equalizer view: a row per bus with its switch and its bands, each band with its kind, gain,
//! frequency and width, and behind them its limiter with ceiling and lookahead and its delay; under the bands the
//! curve of all of them together. A strip has no room for that; it only has the switches.
//! Under the curve of each a line DYNAMIC: a threshold per band, which makes it dynamic, and how far
//! in those bands are now; the curve then shows what the bands do at this moment, and faintly
//! what they do at most. Below
//! the buses a row per input: its equalizer, the switch that puts it before gate and compressor,
//! its limiter, and under its curve its sidechain (what its gate and compressor listen to:
//! another input, or itself through filters), its de-esser (the level above which the band of
//! its sibilants goes down, where that band begins, and how much it takes now) and its ducker:
//! the input that turns it down, how far and how fast, and how much it takes away now.

use eframe::egui::{
    Align, Align2, CornerRadius, FontId, Frame, Layout, Margin, Pos2, RichText, ScrollArea, Sense,
    Stroke, Ui, pos2, vec2,
};
use pult_core::Band;
use pult_core::deess::DeessParam;
use pult_core::duck::{DuckChange, DuckParam};
use pult_core::multiband::MultibandParam;
use pult_core::response_db;
use pult_core::sidechain::{SidechainChange, SidechainParam};
use pult_core::view::{Action, BandKind, BusView, EQ_BANDS, InputView, MixerView};

use crate::binding::{BandParam, Binding, BusParam, InputParam, Value, Values};
use crate::theme::Theme;
use crate::widgets::dial::labelled;
use crate::widgets::toggle::{Role, toggle};

const BAND_WIDTH: f32 = 154.0;
const BAND_GAP: f32 = 10.0;
const DIAL_SIZE: f32 = 30.0;
const DIAL_ROW_HEIGHT: f32 = 58.0;
const KIND_HEIGHT: f32 = 20.0;
const SWITCH_SIZE: [f32; 2] = [48.0, 22.0];
const LIMIT_WIDTH: f32 = 112.0;
const CURVE_HEIGHT: f32 = 72.0;
/// Decibels at the upper and lower edge of the curve, unless the curve goes further.
const CURVE_DB: f32 = 12.0;
/// The most the curve shows up and down, in dB: the bands together can go beyond.
const CURVE_MOST_DB: f32 = 36.0;
/// Points the curve is drawn through.
const CURVE_POINTS: usize = 200;
/// The ends of the frequency axis in Hz.
const CURVE_HZ: (f32, f32) = (20.0, 20_000.0);

/// The kinds a band can have, with what their buttons say.
const KINDS: [(BandKind, &str); 3] = [
    (BandKind::LowShelf, "LOW"),
    (BandKind::Peak, "BELL"),
    (BandKind::HighShelf, "HIGH"),
];

/// The settings of a band that have a dial, with what stands over it.
const DIALS: [(BandParam, &str); 3] = [
    (BandParam::Gain, "GAIN"),
    (BandParam::Freq, "HZ"),
    (BandParam::Q, "Q"),
];

/// Whose equalizer a column belongs to.
#[derive(Clone, Copy)]
enum Owner {
    Bus(usize),
    Input(usize),
}

impl Owner {
    /// The binding of a setting of one of its bands.
    fn band(self, band: usize, param: BandParam) -> Binding {
        match self {
            Self::Bus(bus) => Binding::Bus(bus, BusParam::EqBand(band, param)),
            Self::Input(input) => Binding::Input(input, InputParam::EqBand(band, param)),
        }
    }
}

/// What a curve is drawn of.
struct Curve<'a> {
    name: &'a str,
    bands: &'a [Band],
    on: bool,
    sample_rate: u32,
    /// The gain each band has now in dB: where a dynamic band stands.
    now_db: &'a [f32],
}

/// Draws the equalizers of `view`; a change of a control asks for it.
pub fn show(ui: &mut Ui, view: &MixerView, actions: &mut Vec<Action>) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    let mut values = Values::new(view);
    ui.label(
        RichText::new("EQUALIZER  behind the sum of a bus, before its fader; LIMITER  behind the fader, the lookahead delays the bus; double click a dial for its start")
            .size(10.0)
            .color(p.text_faint),
    );
    ui.add_space(6.0);
    // A name of its own: the matrix scrolls in the same place, and would lend it its offset.
    let area = ScrollArea::both().id_salt("equalizer");
    area.auto_shrink(false).show(ui, |ui| {
        for (index, bus) in view.buses.iter().enumerate() {
            Frame::new()
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(CornerRadius::same(m.radius))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| bus_row(ui, index, bus, &mut values, actions));
            ui.add_space(8.0);
        }
        ui.add_space(4.0);
        ui.label(
            RichText::new("INPUTS  the equalizer of an input sits behind its gate and compressor, or before them (PRE); its limiter has no lookahead; SIDECHAIN is what its gate and compressor listen to; DE-ESS turns only its sibilants down; its ducker turns it down while its key sounds")
                .size(10.0)
                .color(p.text_faint),
        );
        ui.add_space(6.0);
        for (index, input) in view.inputs.iter().enumerate() {
            Frame::new()
                .fill(p.surface)
                .stroke(Stroke::new(1.0, p.line))
                .corner_radius(CornerRadius::same(m.radius))
                .inner_margin(Margin::same(12))
                .show(ui, |ui| input_row(ui, index, input, &mut values, actions));
            ui.add_space(8.0);
        }
    });
}

/// A switch of a row's head, as wide as its word needs.
fn head_switch(
    ui: &mut Ui,
    switch: Binding,
    text: &str,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    if let Some(Value::Switch(mut on)) = values.read(switch) {
        let title = switch.title(values.view());
        let size = vec2(SWITCH_SIZE[0], SWITCH_SIZE[1]);
        if toggle(ui, &mut on, text, &title, Role::Assign, size).changed() {
            actions.extend(values.write(switch, Value::Switch(on)));
        }
    }
}

/// The row of an input: the switch of its equalizer and its place, its bands, its limiter, and
/// the curve of its bands.
fn input_row(
    ui: &mut Ui,
    index: usize,
    input: &InputView,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let p = &Theme::current().palette;
    ui.horizontal(|ui| {
        head_switch(
            ui,
            Binding::Input(index, InputParam::Eq),
            "EQ",
            values,
            actions,
        );
        head_switch(
            ui,
            Binding::Input(index, InputParam::EqPre),
            "PRE",
            values,
            actions,
        );
        ui.label(
            RichText::new(input.shown_name())
                .size(12.5)
                .color(p.text)
                .strong(),
        );
    });
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = BAND_GAP;
        for band in 0..EQ_BANDS {
            ui.allocate_ui_with_layout(
                vec2(BAND_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(BAND_WIDTH);
                    band_column(ui, Owner::Input(index), band, values, actions);
                },
            );
        }
        ui.allocate_ui_with_layout(
            vec2(LIMIT_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(LIMIT_WIDTH);
                input_limit_column(ui, index, input, values, actions);
            },
        );
    });
    ui.add_space(6.0);
    curve(
        ui,
        &Curve {
            name: input.shown_name(),
            bands: &input.eq,
            on: input.eq_on,
            sample_rate: input.sample_rate,
            now_db: &input.eq_now_db,
        },
    );
    ui.add_space(6.0);
    dynamic_line(
        ui,
        Owner::Input(index),
        input.eq_on,
        &input.eq,
        &input.eq_now_db,
        values,
        actions,
    );
    ui.add_space(2.0);
    sidechain_line(ui, index, input, values, actions);
    ui.add_space(2.0);
    deess_line(ui, index, input, values, actions);
    ui.add_space(2.0);
    duck_line(ui, index, input, values, actions);
}

const DUCK_HEAD_WIDTH: f32 = 96.0;
const DUCK_KEY_WIDTH: f32 = 78.0;
const DUCK_DIAL_WIDTH: f32 = 50.0;
/// Letters of the name of an input on the button that makes it the key.
const DUCK_KEY_LETTERS: usize = 9;

/// The settings of a ducker that have a dial, with what stands over it.
const DUCK_DIALS: [(DuckParam, &str); 5] = [
    (DuckParam::Depth, "DEPTH"),
    (DuckParam::Threshold, "THR"),
    (DuckParam::Attack, "ATT"),
    (DuckParam::Hold, "HOLD"),
    (DuckParam::Release, "REL"),
];

/// What the button of a key says: the number of its input and the start of its name.
fn key_text(number: usize, name: &str) -> String {
    let mut short: String = name.chars().take(DUCK_KEY_LETTERS).collect();
    if name.chars().count() > DUCK_KEY_LETTERS {
        short.push('.');
    }
    format!("{number} {short}")
}

/// A line under the curve of an input that has a key: its head, a button per input that can be
/// the key (and one for none), and its dials.
struct KeyLine<'a> {
    /// What stands at its start, and whether that is at work now.
    head: String,
    active: bool,
    /// What stands under the head: what the key does.
    does: &'a str,
    /// The address of the key, and the input it names now (from 1, 0 for none).
    key: Binding,
    current: usize,
    /// What the button for no key says, and the word for it in its accessible name.
    none: (&'a str, &'a str),
    /// The value that makes input `number` the key.
    value: fn(usize) -> Value,
    /// Its dials, with what stands over each.
    dials: &'a [(Binding, &'a str)],
}

/// Draws `line` for input `index`.
fn key_line(
    ui: &mut Ui,
    index: usize,
    line: &KeyLine<'_>,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let p = &Theme::current().palette;
    let view = values.view();
    let color = if line.active { p.accent } else { p.text_faint };
    // Room for the dials of the line that has most, so the buttons of all lines line up.
    #[allow(clippy::cast_precision_loss)]
    let dials_width = DUCK_DIAL_WIDTH * DUCK_DIALS.len() as f32;
    // As wide as the bands and the limiter above it, so the row keeps its width.
    #[allow(clippy::cast_precision_loss)]
    let row_width = (BAND_WIDTH + BAND_GAP) * EQ_BANDS as f32 + LIMIT_WIDTH;
    let keys_width = row_width - DUCK_HEAD_WIDTH - dials_width - 2.0 * BAND_GAP;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = BAND_GAP;
        ui.allocate_ui_with_layout(
            vec2(DUCK_HEAD_WIDTH, DIAL_ROW_HEIGHT),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(DUCK_HEAD_WIDTH);
                ui.label(RichText::new(&line.head).size(9.5).color(color));
                ui.label(RichText::new(line.does).size(8.5).color(p.text_faint));
            },
        );
        ui.allocate_ui_with_layout(
            vec2(keys_width, DIAL_ROW_HEIGHT),
            Layout::left_to_right(Align::Min).with_main_wrap(true),
            |ui| {
                ui.set_width(keys_width);
                ui.spacing_mut().item_spacing = vec2(3.0, 3.0);
                let title = line.key.title(view);
                let (none_text, none_word) = line.none;
                let none =
                    std::iter::once((0, none_text.to_owned(), format!("{title} {none_word}")));
                let others = view
                    .inputs
                    .iter()
                    .enumerate()
                    .filter(|(other, _)| *other != index)
                    .map(|(other, of)| {
                        let number = other + 1;
                        (
                            number,
                            key_text(number, of.shown_name()),
                            format!("{title} {number}"),
                        )
                    });
                for (number, text, label) in none.chain(others) {
                    let mut on = line.current == number;
                    let size = vec2(DUCK_KEY_WIDTH, KIND_HEIGHT);
                    let response = toggle(ui, &mut on, &text, &label, Role::Assign, size);
                    // A click on the key it has changes nothing: one of them is always on.
                    if response.changed() && on {
                        actions.extend(values.write(line.key, (line.value)(number)));
                    }
                }
            },
        );
        ui.allocate_ui_with_layout(
            vec2(dials_width, DIAL_ROW_HEIGHT),
            Layout::left_to_right(Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (dial, text) in line.dials {
                    actions.extend(labelled(
                        ui,
                        values,
                        *dial,
                        text,
                        DIAL_SIZE,
                        DUCK_DIAL_WIDTH,
                    ));
                }
            },
        );
    });
}

/// A line under a curve that has dials only: what stands at its start and whether that is at
/// work now, what stands under it, a sentence between, and its dials with what stands over each.
struct DialLine<'a> {
    head: String,
    active: bool,
    under: &'a str,
    says: String,
    dials: &'a [(Binding, &'a str)],
}

/// Draws `line`, as wide as the lines with keys around it, its dials under theirs.
fn dial_line(ui: &mut Ui, line: &DialLine<'_>, values: &mut Values<'_>, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    let color = if line.active { p.accent } else { p.text_faint };
    #[allow(clippy::cast_precision_loss)]
    let dials_width = DUCK_DIAL_WIDTH * DUCK_DIALS.len() as f32;
    #[allow(clippy::cast_precision_loss)]
    let row_width = (BAND_WIDTH + BAND_GAP) * EQ_BANDS as f32 + LIMIT_WIDTH;
    let between = row_width - DUCK_HEAD_WIDTH - dials_width - 2.0 * BAND_GAP;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = BAND_GAP;
        ui.allocate_ui_with_layout(
            vec2(DUCK_HEAD_WIDTH, DIAL_ROW_HEIGHT),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(DUCK_HEAD_WIDTH);
                ui.label(RichText::new(&line.head).size(9.5).color(color));
                ui.label(RichText::new(line.under).size(8.5).color(p.text_faint));
            },
        );
        ui.allocate_ui_with_layout(
            vec2(between, DIAL_ROW_HEIGHT),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(between);
                ui.label(RichText::new(&line.says).size(8.5).color(p.text_faint));
            },
        );
        ui.allocate_ui_with_layout(
            vec2(dials_width, DIAL_ROW_HEIGHT),
            Layout::left_to_right(Align::Min),
            |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                for (dial, text) in line.dials {
                    actions.extend(labelled(
                        ui,
                        values,
                        *dial,
                        text,
                        DIAL_SIZE,
                        DUCK_DIAL_WIDTH,
                    ));
                }
            },
        );
    });
}

/// The de-esser of an input: how much it takes from the band of the sibilants now, and its
/// threshold and its corner.
fn deess_line(
    ui: &mut Ui,
    index: usize,
    input: &InputView,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let taken = if input.deess.is_on() {
        input.deess_reduction_db
    } else {
        0.0
    };
    let active = taken >= 0.05;
    let at = |param| Binding::Input(index, InputParam::Deess(param));
    let line = DialLine {
        head: if active {
            format!("DE-ESS  -{taken:.1} dB")
        } else {
            "DE-ESS".to_owned()
        },
        active,
        under: "sibilants",
        says: "what is above HZ goes down while it is louder than THR".to_owned(),
        dials: &[
            (at(DeessParam::Threshold), "THR"),
            (at(DeessParam::Freq), "HZ"),
        ],
    };
    dial_line(ui, &line, values, actions);
}

/// What the dynamic bands among `bands` do now, as the line DYNAMIC says it: `band 2 -4.5 of
/// -12.0 dB`, one after the other; `None` when no band has a threshold.
#[must_use]
pub fn dynamic_now(bands: &[Band], now_db: &[f32]) -> Option<String> {
    let said: Vec<String> = bands
        .iter()
        .enumerate()
        .filter(|(_, band)| band.is_dynamic())
        .map(|(index, band)| {
            let now = now_db.get(index).copied().unwrap_or(0.0);
            format!("band {} {now:+.1} of {:+.1} dB", index + 1, band.gain_db)
        })
        .collect();
    (!said.is_empty()).then(|| said.join("   "))
}

/// The thresholds of the bands of an equalizer, one dial per band: a band with one is dynamic,
/// and the line says how far in each of those is now.
fn dynamic_line(
    ui: &mut Ui,
    owner: Owner,
    on: bool,
    bands: &[Band],
    now_db: &[f32],
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let now = dynamic_now(bands, now_db).filter(|_| on);
    let active = bands
        .iter()
        .zip(now_db)
        .any(|(band, now)| on && band.is_dynamic() && now.abs() >= 0.05);
    let dials: [(Binding, &str); EQ_BANDS] = std::array::from_fn(|band| {
        (
            owner.band(band, BandParam::Threshold),
            ["THR 1", "THR 2", "THR 3", "THR 4"][band],
        )
    });
    let line = DialLine {
        head: "DYNAMIC".to_owned(),
        active,
        under: "bands above THR",
        says: now.unwrap_or_else(|| {
            "a band with a threshold has its gain only while its own range is louder".to_owned()
        }),
        dials: &dials,
    };
    dial_line(ui, &line, values, actions);
}

/// The sidechain of an input: a button per input its gate and compressor can listen to (and one
/// for the input itself), and the filters before their detectors.
fn sidechain_line(
    ui: &mut Ui,
    index: usize,
    input: &InputView,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let at = |param| Binding::Input(index, InputParam::Sidechain(param));
    let key = at(SidechainParam::Key);
    let Some(Value::Sidechain(SidechainChange::Key(current))) = values.read(key) else {
        return;
    };
    let line = KeyLine {
        head: "SIDECHAIN".to_owned(),
        active: !input.sidechain.is_neutral(),
        does: "gate, comp hear",
        key,
        current,
        none: ("SELF", "self"),
        value: |number| Value::Sidechain(SidechainChange::Key(number)),
        dials: &[
            (at(SidechainParam::Hpf), "HPF"),
            (at(SidechainParam::Lpf), "LPF"),
        ],
    };
    key_line(ui, index, &line, values, actions);
}

/// The ducker of an input: how much it takes away now, a button per input that can be its key
/// (and one for none), and its depth, its threshold and its times.
fn duck_line(
    ui: &mut Ui,
    index: usize,
    input: &InputView,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let key = Binding::Input(index, InputParam::Duck(DuckParam::Key));
    let Some(Value::Duck(DuckChange::Key(current))) = values.read(key) else {
        return;
    };
    let taken = if input.duck.is_on() {
        input.duck_reduction_db
    } else {
        0.0
    };
    let active = taken >= 0.05;
    let head = if active {
        format!("DUCK  -{taken:.1} dB")
    } else {
        "DUCK".to_owned()
    };
    let dials =
        DUCK_DIALS.map(|(param, text)| (Binding::Input(index, InputParam::Duck(param)), text));
    let line = KeyLine {
        head,
        active,
        does: "ducked by",
        key,
        current,
        none: ("NONE", "none"),
        value: |number| Value::Duck(DuckChange::Key(number)),
        dials: &dials,
    };
    key_line(ui, index, &line, values, actions);
}

/// The limiter of an input: its switch, its ceiling, and how far it turns down now.
fn input_limit_column(
    ui: &mut Ui,
    index: usize,
    input: &InputView,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let p = &Theme::current().palette;
    let taken = if input.limit_on {
        input.limited_db
    } else {
        0.0
    };
    let (head, color) = if taken >= 0.05 {
        (format!("LIMITER  -{taken:.1} dB"), p.accent)
    } else {
        ("LIMITER".to_owned(), p.text_faint)
    };
    ui.label(RichText::new(head).size(9.5).color(color));
    let switch = Binding::Input(index, InputParam::Limit);
    if let Some(Value::Switch(mut on)) = values.read(switch) {
        let title = switch.title(values.view());
        let size = vec2(LIMIT_WIDTH, KIND_HEIGHT);
        if toggle(ui, &mut on, "LIM", &title, Role::Assign, size).changed() {
            actions.extend(values.write(switch, Value::Switch(on)));
        }
    }
    ui.add_space(4.0);
    let dial = Binding::Input(index, InputParam::LimitCeiling);
    actions.extend(labelled(ui, values, dial, "CEIL", DIAL_SIZE, LIMIT_WIDTH));
}

fn bus_row(
    ui: &mut Ui,
    index: usize,
    bus: &BusView,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let p = &Theme::current().palette;
    ui.horizontal(|ui| {
        let switch = Binding::Bus(index, BusParam::Eq);
        if let Some(Value::Switch(mut on)) = values.read(switch) {
            let title = switch.title(values.view());
            let size = vec2(SWITCH_SIZE[0], SWITCH_SIZE[1]);
            if toggle(ui, &mut on, "EQ", &title, Role::Assign, size).changed() {
                actions.extend(values.write(switch, Value::Switch(on)));
            }
        }
        ui.label(
            RichText::new(bus.shown_name())
                .size(12.5)
                .color(p.text)
                .strong(),
        );
        ui.label(RichText::new(&bus.device).size(10.0).color(p.text_dim));
    });
    ui.add_space(6.0);
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = BAND_GAP;
        for band in 0..EQ_BANDS {
            ui.allocate_ui_with_layout(
                vec2(BAND_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
                Layout::top_down(Align::Min),
                |ui| {
                    ui.set_width(BAND_WIDTH);
                    band_column(ui, Owner::Bus(index), band, values, actions);
                },
            );
        }
        ui.allocate_ui_with_layout(
            vec2(COMP_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(COMP_WIDTH);
                comp_column(ui, index, values, actions);
            },
        );
        ui.allocate_ui_with_layout(
            vec2(MULTIBAND_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(MULTIBAND_WIDTH);
                multiband_column(ui, index, values, actions);
            },
        );
        ui.allocate_ui_with_layout(
            vec2(LIMIT_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(LIMIT_WIDTH);
                limit_column(ui, index, values, actions);
            },
        );
        ui.allocate_ui_with_layout(
            vec2(DELAY_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(DELAY_WIDTH);
                delay_column(ui, index, values, actions);
            },
        );
    });
    ui.add_space(6.0);
    curve(
        ui,
        &Curve {
            name: bus.shown_name(),
            bands: &bus.eq,
            on: bus.eq_on,
            sample_rate: bus.sample_rate,
            now_db: &bus.eq_now_db,
        },
    );
    ui.add_space(6.0);
    dynamic_line(
        ui,
        Owner::Bus(index),
        bus.eq_on,
        &bus.eq,
        &bus.eq_now_db,
        values,
        actions,
    );
}

/// The frequency response of a bus's bands together, under the bands: frequency on a logarithmic
/// axis from 20 Hz to 20 kHz, 0 dB marked, faint while the equalizer is switched off (the bands
/// keep their settings, the sound passes flat).
fn curve(ui: &mut Ui, of: &Curve<'_>) {
    let theme = Theme::current();
    let (p, m) = (&theme.palette, &theme.metrics);
    #[allow(clippy::cast_precision_loss)]
    let width = BAND_WIDTH * EQ_BANDS as f32 + BAND_GAP * (EQ_BANDS - 1) as f32;
    let (rect, response) = ui.allocate_exact_size(vec2(width, CURVE_HEIGHT), Sense::hover());
    // With a dynamic band there are two curves: what the bands do now, and at most.
    let moving = of.on && of.bands.iter().any(Band::is_dynamic);
    response.on_hover_text(if moving {
        format!(
            "{}: what the bands do together now, 20 Hz to 20 kHz; faint, what they do at most",
            of.name
        )
    } else {
        format!("{}: what the bands do together, 20 Hz to 20 kHz", of.name)
    });
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, CornerRadius::same(m.radius), p.well);

    let (low, high) = CURVE_HZ;
    // Above half the rate there is no sound to shape.
    #[allow(clippy::cast_precision_loss)]
    let top_hz = high.min(of.sample_rate as f32 * 0.5);
    let span = (high / low).ln();
    let x_of = |hz: f32| rect.left() + rect.width() * (hz / low).ln() / span;
    #[allow(clippy::cast_precision_loss)]
    let points: Vec<(f32, f32)> = (0..CURVE_POINTS)
        .map(|i| {
            let hz = low * (top_hz / low).powf(i as f32 / (CURVE_POINTS - 1) as f32);
            (hz, response_db(of.bands, of.sample_rate, hz))
        })
        .collect();
    let furthest = points
        .iter()
        .fold(0.0_f32, |most, (_, db)| most.max(db.abs()));
    // In steps of 6 dB, so the scale does not twitch while a gain is turned.
    let range = (furthest / 6.0)
        .ceil()
        .mul_add(6.0, 0.0)
        .clamp(CURVE_DB, CURVE_MOST_DB);
    let middle = rect.center().y;
    let reach = rect.height() / 2.0 - 4.0;
    let y_of = |db: f32| middle - db.clamp(-range, range) / range * reach;

    let faint = Stroke::new(1.0, p.line);
    for hz in [100.0, 1000.0, 10_000.0] {
        let x = x_of(hz);
        painter.line_segment([pos2(x, rect.top()), pos2(x, rect.bottom())], faint);
    }
    painter.line_segment(
        [pos2(rect.left(), middle), pos2(rect.right(), middle)],
        Stroke::new(1.0, p.line_strong),
    );
    let small = FontId::proportional(9.0);
    for (hz, text) in [(100.0, "100"), (1000.0, "1k"), (10_000.0, "10k")] {
        painter.text(
            pos2(x_of(hz) + 3.0, rect.bottom() - 2.0),
            Align2::LEFT_BOTTOM,
            text,
            small.clone(),
            p.text_faint,
        );
    }
    painter.text(
        pos2(rect.left() + 4.0, rect.top() + 2.0),
        Align2::LEFT_TOP,
        format!("+{range:.0} dB"),
        small.clone(),
        p.text_faint,
    );
    painter.text(
        pos2(rect.right() - 4.0, middle + 1.0),
        Align2::RIGHT_TOP,
        "0 dB",
        small,
        p.text_faint,
    );

    let line: Vec<Pos2> = points
        .iter()
        .map(|&(hz, db)| pos2(x_of(hz), y_of(db)))
        .collect();
    if moving {
        painter.line(line, Stroke::new(1.0, p.text_faint));
        let now = bands_now(of.bands, of.now_db);
        let line: Vec<Pos2> = points
            .iter()
            .map(|&(hz, _)| pos2(x_of(hz), y_of(response_db(&now, of.sample_rate, hz))))
            .collect();
        painter.line(line, Stroke::new(1.5, p.accent));
    } else {
        let color = if of.on { p.accent } else { p.text_faint };
        painter.line(line, Stroke::new(1.5, color));
    }
}

/// `bands` as they stand at this moment: a dynamic band with the gain it has now, the others
/// as they are set.
#[must_use]
pub fn bands_now(bands: &[Band], now_db: &[f32]) -> Vec<Band> {
    bands
        .iter()
        .enumerate()
        .map(|(index, band)| match now_db.get(index) {
            Some(now) if band.is_dynamic() => band.with_gain(*now),
            _ => *band,
        })
        .collect()
}

/// Width of the column of the bus compressor: three dials.
const COMP_WIDTH: f32 = 150.0;

/// The compressor of a bus, before its fader and limiter: its knob, the mix of dry and
/// compressed, the sidechain filter, and what it takes away now.
fn comp_column(ui: &mut Ui, bus: usize, values: &mut Values<'_>, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    let taken = values.view().buses.get(bus).map_or(0.0, |b| {
        if b.comp > 0.0 {
            b.comp_reduction_db
        } else {
            0.0
        }
    });
    let head = if taken >= 0.05 {
        format!("COMPRESSOR  -{taken:.1} dB")
    } else {
        "COMPRESSOR".to_owned()
    };
    let color = if taken >= 0.05 {
        p.accent
    } else {
        p.text_faint
    };
    ui.label(RichText::new(head).size(9.5).color(color));
    ui.add_space(4.0 + KIND_HEIGHT);
    ui.allocate_ui_with_layout(
        vec2(COMP_WIDTH, DIAL_ROW_HEIGHT),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let width = COMP_WIDTH / 3.0;
            for (param, text) in [
                (BusParam::Comp, "COMP"),
                (BusParam::CompMix, "MIX"),
                (BusParam::CompHpf, "HPF"),
            ] {
                let dial = Binding::Bus(bus, param);
                actions.extend(labelled(ui, values, dial, text, DIAL_SIZE, width));
            }
        },
    );
}

const MULTIBAND_WIDTH: f32 = 195.0;

/// The multiband compressor of a bus, behind its compressor: the knob of each band, the two
/// corners between them, and the most a band takes away now.
fn multiband_column(ui: &mut Ui, bus: usize, values: &mut Values<'_>, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    let taken = values.view().buses.get(bus).map_or(0.0, |b| {
        b.multiband
            .bands
            .iter()
            .zip(b.multiband_reduction_db)
            .filter(|(knob, _)| **knob > 0.0)
            .fold(0.0_f32, |most, (_, db)| most.max(db))
    });
    let head = if taken >= 0.05 {
        format!("MULTIBAND  -{taken:.1} dB")
    } else {
        "MULTIBAND".to_owned()
    };
    let color = if taken >= 0.05 {
        p.accent
    } else {
        p.text_faint
    };
    ui.label(RichText::new(head).size(9.5).color(color));
    ui.add_space(4.0 + KIND_HEIGHT);
    ui.allocate_ui_with_layout(
        vec2(MULTIBAND_WIDTH, DIAL_ROW_HEIGHT),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let width = MULTIBAND_WIDTH / 5.0;
            for (param, text) in [
                (MultibandParam::Band(0), "LOW"),
                (MultibandParam::LowSplit, "X LO"),
                (MultibandParam::Band(1), "MID"),
                (MultibandParam::HighSplit, "X HI"),
                (MultibandParam::Band(2), "HIGH"),
            ] {
                let dial = Binding::Bus(bus, BusParam::Multiband(param));
                actions.extend(labelled(ui, values, dial, text, DIAL_SIZE, width));
            }
        },
    );
}

/// The limiter of a bus: its switch, what it is at now, ceiling and lookahead.
const DELAY_WIDTH: f32 = 64.0;

/// The delay of a bus: how far all of it is held back, to line it up with another bus.
fn delay_column(ui: &mut Ui, bus: usize, values: &mut Values<'_>, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    ui.label(RichText::new("DELAY").size(9.5).color(p.text_faint));
    ui.add_space(4.0 + KIND_HEIGHT);
    let dial = Binding::Bus(bus, BusParam::Delay);
    actions.extend(labelled(ui, values, dial, "MS", DIAL_SIZE, DELAY_WIDTH));
}

fn limit_column(ui: &mut Ui, bus: usize, values: &mut Values<'_>, actions: &mut Vec<Action>) {
    let p = &Theme::current().palette;
    ui.label(RichText::new("LIMITER").size(9.5).color(p.text_faint));
    let switch = Binding::Bus(bus, BusParam::Limit);
    if let Some(Value::Switch(mut on)) = values.read(switch) {
        let title = switch.title(values.view());
        let size = vec2(LIMIT_WIDTH, KIND_HEIGHT);
        if toggle(ui, &mut on, "LIM", &title, Role::Assign, size).changed() {
            actions.extend(values.write(switch, Value::Switch(on)));
        }
    }
    ui.add_space(4.0);
    ui.allocate_ui_with_layout(
        vec2(LIMIT_WIDTH, DIAL_ROW_HEIGHT),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            let width = LIMIT_WIDTH / 2.0;
            for (param, text) in [
                (BusParam::LimitCeiling, "CEIL"),
                (BusParam::LimitLookahead, "LOOK"),
            ] {
                let dial = Binding::Bus(bus, param);
                actions.extend(labelled(ui, values, dial, text, DIAL_SIZE, width));
            }
        },
    );
}

fn band_column(
    ui: &mut Ui,
    owner: Owner,
    band: usize,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let p = &Theme::current().palette;
    let at = |param| owner.band(band, param);
    ui.label(
        RichText::new(format!("BAND {}", band + 1))
            .size(9.5)
            .color(p.text_faint),
    );
    ui.horizontal(|ui| {
        let gap = 3.0;
        ui.spacing_mut().item_spacing.x = gap;
        #[allow(clippy::cast_precision_loss)]
        let width = (BAND_WIDTH - gap * (KINDS.len() - 1) as f32) / KINDS.len() as f32;
        let kind = at(BandParam::Kind);
        let Some(Value::Kind(current)) = values.read(kind) else {
            return;
        };
        for (choice, text) in KINDS {
            let mut on = current == choice;
            let label = format!("{} {}", kind.title(values.view()), choice.word());
            let response = toggle(
                ui,
                &mut on,
                text,
                &label,
                Role::Assign,
                vec2(width, KIND_HEIGHT),
            );
            // A click on the kind it has changes nothing: one of them is always on.
            if response.changed() && on {
                actions.extend(values.write(kind, Value::Kind(choice)));
            }
        }
    });
    ui.add_space(4.0);
    ui.allocate_ui_with_layout(
        vec2(BAND_WIDTH, DIAL_ROW_HEIGHT),
        Layout::left_to_right(Align::Min),
        |ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            #[allow(clippy::cast_precision_loss)]
            let width = BAND_WIDTH / DIALS.len() as f32;
            for (param, text) in DIALS {
                actions.extend(labelled(ui, values, at(param), text, DIAL_SIZE, width));
            }
        },
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_dynamic_bands_say_how_far_in_they_are() {
        let mut bands = Band::all_flat();
        assert_eq!(dynamic_now(&bands, &[0.0; EQ_BANDS]), None);
        bands[1] = Band {
            gain_db: -12.0,
            threshold_db: -30.0,
            ..bands[1]
        };
        bands[3] = Band {
            gain_db: 4.0,
            threshold_db: -20.0,
            ..bands[3]
        };
        // A plain band with a gain is not named.
        bands[0] = bands[0].with_gain(3.0);
        let now = [3.0, -4.5, 0.0, 0.0];
        assert_eq!(
            dynamic_now(&bands, &now).as_deref(),
            Some("band 2 -4.5 of -12.0 dB   band 4 +0.0 of +4.0 dB")
        );
        // The curve of this moment: the dynamic bands where they stand, the plain one as set,
        // also while it still slides.
        let standing = bands_now(&bands, &[1.0, -4.5, 0.0, 0.0]);
        assert!((standing[0].gain_db - 3.0).abs() < f32::EPSILON);
        assert!((standing[1].gain_db + 4.5).abs() < f32::EPSILON);
        assert!(standing[3].gain_db.abs() < f32::EPSILON);
        // A state that names no gains leaves the bands as they are set.
        assert_eq!(bands_now(&bands, &[]), bands.to_vec());
    }
}
