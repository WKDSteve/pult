//! The console driven like a user would: clicks, drags, double clicks and keys on the real
//! window against `FakeDesk`, checked through what the desk ends up with. Controls are found
//! by their accessible names (`Spotify.exe mute`, `Spotify.exe to A1`, `Spotify.exe gain dB`).

use eframe::egui::{Key, vec2};
use egui_kittest::kittest::Queryable;
use egui_kittest::{Harness, HarnessBuilder};
use pult_app::console::{self, ConsoleState, Tab};
use pult_app::log::Source;
use pult_app::start::{self, Choice};
use pult_app::theme::Theme;
use pult_app::tray::{self, Answer};
use pult_core::view::{Desk, FakeDesk, MixerView};

/// `FakeDesk`'s third input: -3 dB, on bus A2 only.
const SPOTIFY: usize = 2;

struct App {
    desk: FakeDesk,
    state: ConsoleState,
    /// An input to show under another name: (index, name).
    rename: Option<(usize, String)>,
}

fn harness() -> Harness<'static, App> {
    let app = App {
        desk: FakeDesk::new(),
        state: ConsoleState::default(),
        rename: None,
    };
    HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        // Short steps, so two clicks in successive frames count as a double click.
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(
            |ui, app: &mut App| {
                Theme::current().apply(ui.ctx());
                let mut view = app.desk.view();
                if let Some((input, name)) = &app.rename {
                    view.inputs[*input].name.clone_from(name);
                }
                for action in console::show(ui, &view, &mut app.state, 0.0) {
                    if let Err(reason) = app.desk.act(action) {
                        app.state.refused(view.elapsed, &reason);
                    }
                }
            },
            app,
        )
}

/// The desk's state after the harness settled.
fn desk_view(harness: &mut Harness<'_, App>) -> MixerView {
    harness.run_steps(2);
    harness.state_mut().desk.view()
}

#[test]
fn mute_and_solo_buttons_switch_the_input() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Spotify.exe mute").click();
    harness.run_steps(2);
    harness.get_by_label("Spotify.exe solo").click();
    let view = desk_view(&mut harness);
    assert!(view.inputs[SPOTIFY].muted);
    assert!(view.inputs[SPOTIFY].solo);
}

#[test]
fn a_bus_button_routes_and_the_desk_can_refuse() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Spotify.exe to A1").click();
    let view = desk_view(&mut harness);
    assert_eq!(view.inputs[SPOTIFY].buses, [0, 1]);

    // Taking it off both buses would leave it feeding nothing: the desk says no, the log
    // and the status line say why.
    harness.get_by_label("Spotify.exe to A1").click();
    harness.run_steps(2);
    harness.get_by_label("Spotify.exe to A2").click();
    let view = desk_view(&mut harness);
    assert_eq!(view.inputs[SPOTIFY].buses, [1]);
    let last = harness
        .state()
        .state
        .log()
        .last()
        .cloned()
        .expect("an entry");
    assert_eq!(last.source, Source::Refused);
    assert!(last.text.contains("would feed no bus"), "{}", last.text);
}

#[test]
fn dragging_the_fader_up_raises_the_gain() {
    let mut harness = harness();
    harness.run_steps(2);
    let center = harness.get_by_label("Spotify.exe gain dB").rect().center();
    harness.hover_at(center);
    harness.drag_at(center);
    harness.run_steps(1);
    harness.hover_at(center - vec2(0.0, 30.0));
    harness.run_steps(1);
    harness.drop_at(center - vec2(0.0, 30.0));
    let gain = desk_view(&mut harness).inputs[SPOTIFY].gain_db;
    assert!(gain > -3.0 + 1.0, "gain {gain}");
}

#[test]
fn double_click_sets_the_fader_to_unity() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Spotify.exe gain dB").click();
    harness.step();
    harness.get_by_label("Spotify.exe gain dB").click();
    let gain = desk_view(&mut harness).inputs[SPOTIFY].gain_db;
    assert!(gain.abs() < 1e-6, "gain {gain}");
}

#[test]
fn keys_step_a_focused_fader() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Spotify.exe gain dB").focus();
    // The focus lock that keeps the arrows on the fader holds from the frame after the focus.
    harness.run_steps(2);
    harness.key_press(Key::ArrowUp);
    harness.run_steps(1);
    harness.key_press(Key::PageDown);
    let gain = desk_view(&mut harness).inputs[SPOTIFY].gain_db;
    assert!((gain - (-3.0 + 0.5 - 3.0)).abs() < 1e-4, "gain {gain}");
}

#[test]
fn the_matrix_routes_too() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Matrix").click();
    harness.run_steps(2);
    assert_eq!(harness.state().state.tab, Tab::Matrix);
    harness.get_by_label("Spotify.exe to A1").click();
    let view = desk_view(&mut harness);
    assert_eq!(view.inputs[SPOTIFY].buses, [0, 1]);
}

#[test]
fn inputs_of_the_same_name_keep_their_own_crosspoints() {
    // Two instances of one program carry the same name; each crosspoint must still switch
    // its own input.
    let mut harness = harness();
    harness.state_mut().rename = Some((1, "Spotify.exe".to_owned()));
    harness.state_mut().state.tab = Tab::Matrix;
    harness.run_steps(2);
    let nodes: Vec<_> = harness.get_all_by_label("Spotify.exe to A2").collect();
    assert_eq!(nodes.len(), 2);
    nodes[0].click();
    let view = desk_view(&mut harness);
    // Input 2 (index 1) fed A1 and A2; the click takes it off A2 and leaves input 3 alone.
    assert_eq!(view.inputs[1].buses, [0]);
    assert_eq!(view.inputs[SPOTIFY].buses, [1]);
}

#[test]
fn the_start_screen_offers_a_retry_and_the_demo() {
    let mut harness = HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        .build_ui_state(
            |ui, picked: &mut Vec<Choice>| {
                let path = std::path::Path::new("studio.toml");
                if let Some(choice) = start::screen(ui, Some(path), Some("no device")) {
                    picked.push(choice);
                }
            },
            Vec::new(),
        );
    harness.run_steps(2);
    harness.get_by_label("Try again").click();
    harness.run_steps(2);
    harness.get_by_label("Demo desk").click();
    harness.run_steps(2);
    assert_eq!(harness.state(), &[Choice::Retry, Choice::Demo]);

    // Without a profile there is nothing to try again.
    let harness = HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        .build_ui(|ui| {
            start::screen(ui, None, None);
        });
    assert!(harness.query_by_label("Try again").is_none());
}

#[test]
fn while_the_profile_opens_the_start_screen_offers_nothing() {
    let mut harness = HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        .build_ui(|ui| {
            start::starting(ui, Some(std::path::Path::new("studio.toml")));
        });
    harness.run_steps(2);
    assert!(harness.query_by_label("Starting the route").is_some());
    assert!(harness.query_by_label("studio.toml").is_some());
    // Nothing to click until the route stands or fails.
    assert!(harness.query_by_label("Try again").is_none());
    assert!(harness.query_by_label("Demo desk").is_none());
}

#[test]
fn the_first_close_asks_whether_to_hide_or_quit() {
    for (button, answer) in [("To the tray", Answer::ToTray), ("Quit pult", Answer::Quit)] {
        let mut harness = HarnessBuilder::default()
            .with_size(vec2(1280.0, 760.0))
            .build_ui_state(
                |ui, picked: &mut Vec<Answer>| {
                    if let Some(answer) = tray::hint(ui) {
                        picked.push(answer);
                    }
                },
                Vec::new(),
            );
        harness.run_steps(2);
        assert!(
            harness
                .query_by_label("pult keeps playing in the tray")
                .is_some()
        );
        harness.get_by_label(button).click();
        harness.run_steps(2);
        assert_eq!(harness.state(), &[answer]);
    }
}

#[test]
fn a_cut_name_shows_in_full_on_hover() {
    let device = "Lautsprecher (2- Focusrite USB Audio)";
    let mut harness = harness();
    harness.state_mut().state.tab = Tab::Matrix;
    harness.run_steps(2);
    // The matrix head cuts the device name; only the hover area carries it in full.
    assert_eq!(harness.get_all_by_label(device).count(), 1);
    harness.get_by_label(device).hover();
    harness.run_steps(30);
    assert_eq!(harness.get_all_by_label(device).count(), 2, "no tooltip");
}
