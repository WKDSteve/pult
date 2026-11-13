//! Snapshot images of the console: rendered offscreen through wgpu and compared with
//! `tests/snapshots/*.png`. After a deliberate change of the look, refresh them with
//! `UPDATE_SNAPSHOTS=1 cargo test -p pult-app --test design` and review the new images.
//!
//! The images show a fuller desk than `FakeDesk` (seven inputs, three buses, solo, mute, a
//! missing device), built here as a `MixerView` with meters that depend only on the time.

use std::path::PathBuf;
use std::time::Duration;

use eframe::egui::{Rect, pos2, vec2};
use egui_kittest::Harness;
use pult_app::binding::Values;
use pult_app::console::{self, ConsoleState, Tab};
use pult_app::layout::{self, Control, Meters};
use pult_app::theme::Theme;
use pult_app::widgets::strip;
use pult_core::view::{
    BusView, EventView, InputView, LoudnessView, MeterPoint, MixerView, SourceKind,
};

/// Frames to run before the shot, so peak lines have something to hold.
const FRAMES: usize = 40;
/// Time of the first frame and the step per frame, in seconds.
const START: f64 = 20.0;
const STEP: f64 = 1.0 / 30.0;

type Showcase = (&'static str, SourceKind, u32, f32, &'static [usize], f32);

/// Inputs of the showcase: name, kind, rate, gain, buses, base level of the meter.
const INPUTS: [Showcase; 7] = [
    ("Mic 1", SourceKind::Device, 48_000, -3.0, &[0, 2], -14.0),
    ("Guitar", SourceKind::Device, 48_000, -8.5, &[0], -9.0),
    (
        "Spotify",
        SourceKind::Program,
        44_100,
        -12.0,
        &[0, 1],
        -18.0,
    ),
    ("Discord", SourceKind::Program, 48_000, 0.0, &[1, 2], -22.0),
    ("Browser", SourceKind::Program, 48_000, -20.0, &[0], -30.0),
    (
        "CABLE Output",
        SourceKind::Device,
        48_000,
        0.0,
        &[0, 1],
        -16.0,
    ),
    ("USB Mic", SourceKind::Device, 44_100, 0.0, &[2], -60.0),
];

/// A level that wanders around 0 by up to 1, the same for the same `seed` and `t`.
#[allow(clippy::cast_possible_truncation)]
fn wobble(seed: f64, t: f64) -> f32 {
    ((t * (1.3 + seed * 0.37)).sin() * 0.6 + (t * (4.1 + seed)).sin() * 0.4) as f32
}

/// Mic 1 with a high-pass against rumble, the Guitar turned over.
fn showcase_strip(input: usize) -> pult_core::strip::StripSettings {
    pult_core::strip::StripSettings {
        hpf_hz: if input == 0 { 80.0 } else { 20.0 },
        polarity: input == 1,
        ..pult_core::strip::StripSettings::default()
    }
}

fn showcase(t: f64) -> MixerView {
    let inputs: Vec<InputView> = INPUTS
        .iter()
        .enumerate()
        .map(|(i, &(name, kind, sample_rate, gain_db, buses, base))| {
            #[allow(clippy::cast_precision_loss)]
            let seed = i as f64;
            InputView {
                name: name.to_owned(),
                label: None,
                kind,
                sample_rate,
                gain_db,
                muted: i == 3,
                solo: i == 1,
                pan: [0.0, -0.35, 0.25, 0.0, 0.0, 0.0, 0.0][i],
                gate: [3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0][i],
                comp: [4.0, 2.5, 0.0, 0.0, 0.0, 0.0, 0.0][i],
                // Mic 1 and Guitar share the automixer; Mic 1 speaks.
                automix: i < 2,
                automix_weight_db: 0.0,
                automix_gain_db: [-0.4, -14.0, 0.0, 0.0, 0.0, 0.0, 0.0][i],
                // Mic 1: the compressor takes 4.5 dB; Guitar: its gate is shut.
                gate_reduction_db: [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0][i],
                gate_open: true,
                comp_reduction_db: [4.5, 1.2, 0.0, 0.0, 0.0, 0.0, 0.0][i],
                duck: pult_core::duck::DuckSettings::default(),
                duck_reduction_db: 0.0,
                strip: showcase_strip(i),
                buses: buses.to_vec(),
                sends: Vec::new(),
                present: i != 6,
                peak_db: (i != 3 && i != 6)
                    .then(|| (base + gain_db.min(0.0) * 0.3 + wobble(seed, t) * 8.0).min(0.0)),
                drift_ppm: if i == 2 { 41.0 } else { 0.0 },
                dropouts: if i == 5 { 3 } else { 0 },
            }
        })
        .collect();
    let buses = [
        ("A1", "Scarlett 2i2", 0.0),
        ("A2", "Headphones", -6.0),
        ("B1", "Stream (CABLE In)", -2.0),
    ]
    .iter()
    .enumerate()
    .map(|(j, &(name, device, gain_db))| {
        #[allow(clippy::cast_precision_loss)]
        let seed = 10.0 + j as f64;
        BusView {
            name: name.to_owned(),
            label: None,
            device: device.to_owned(),
            sample_rate: 48_000,
            gain_db,
            muted: false,
            eq_on: j != 1,
            eq: if j == 0 {
                let mut bands = pult_core::Band::all_flat();
                bands[0] = bands[0].with_gain(3.0);
                bands[2] = bands[2].with_gain(-4.5);
                bands
            } else {
                pult_core::Band::all_flat()
            },
            limit: pult_core::view::LimitSettings {
                on: j != 1,
                ceiling_db: -0.3,
                lookahead_ms: 1.0,
            },
            monitor: false,
            minus: None,
            // The stream bus with its bus compressor at work, half parallel.
            comp: [0.0, 0.0, 3.0][j],
            comp_mix: [100.0, 100.0, 50.0][j],
            comp_hpf_hz: [20.0, 20.0, 120.0][j],
            delay_ms: 0.0,
            comp_reduction_db: [0.0, 0.0, 2.4][j],
            // The stream bus at the loudness of a streaming platform.
            loudness: LoudnessView {
                short_term_lufs: Some([-19.2, -23.5, -14.1][j]),
                integrated_lufs: Some([-20.0, -24.0, -14.0][j]),
                ..LoudnessView::default()
            },
            limited_db: if j == 0 { 2.5 } else { 0.0 },
            present: true,
            peak_db: Some((-10.0 + gain_db + wobble(seed, t) * 6.0).min(0.0)),
            clipped: 0,
        }
    })
    .collect();
    MixerView {
        elapsed: Duration::from_secs(2 * 3600 + 14 * 60 + 7) + Duration::from_secs_f64(t),
        dropouts_per_hour: 0.4,
        glitches: 1,
        meter_point: MeterPoint::PostFader,
        inputs,
        buses,
        events: Vec::new(),
        profile: Some(PathBuf::from(r"C:\Users\me\pult\studio.toml")),
        scenes: Vec::new(),
        midi: None,
    }
}

/// The events of the showcase, delivered with the first view as a desk would.
fn first_events() -> Vec<EventView> {
    vec![
        EventView {
            elapsed: Duration::from_secs(5112),
            kind: pult_core::view::EventKind::Lost,
            text: "[7] USB Mic is gone".to_owned(),
        },
        EventView {
            elapsed: Duration::from_secs(7980),
            kind: pult_core::view::EventKind::Back,
            text: "A1 Scarlett 2i2 is back".to_owned(),
        },
    ]
}

#[test]
fn channel_strip() {
    let mut meters = Meters::default();
    let mut frame = 0_u32;
    let mut harness = Harness::builder()
        .with_size(vec2(318.0, 640.0))
        .with_pixels_per_point(2.0)
        .build_ui(move |ui| {
            Theme::current().apply(ui.ctx());
            let now = START + f64::from(frame) * STEP;
            frame += 1;
            let view = showcase(now);
            let m = &Theme::current().metrics;
            let p = &Theme::current().palette;
            let full = ui.max_rect();
            ui.painter().rect_filled(full, 0.0, p.base);
            meters.fit(&view);
            let mut values = Values::new(&view);
            let mut show = |ui: &mut _, rect, element: layout::Element| {
                let Control::Strip(strip) = element.control else {
                    unreachable!("the layout builds strips");
                };
                strip::show(ui, rect, &strip, &mut values, &mut meters, now);
            };
            let top = full.top() + 10.0;
            let height = full.height() - 20.0;
            for (slot, index) in [1_usize, 2].into_iter().enumerate() {
                #[allow(clippy::cast_precision_loss)]
                let x = full.left() + 10.0 + slot as f32 * (m.strip_width + m.strip_gap);
                let rect = Rect::from_min_size(pos2(x, top), vec2(m.strip_width, height));
                show(ui, rect, layout::input_strip(index, &view));
            }
            let x = full.left() + 10.0 + 2.0 * (m.strip_width + m.strip_gap) + 6.0;
            let rect = Rect::from_min_size(pos2(x, top), vec2(m.bus_strip_width, height));
            show(ui, rect, layout::bus_strip(0));
        });
    harness.run_steps(FRAMES);
    harness.snapshot("channel_strip");
}

/// The whole window on `tab`, with one refused action in the log.
fn console_shot(tab: Tab, name: &str) {
    let mut state = ConsoleState::default();
    state.tab = tab;
    let mut frame = 0_u32;
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 760.0))
        .with_pixels_per_point(1.5)
        .build_ui(move |ui| {
            Theme::current().apply(ui.ctx());
            let now = START + f64::from(frame) * STEP;
            let mut view = showcase(now);
            if frame == 0 {
                view.events = first_events();
            }
            if frame == 1 {
                state.refused(
                    Duration::from_secs(8040),
                    "[3] would feed no bus; mute it instead",
                );
            }
            frame += 1;
            let _ = console::show(ui, &view, &mut state, now);
        });
    harness.run_steps(FRAMES);
    harness.snapshot(name);
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "the pictures are of Windows: its fonts, its renderer"
)]
fn console_overview() {
    console_shot(Tab::Console, "console_overview");
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "the pictures are of Windows: its fonts, its renderer"
)]
fn matrix_view() {
    console_shot(Tab::Matrix, "matrix_view");
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "the pictures are of Windows: its fonts, its renderer"
)]
fn equalizer_view() {
    console_shot(Tab::Equalizer, "equalizer_view");
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "the pictures are of Windows: its fonts, its renderer"
)]
fn events_view() {
    console_shot(Tab::Events, "events_view");
}

#[test]
fn start_screen() {
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 760.0))
        .with_pixels_per_point(1.5)
        .build_ui(|ui| {
            Theme::current().apply(ui.ctx());
            let base = Theme::current().palette.base;
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::new().fill(base))
                .show(ui, |ui| {
                    pult_app::start::screen(
                        ui,
                        Some(std::path::Path::new(r"C:\Users\me\pult\studio.toml")),
                        Some("input \"Analogue 1 + 2\": no capture device matches"),
                    )
                });
        });
    harness.run_steps(4);
    harness.snapshot("start_screen");
}

#[test]
#[cfg_attr(
    not(windows),
    ignore = "the pictures are of Windows: its fonts, its renderer"
)]
fn options_dialog() {
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 760.0))
        .with_pixels_per_point(1.5)
        .build_ui(|ui| {
            Theme::current().apply(ui.ctx());
            let base = Theme::current().palette.base;
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::new().fill(base))
                .show(ui, |ui| {
                    let start = pult_app::settings::AutoStart {
                        on: true,
                        mode: pult_app::settings::StartMode::Window,
                    };
                    let _ = pult_app::options::show(ui, start, None);
                });
        });
    harness.run_steps(4);
    harness.snapshot("options_dialog");
}

#[test]
fn tray_hint() {
    let mut harness = Harness::builder()
        .with_size(vec2(1280.0, 760.0))
        .with_pixels_per_point(1.5)
        .build_ui(|ui| {
            Theme::current().apply(ui.ctx());
            let base = Theme::current().palette.base;
            eframe::egui::CentralPanel::default()
                .frame(eframe::egui::Frame::new().fill(base))
                .show(ui, |ui| {
                    let _ = pult_app::tray::hint(ui);
                });
        });
    harness.run_steps(4);
    harness.snapshot("tray_hint");
}
