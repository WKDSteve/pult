//! The console driven like a user would: clicks, drags, double clicks and keys on the real
//! window against `FakeDesk`, checked through what the desk ends up with. Controls are found
//! by their accessible names (`Spotify.exe mute`, `Spotify.exe to A1`, `Spotify.exe gain dB`).

use eframe::egui::{Key, vec2};
use egui_kittest::kittest::{NodeT, Queryable};
use egui_kittest::{Harness, HarnessBuilder};
use pult_app::console::{self, ConsoleState, Tab};
use pult_app::log::Source;
use pult_app::options::{self, BACKGROUND_LABEL};
use pult_app::settings::{AutoStart, StartMode};
use pult_app::start::{self, Choice};
use pult_app::theme::Theme;
use pult_app::tray::{self, Answer, START_LABEL};
use pult_core::view::{Desk, FakeDesk, MixerView};

/// `FakeDesk`'s third input: -3 dB, on bus A2 only.
const SPOTIFY: usize = 2;

struct App {
    desk: FakeDesk,
    state: ConsoleState,
    /// An input to show under another name: (index, name).
    rename: Option<(usize, String)>,
    /// An input to show with a label: (index, label).
    label: Option<(usize, String)>,
    /// Hides the scenes of the desk.
    no_scenes: bool,
}

fn harness() -> Harness<'static, App> {
    let app = App {
        desk: FakeDesk::new(),
        state: ConsoleState::default(),
        rename: None,
        label: None,
        no_scenes: false,
    };
    HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        // Short steps, so two clicks in successive frames count as a double click.
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(
            |ui, app: &mut App| {
                Theme::current().apply(ui.ctx());
                let mut view = app.desk.view();
                if app.no_scenes {
                    view.scenes.clear();
                }
                if let Some((input, name)) = &app.rename {
                    view.inputs[*input].name.clone_from(name);
                }
                if let Some((input, label)) = &app.label {
                    view.inputs[*input].label = Some(label.clone());
                }
                for action in console::show(ui, &view, &mut app.state, 0.0) {
                    app.state.act(&mut app.desk, view.elapsed, action);
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
fn a_screen_reader_sets_the_fader() {
    let mut harness = harness();
    harness.run_steps(2);
    let fader = harness.get_by_label("Spotify.exe gain dB");
    let node = fader.accesskit_node();
    assert_eq!(
        node.min_numeric_value(),
        Some(-60.0),
        "the floor is the range"
    );
    assert_eq!(node.max_numeric_value(), Some(12.0), "the top is the range");
    fader.set_value_accesskit(-12.0);
    harness.run_steps(2);
    let gain = desk_view(&mut harness).inputs[SPOTIFY].gain_db;
    assert!((gain - -12.0).abs() < 1e-4, "gain {gain}");
    // Off is below the floor, and nothing goes above the top.
    harness
        .get_by_label("Spotify.exe gain dB")
        .set_value_accesskit(-500.0);
    harness.run_steps(2);
    let off = desk_view(&mut harness).inputs[SPOTIFY].gain_db;
    assert!(off < -60.0, "off is below the floor: {off}");
    harness
        .get_by_label("Spotify.exe gain dB")
        .set_value_accesskit(500.0);
    harness.run_steps(2);
    assert_eq!(desk_view(&mut harness).inputs[SPOTIFY].gain_db, 12.0);
    harness
        .get_by_label("Spotify.exe gain dB")
        .set_value_accesskit(f64::NAN);
    harness.run_steps(2);
    assert_eq!(
        desk_view(&mut harness).inputs[SPOTIFY].gain_db,
        12.0,
        "not a number is no request"
    );
}

#[test]
fn a_screen_reader_sets_the_pan() {
    let mut harness = harness();
    harness.run_steps(2);
    let knob = harness.get_by_label("Spotify.exe pan");
    assert_eq!(knob.accesskit_node().min_numeric_value(), Some(-1.0));
    assert_eq!(knob.accesskit_node().max_numeric_value(), Some(1.0));
    knob.set_value_accesskit(-0.5);
    harness.run_steps(2);
    let pan = desk_view(&mut harness).inputs[SPOTIFY].pan;
    assert!((pan - -0.5).abs() < 1e-4, "pan {pan}");
    harness
        .get_by_label("Spotify.exe pan")
        .set_value_accesskit(7.0);
    harness.run_steps(2);
    assert_eq!(
        desk_view(&mut harness).inputs[SPOTIFY].pan,
        1.0,
        "the pan stays in its range"
    );
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
fn a_label_names_the_strip_and_the_matrix_row() {
    let mut harness = harness();
    harness.state_mut().label = Some((SPOTIFY, "Musik".to_owned()));
    harness.run_steps(2);
    harness.get_by_label("Musik").hover();
    harness.get_by_label("Musik mute").click();
    assert!(desk_view(&mut harness).inputs[SPOTIFY].muted);
    assert!(harness.query_by_label("Spotify.exe mute").is_none());

    harness.state_mut().state.tab = Tab::Matrix;
    harness.run_steps(2);
    harness.get_by_label("Musik to A1").click();
    assert_eq!(desk_view(&mut harness).inputs[SPOTIFY].buses, [0, 1]);
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
    harness.get_by_label("Options").click();
    harness.run_steps(2);
    assert_eq!(
        harness.state(),
        &[Choice::Retry, Choice::Demo, Choice::Options]
    );

    // Without a profile there is nothing to try again.
    let harness = HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        .build_ui(|ui| {
            start::screen(ui, None, None);
        });
    assert!(harness.query_by_label("Try again").is_none());
    assert!(
        harness.query_by_label("Options").is_some(),
        "the options matter before the first profile too"
    );
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
fn escape_cancels_the_hint() {
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
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert_eq!(harness.state().first(), Some(&Answer::Cancel));
}

#[test]
fn a_scene_is_chosen_from_the_menu_and_the_status_says_so() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Scene").click();
    harness.run_steps(2);
    harness.get_by_label("Abend").click();
    let view = desk_view(&mut harness);
    assert_eq!(view.inputs[0].gain_db, -12.0);
    assert!(!view.inputs[1].muted);
    let entry = harness.state().state.log().last().cloned().unwrap();
    assert_eq!(entry.source, Source::Done);
    assert_eq!(entry.text, "scene Abend");
}

#[test]
fn a_route_without_scenes_offers_none() {
    let mut harness = harness();
    harness.state_mut().no_scenes = true;
    harness.run_steps(2);
    assert!(harness.query_by_label("Scene").is_none());
}

#[test]
fn saving_the_profile_says_so() {
    let mut harness = harness();
    harness.run_steps(2);
    harness.get_by_label("Save profile").click();
    harness.run_steps(2);
    let entry = harness.state().state.log().last().cloned().unwrap();
    assert_eq!(entry.source, Source::Done);
    assert!(entry.text.contains("saved the current state to pult.toml"));
    // The status line shows it at once.
    assert!(
        harness
            .query_by_label_contains("saved the current state")
            .is_some()
    );
}

#[test]
fn the_first_close_asks_whether_to_hide_or_quit() {
    for (button, answer) in [
        ("To the tray", Answer::ToTray),
        ("Quit pult", Answer::Quit),
        ("Cancel", Answer::Cancel),
    ] {
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

/// The options as the window keeps them: what the user chooses holds at once.
struct Options {
    start: AutoStart,
    problem: Option<String>,
    closed: bool,
}

fn options_harness(start: AutoStart, problem: Option<&str>) -> Harness<'static, Options> {
    let state = Options {
        start,
        problem: problem.map(str::to_owned),
        closed: false,
    };
    let mut harness = HarnessBuilder::default()
        .with_size(vec2(800.0, 500.0))
        .build_ui_state(
            |ui, state: &mut Options| {
                Theme::current().apply(ui.ctx());
                let answer = options::show(ui, state.start, state.problem.as_deref());
                if let Some(choice) = answer.choice {
                    state.start = choice;
                }
                state.closed |= answer.close;
            },
            state,
        );
    harness.run_steps(2);
    harness
}

const OFF: AutoStart = AutoStart {
    on: false,
    mode: StartMode::Tray,
};

#[test]
fn the_start_with_the_system_is_a_check_and_off_by_default() {
    assert_eq!(AutoStart::default(), OFF);
    let mut harness = options_harness(OFF, None);
    assert!(
        harness
            .query_by_label_contains("only when you start it")
            .is_some()
    );
    harness.get_by_label(START_LABEL).click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().start,
        AutoStart {
            on: true,
            mode: StartMode::Tray
        },
        "turned on, it comes up in the background"
    );
    assert!(
        harness
            .query_by_label_contains("stays in the tray")
            .is_some()
    );
    harness.get_by_label(START_LABEL).click();
    harness.run_steps(2);
    assert_eq!(harness.state().start, OFF);
}

#[test]
fn the_way_it_starts_can_be_chosen_while_it_starts() {
    let on = AutoStart {
        on: true,
        mode: StartMode::Tray,
    };
    let mut harness = options_harness(on, None);
    harness.get_by_label(BACKGROUND_LABEL).click();
    harness.run_steps(2);
    assert_eq!(
        harness.state().start,
        AutoStart {
            on: true,
            mode: StartMode::Window
        },
    );
    assert!(
        harness
            .query_by_label_contains("opens its window")
            .is_some()
    );
    harness.get_by_label(BACKGROUND_LABEL).click();
    harness.run_steps(2);
    assert_eq!(harness.state().start, on);
}

#[test]
fn the_way_it_starts_does_nothing_while_it_does_not_start() {
    let off = AutoStart {
        on: false,
        mode: StartMode::Window,
    };
    let mut harness = options_harness(off, None);
    assert!(
        harness
            .get_by_label(BACKGROUND_LABEL)
            .accesskit_node()
            .is_disabled()
    );
    harness.get_by_label(BACKGROUND_LABEL).click();
    harness.run_steps(2);
    assert_eq!(harness.state().start, off);
    // Turning it on brings the choice back.
    harness.get_by_label(START_LABEL).click();
    harness.run_steps(2);
    assert_eq!(harness.state().start, AutoStart { on: true, ..off });
    assert!(
        !harness
            .get_by_label(BACKGROUND_LABEL)
            .accesskit_node()
            .is_disabled()
    );
}

#[test]
fn the_options_say_why_a_choice_did_not_take_and_close() {
    let mut harness = options_harness(OFF, Some("could not change the start on signing in: no"));
    assert!(
        harness
            .query_by_label_contains("could not change")
            .is_some()
    );
    assert!(!harness.state().closed);
    harness.get_by_label("Close").click();
    harness.run_steps(2);
    assert!(harness.state().closed);
}

#[test]
fn escape_closes_the_options() {
    let mut harness = options_harness(OFF, None);
    harness.key_press(Key::Escape);
    harness.run_steps(2);
    assert!(harness.state().closed);
}

#[test]
fn the_header_asks_for_the_options_once() {
    let mut harness = harness();
    harness.run_steps(2);
    assert!(!harness.state_mut().state.take_options_asked());
    harness.get_by_label("Options").click();
    harness.run_steps(2);
    assert!(harness.state_mut().state.take_options_asked());
    assert!(!harness.state_mut().state.take_options_asked());
}

/// The console with the dialog over it, wired as the window wires them.
struct Window {
    desk: FakeDesk,
    state: ConsoleState,
    start: AutoStart,
    open: bool,
}

#[test]
fn the_options_open_over_the_console_and_every_name_is_unique() {
    let window = Window {
        desk: FakeDesk::new(),
        state: ConsoleState::default(),
        start: OFF,
        open: false,
    };
    let mut harness = HarnessBuilder::default()
        .with_size(vec2(1280.0, 760.0))
        .with_step_dt(1.0 / 30.0)
        .build_ui_state(
            |ui, window: &mut Window| {
                Theme::current().apply(ui.ctx());
                let view = window.desk.view();
                let _ = console::show(ui, &view, &mut window.state, 0.0);
                if window.state.take_options_asked() {
                    window.open = true;
                }
                if window.open {
                    let answer = options::show(ui, window.start, None);
                    if let Some(choice) = answer.choice {
                        window.start = choice;
                    }
                    window.open = !answer.close;
                }
            },
            window,
        );
    harness.run_steps(2);
    assert!(harness.query_by_label(START_LABEL).is_none(), "not up yet");
    harness.get_by_label("Options").click();
    harness.run_steps(4);
    assert!(
        harness.state().open,
        "the click that opens them does not close them"
    );

    // `get_by_label` refuses a name that is there twice, as a test sheet and a screen reader
    // cannot tell such controls apart.
    for name in ["Options", START_LABEL, BACKGROUND_LABEL, "Close"] {
        let _ = harness.get_by_label(name);
    }

    harness.get_by_label(START_LABEL).click();
    harness.run_steps(2);
    assert!(harness.state().start.on);
    assert!(harness.state().open, "a choice leaves them up");
    harness.get_by_label("Close").click();
    harness.run_steps(2);
    assert!(!harness.state().open);
    assert!(harness.state().start.on, "closing keeps the choice");
}
