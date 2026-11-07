//! The equalizer view: a row per bus with its switch and its bands, each band with its kind, gain,
//! frequency and width, and behind them its limiter with ceiling and lookahead. A strip has no room
//! for that; it only has the switches.

use eframe::egui::{
    Align, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke, Ui, vec2,
};
use pult_core::view::{Action, BandKind, BusView, EQ_BANDS, MixerView};

use crate::binding::{BandParam, Binding, BusParam, Value, Values};
use crate::theme::Theme;
use crate::widgets::dial::labelled;
use crate::widgets::toggle::{Role, toggle};

const BAND_WIDTH: f32 = 168.0;
const BAND_GAP: f32 = 10.0;
const DIAL_SIZE: f32 = 30.0;
const DIAL_ROW_HEIGHT: f32 = 58.0;
const KIND_HEIGHT: f32 = 20.0;
const SWITCH_SIZE: [f32; 2] = [48.0, 22.0];
const LIMIT_WIDTH: f32 = 112.0;

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
    });
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
                    band_column(ui, index, band, values, actions);
                },
            );
        }
        ui.allocate_ui_with_layout(
            vec2(LIMIT_WIDTH, KIND_HEIGHT + DIAL_ROW_HEIGHT + 22.0),
            Layout::top_down(Align::Min),
            |ui| {
                ui.set_width(LIMIT_WIDTH);
                limit_column(ui, index, values, actions);
            },
        );
    });
}

/// The limiter of a bus: its switch, what it is at now, ceiling and lookahead.
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
    bus: usize,
    band: usize,
    values: &mut Values<'_>,
    actions: &mut Vec<Action>,
) {
    let p = &Theme::current().palette;
    let at = |param| Binding::Bus(bus, BusParam::EqBand(band, param));
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
