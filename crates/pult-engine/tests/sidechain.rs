//! The sidechain of an input in the whole route, rendered without devices: its gate and its
//! compressor listen to another input, or to their own sound through filters; set by the
//! profile and by commands, shown, and kept.

use pult_core::control::{Command, Target};
use pult_core::sidechain::SidechainChange;
use pult_core::view::Desk as _;
use pult_engine::{EngineDesk, Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "A1"
device = "offline A1"

[[input]]
device = "bass"
buses = ["A1"]
gate = 5
sidechain_by = 2

[[input]]
device = "kick"
buses = ["A1"]
"#;

fn offline(text: &str) -> Offline {
    let spec = profile::parse(text).expect("the profile is valid");
    Offline::new(&spec, RATE, 1).expect("the route builds")
}

/// Runs `callbacks` with the constant levels `bass` and `kick`; returns the peak the bass put
/// into the mix in the last callback.
fn run(offline: &mut Offline, bass: f32, kick: f32, callbacks: usize) -> f32 {
    let feeds = [vec![bass; PERIOD], vec![kick; PERIOD]];
    let refs: Vec<&[f32]> = feeds.iter().map(Vec::as_slice).collect();
    for _ in 0..callbacks {
        offline.running().inputs[0].routes[0].stats.peak.take();
        offline.process(&refs, PERIOD);
    }
    offline.running().inputs[0].routes[0].stats.peak.take()
}

/// Runs `callbacks` of a sine of `hz` at half scale into the first input; returns the most its
/// compressor took in the last of them, in dB.
fn compressed(offline: &mut Offline, hz: f32, callbacks: usize) -> f32 {
    let step = std::f32::consts::TAU * hz / RATE as f32;
    let mut phase = 0.0_f32;
    let silence = vec![0.0; PERIOD];
    let mut taken = 0.0;
    for _ in 0..callbacks {
        let tone: Vec<f32> = (0..PERIOD)
            .map(|_| {
                phase = (phase + step) % std::f32::consts::TAU;
                0.5 * phase.sin()
            })
            .collect();
        offline.running().inputs[0].controls.comp_reduction.take();
        offline.process(&[&tone, &silence], PERIOD);
        taken = offline.running().inputs[0].controls.comp_reduction.take();
    }
    taken
}

#[test]
fn a_keyed_gate_opens_with_the_other_input_only() {
    let mut o = offline(PROFILE);
    // The bass plays, well above the threshold of its gate, but the kick is silent.
    let alone = run(&mut o, 0.3, 0.0, 100);
    assert!(alone < 1e-3, "shut without the kick: {alone}");
    let with = run(&mut o, 0.3, 0.3, 100);
    assert!((with - 0.3).abs() < 3e-3, "open with the kick: {with}");
    // The kick alone opens it for nothing: the gate lets through what the bass plays.
    let nothing = run(&mut o, 0.0, 0.3, 100);
    assert!(nothing < 1e-3, "{nothing}");
    // Back to itself: the bass opens its own gate again.
    let said = o
        .running()
        .apply(&Command::Sidechain(0, SidechainChange::Key(0)))
        .expect("off");
    assert_eq!(said, ["[1] bass: sidechain by itself"]);
    let own = run(&mut o, 0.3, 0.0, 100);
    assert!((own - 0.3).abs() < 3e-3, "its own key again: {own}");
}

#[test]
fn a_muted_key_keys_nothing_and_two_inputs_can_key_each_other() {
    let mut o = offline(PROFILE);
    o.running()
        .apply(&Command::Mute(Target::Input(1), true))
        .expect("mute");
    let muted = run(&mut o, 0.3, 0.3, 100);
    assert!(muted < 1e-3, "a muted key: {muted}");
    o.running()
        .apply(&Command::Mute(Target::Input(1), false))
        .expect("unmute");
    // The kick gets a gate that listens to the bass: the key is taken before the gates, so
    // neither locks the other out.
    o.running()
        .apply(&Command::Gate(1, 5.0))
        .expect("a gate for the kick");
    o.running()
        .apply(&Command::Sidechain(1, SidechainChange::Key(1)))
        .expect("the other way round");
    let both = run(&mut o, 0.3, 0.3, 100);
    assert!((both - 0.3).abs() < 3e-3, "both open: {both}");
    assert!(
        o.running().inputs[1]
            .controls
            .gate_open
            .load(std::sync::atomic::Ordering::Relaxed),
        "the gate of the kick too"
    );
}

#[test]
fn a_keyed_compressor_turns_down_by_the_other_input() {
    let text = PROFILE.replace("gate = 5", "comp = 10");
    let mut o = offline(&text);
    let step = |o: &mut Offline, kick: f32| {
        run(o, 0.02, kick, 100);
        o.running().inputs[0].controls.comp_reduction.take();
        run(o, 0.02, kick, 2);
        o.running().inputs[0].controls.comp_reduction.take()
    };
    // The bass is below the threshold of its compressor (-30 dB): alone it would not be touched,
    // and with a silent key it is not either.
    let alone = step(&mut o, 0.0);
    assert!(alone < 0.1, "nothing to take: {alone} dB");
    // The kick at -6 dB is 24 dB above it: seven eighths of that go.
    let under = step(&mut o, 0.5);
    assert!((under - 21.0).abs() < 1.0, "by the kick: {under} dB");
}

#[test]
fn a_high_pass_in_the_sidechain_keeps_the_bass_from_the_compressor() {
    let text = PROFILE
        .replace("gate = 5", "comp = 10")
        .replace("sidechain_by = 2\n", "");
    let mut o = offline(&text);
    let full = compressed(&mut o, 100.0, 100);
    assert!(full > 15.0, "a bass tone is compressed: {full} dB");
    let said = o
        .running()
        .apply(&Command::Sidechain(0, SidechainChange::Hpf(4000.0)))
        .expect("a high-pass");
    assert_eq!(said, ["[1] bass: sidechain hpf 4000 Hz"]);
    let spared = compressed(&mut o, 100.0, 100);
    assert!(spared < 0.1, "behind the high-pass it is not: {spared} dB");
    // What is above the corner still is.
    let top = compressed(&mut o, 8000.0, 100);
    assert!(top > 15.0, "a high tone is: {top} dB");
    // And a low-pass below it spares that too.
    o.running()
        .apply(&Command::Sidechain(0, SidechainChange::Hpf(20.0)))
        .expect("the high-pass off");
    o.running()
        .apply(&Command::Sidechain(0, SidechainChange::Lpf(500.0)))
        .expect("a low-pass");
    let spared = compressed(&mut o, 8000.0, 100);
    assert!(spared < 0.1, "behind the low-pass: {spared} dB");
}

#[test]
fn commands_refuse_what_does_not_fit_and_the_profile_keeps_what_was_set() {
    let mut o = offline(PROFILE);
    for bad in [
        Command::Sidechain(0, SidechainChange::Key(1)),
        Command::Sidechain(0, SidechainChange::Key(3)),
        Command::Sidechain(0, SidechainChange::Hpf(5.0)),
        Command::Sidechain(0, SidechainChange::Lpf(30_000.0)),
        Command::Sidechain(5, SidechainChange::Key(0)),
    ] {
        assert!(o.running().apply(&bad).is_err(), "{bad:?}");
    }
    o.running()
        .apply(&Command::Sidechain(0, SidechainChange::Hpf(300.0)))
        .expect("a high-pass");
    o.running()
        .apply(&Command::Sidechain(1, SidechainChange::Lpf(2000.0)))
        .expect("a low-pass");
    let listed = o.running().apply(&Command::Show).expect("a list");
    assert!(
        listed[0].contains("sidechain by [2] hpf 300 Hz"),
        "{listed:?}"
    );

    let now = o.running().current();
    assert_eq!(now.inputs[0].sidechain.key, 2);
    let text = profile::render(&now);
    assert!(text.contains("sidechain_by = 2"), "{text}");
    assert!(text.contains("sidechain_hpf_hz = 300"), "{text}");
    assert!(text.contains("sidechain_lpf_hz = 2000"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&now));
    let merged = profile::merge(PROFILE, &now);
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
    // Taken away again, the merged file loses the key.
    o.running()
        .apply(&Command::Sidechain(0, SidechainChange::Key(0)))
        .expect("off");
    let now = o.running().current();
    let merged = profile::merge(&merged, &now);
    assert!(!merged.contains("sidechain_by"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
}

#[test]
fn a_profile_refuses_a_key_or_a_corner_that_does_not_fit() {
    for (bad, why) in [
        ("sidechain_by = 1", "itself"),
        ("sidechain_by = 7", "no input 7"),
        ("sidechain_by = 2\nsidechain_hpf_hz = 3", "high-pass"),
        ("sidechain_by = 2\nsidechain_lpf_hz = 50", "low-pass"),
    ] {
        let text = PROFILE.replace("sidechain_by = 2", bad);
        let said = match profile::parse(&text) {
            Err(error) => error.0,
            Ok(spec) => spec.validate().expect_err("refused").to_string(),
        };
        assert!(said.contains(why), "{bad}: {said}");
    }
}

#[test]
fn the_desk_shows_it_and_takes_its_actions() {
    let spec = profile::parse(PROFILE).expect("the profile is valid");
    let offline = Offline::new(&spec, RATE, 1).expect("the route builds");
    let mut desk = EngineDesk::new(offline.into_running(), None);
    assert_eq!(desk.view().inputs[0].sidechain.key, 2);
    desk.act(pult_core::view::Action::InputSidechain {
        input: 0,
        change: SidechainChange::Lpf(900.0),
    })
    .expect("a low-pass");
    let view = desk.view();
    assert!((view.inputs[0].sidechain.lpf_hz - 900.0).abs() < f32::EPSILON);
    assert!(view.inputs[1].sidechain.is_neutral());
}
