//! The commands to a running pult across the process boundary: the real program, its status and
//! its output, against a hub on the demo desk, so no audio device is needed.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use pult_core::midi::MidiStatus;
use pult_core::view::{Desk, FakeDesk};
use pult_hub::{Control, Env, IDENTITY};

/// A folder for one test, removed when it ends.
struct Folder(PathBuf);

impl Folder {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("pult-remote-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("a folder");
        Self(dir)
    }
}

impl Drop for Folder {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A hub on the demo desk, pumped by a thread as the owner of a route would.
struct Hub {
    stop: Arc<AtomicBool>,
    owner: Option<thread::JoinHandle<()>>,
}

impl Hub {
    fn start(dir: &Path) -> Self {
        Self::with_midi(dir, None)
    }

    /// A hub whose owner reports `midi`, as one with a controller would.
    fn with_midi(dir: &Path, midi: Option<MidiStatus>) -> Self {
        let env = Env::from_pairs([
            (IDENTITY.var("CONFIG_DIR"), dir.as_os_str().to_owned()),
            (IDENTITY.var("PORT"), "0".into()),
        ]);
        let (control, mut pump) = Control::start_with(env).expect("a hub");
        let stop = Arc::new(AtomicBool::new(false));
        let stopping = Arc::clone(&stop);
        let owner = thread::spawn(move || {
            let mut desk = FakeDesk::new();
            // As a route: it ends when asked to, and its hub with it.
            while !stopping.load(Ordering::Relaxed) && !pump.stop_asked() {
                let mut view = desk.view();
                view.midi.clone_from(&midi);
                pump.pump(&mut desk, &view);
                thread::sleep(Duration::from_millis(5));
            }
            drop(control);
        });
        Self {
            stop,
            owner: Some(owner),
        }
    }
}

impl Drop for Hub {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(owner) = self.owner.take() {
            let _ = owner.join();
        }
    }
}

fn pult(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_pult"))
        .args(args)
        .env("PULT_CONFIG_DIR", dir)
        .output()
        .expect("pult runs")
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn without_a_running_pult_every_command_says_so_and_fails() {
    let folder = Folder::new("none");
    for args in [
        &["mute", "1"][..],
        &["gain", "A1", "-6"],
        &["scene", "abend"],
        &["scenes"],
        &["show"],
        &["save"],
    ] {
        let output = pult(&folder.0, args);
        assert_eq!(output.status.code(), Some(1), "{args:?}");
        assert!(stdout(&output).is_empty(), "{args:?}");
        assert!(stderr(&output).contains("no pult is running"), "{args:?}");
    }
}

#[test]
fn commands_reach_the_running_pult() {
    let folder = Folder::new("acts");
    let _hub = Hub::start(&folder.0);

    let output = pult(&folder.0, &["gain", "A1", "-6"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "A1 gain -6.0 dB");

    // Several words of a name, with or without quotes around them.
    let output = pult(&folder.0, &["mute", "Analogue", "1"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "[2] muted");
    let output = pult(&folder.0, &["unmute", "Analogue 1"]);
    assert!(output.status.success(), "{}", stderr(&output));

    let output = pult(&folder.0, &["gain", "2", "-3", "over", "500"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let output = pult(&folder.0, &["pan", "spotify", "0.5", "over", "100"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let output = pult(&folder.0, &["route", "spotify", "A1"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let output = pult(&folder.0, &["scene", "Abend"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "scene Abend");

    let shown = stdout(&pult(&folder.0, &["show"]));
    assert!(shown.contains("gain -12.0 dB"), "the scene: {shown}");
    assert!(shown.contains("gain -6.0 dB"), "the bus: {shown}");
    assert!(shown.contains("pan 0.50"), "{shown}");
    assert!(shown.contains("scenes: Abend, Laut"), "{shown}");
}

#[test]
fn scenes_and_json_are_for_programs() {
    let folder = Folder::new("programs");
    let _hub = Hub::start(&folder.0);

    let output = pult(&folder.0, &["scenes"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(
        stdout(&output).lines().collect::<Vec<_>>(),
        ["Abend", "Laut"]
    );

    let output = pult(&folder.0, &["show", "--json"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let state: pult_core::protocol::State =
        serde_json::from_str(stdout(&output).trim()).expect("a state");
    assert_eq!(state.buses[0].name, "A1");
}

#[test]
fn a_refusal_and_wrong_words_fail_with_their_reason() {
    let folder = Folder::new("refused");
    let _hub = Hub::start(&folder.0);

    let output = pult(&folder.0, &["gain", "A1", "60"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("outside"), "{}", stderr(&output));

    let output = pult(&folder.0, &["scene", "nichts"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(stderr(&output).contains("no scene"), "{}", stderr(&output));

    let output = pult(&folder.0, &["unroute", "cable", "A1"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("would feed no bus"),
        "{}",
        stderr(&output)
    );

    let output = pult(&folder.0, &["mute", "vlc"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("no input or bus matches"),
        "{}",
        stderr(&output)
    );

    let output = pult(&folder.0, &["gain", "A1"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("usage: gain"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn what_needs_no_pult_is_refused_before_looking_for_one() {
    let folder = Folder::new("usage");
    let output = pult(&folder.0, &["save", "elsewhere.toml"]);
    assert_eq!(output.status.code(), Some(2));
    assert!(
        stderr(&output).contains("profile it came from"),
        "{}",
        stderr(&output)
    );
    let output = pult(&folder.0, &["show", "--yaml"]);
    assert_eq!(output.status.code(), Some(2));
    let output = pult(&folder.0, &["scenes", "abend"]);
    assert_eq!(output.status.code(), Some(2));
    for args in [
        &["stop", "now"][..],
        &["open", "--pdf"],
        &["start", "--tray"],
        &["open", "--print", "x"],
    ] {
        let output = pult(&folder.0, args);
        assert_eq!(output.status.code(), Some(2), "{args:?}");
    }
}

#[test]
fn stop_ends_the_route_and_says_so() {
    let folder = Folder::new("stop");
    let hub = Hub::start(&folder.0);
    let output = pult(&folder.0, &["stop"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "stopped");
    assert!(!folder.0.join("hub.json").exists());
    assert!(!folder.0.join("control.secret").exists());
    drop(hub);

    // Nothing to end is no failure: it is what was asked for.
    let output = pult(&folder.0, &["stop"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "no pult is running");
}

#[test]
fn open_gives_a_link_with_a_code_in_its_fragment() {
    let folder = Folder::new("open");
    let _hub = Hub::start(&folder.0);
    let output = pult(&folder.0, &["open", "--print"]);
    assert!(output.status.success(), "{}", stderr(&output));
    let link = stdout(&output);
    assert!(link.trim().starts_with("http://127.0.0.1:"), "{link}");
    assert!(link.contains("/#pair="), "{link}");
}

#[test]
fn start_with_a_route_running_starts_nothing_and_without_a_window_says_so() {
    let folder = Folder::new("start");
    let hub = Hub::start(&folder.0);
    let output = pult(&folder.0, &["start"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert_eq!(stdout(&output).trim(), "pult is running already");
    drop(hub);

    // A copy of the program alone in a folder: no route, and no window to start.
    let alone = folder
        .0
        .join(format!("pult{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(env!("CARGO_BIN_EXE_pult"), &alone).expect("a copy");
    let output = Command::new(&alone)
        .arg("start")
        .env("PULT_CONFIG_DIR", &folder.0)
        .output()
        .expect("pult runs");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no window beside"),
        "{}",
        stderr(&output)
    );
}

#[test]
fn midi_says_what_there_is_and_learn_checks_its_target() {
    let folder = Folder::new("midi");
    let output = pult(&folder.0, &["midi", "devices"]);
    assert!(output.status.success(), "{}", stderr(&output));
    assert!(
        stdout(&output).contains("inputs:"),
        "without a running pult"
    );
    assert_eq!(pult(&folder.0, &["midi"]).status.code(), Some(1));

    let _hub = Hub::start(&folder.0);
    // This owner runs no controller.
    let output = pult(&folder.0, &["midi"]);
    assert!(stdout(&output).contains("no MIDI"), "{}", stdout(&output));
    let output = pult(&folder.0, &["learn", "input/9/gain"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("no input 9"),
        "{}",
        stderr(&output)
    );
    let output = pult(&folder.0, &["learn", "input/1/meter"]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        stderr(&output).contains("only be read"),
        "{}",
        stderr(&output)
    );
    assert_eq!(pult(&folder.0, &["learn"]).status.code(), Some(2));
    assert_eq!(pult(&folder.0, &["midi", "x"]).status.code(), Some(2));
    assert_eq!(
        pult(&folder.0, &["midi", "listen", "x"]).status.code(),
        Some(2)
    );
}

/// It said "nothing moved" after half a minute, for a file that nothing could have helped.
#[test]
fn learn_and_midi_name_what_keeps_midi_off() {
    let folder = Folder::new("midi-off");
    let _hub = Hub::with_midi(
        &folder.0,
        Some(MidiStatus {
            problem: Some("midi.toml does not read".to_owned()),
            ..MidiStatus::default()
        }),
    );
    let started = Instant::now();
    let output = pult(&folder.0, &["learn", "input/1/gain"]);
    assert_eq!(output.status.code(), Some(1));
    let said = stderr(&output);
    assert!(said.contains("midi.toml does not read"), "{said}");
    assert!(!said.contains("nothing moved"), "{said}");
    assert_eq!(stdout(&output), "", "it does not ask for a movement first");
    let output = pult(&folder.0, &["unlearn", "input/1/gain"]);
    assert_eq!(output.status.code(), Some(1), "nothing was freed");
    assert!(
        stderr(&output).contains("cannot unlearn"),
        "{}",
        stderr(&output)
    );
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "it does not wait"
    );

    let said = stdout(&pult(&folder.0, &["midi"]));
    assert!(said.contains("MIDI is off"), "{said}");
    assert!(!said.contains("no device yet"), "{said}");
}

#[test]
fn many_commands_in_a_row_pair_once() {
    // The hub takes 10 pairings a minute: one session has to serve them all.
    let folder = Folder::new("many");
    let _hub = Hub::start(&folder.0);
    for _ in 0..14 {
        let output = pult(&folder.0, &["mute", "1"]);
        assert!(output.status.success(), "{}", stderr(&output));
    }
}
