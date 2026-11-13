//! A monitor bus, rendered without devices: solo acts only there, and a talk bus keeps hearing
//! every source.

use std::sync::atomic::Ordering;

use pult_core::control::{Command, Target};
use pult_engine::{Offline, profile};

const PERIOD: usize = 480;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Talk"
device = "offline talk"

[[bus]]
name = "Phones"
device = "offline phones"
monitor = true

[[input]]
device = "mic"
buses = ["Talk", "Phones"]

[[input]]
device = "music"
buses = ["Talk", "Phones"]
"#;

/// The last samples of both buses after `callbacks`, the mic at 0.25 and the music at 0.5.
fn run(offline: &mut Offline, callbacks: usize) -> (f32, f32) {
    let (mic, music) = (vec![0.25_f32; PERIOD], vec![0.5_f32; PERIOD]);
    let mut last = (0.0, 0.0);
    for _ in 0..callbacks {
        let outs = offline.process(&[&mic, &music], PERIOD);
        last = (outs[0][PERIOD - 1], outs[1][PERIOD - 1]);
    }
    last
}

#[test]
fn solo_silences_the_others_on_the_monitor_bus_only() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert!(spec.buses[1].monitor && !spec.buses[0].monitor);
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    run(&mut o, 60);
    // The mic soloed: the desk silences the music as it does (soloed_out).
    o.running().inputs[1]
        .controls
        .soloed_out
        .store(true, Ordering::Release);
    let (talk, phones) = run(&mut o, 40);
    assert!(
        (talk - 0.75).abs() < 2e-3,
        "the talk bus hears both: {talk}"
    );
    assert!(
        (phones - 0.25).abs() < 2e-3,
        "the monitor hears the solo: {phones}"
    );

    // Without a monitor bus, solo acts on every bus, as it always did.
    o.running()
        .apply(&Command::Monitor(1, false))
        .expect("a bus");
    let (talk, phones) = run(&mut o, 40);
    assert!((talk - 0.25).abs() < 2e-3, "{talk}");
    assert!((phones - 0.25).abs() < 2e-3, "{phones}");

    // A saved profile keeps the monitor.
    let text = profile::render(&spec);
    assert!(text.contains("monitor = true"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&spec));
}

#[test]
fn pfl_hears_a_soloed_input_before_its_fader_on_the_monitor_bus_only() {
    let spec = profile::parse(&format!("pfl = true\n{PROFILE}")).expect("valid profile");
    assert!(spec.pfl);
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    // The mic far down and muted, and soloed as the desk does it.
    o.running()
        .apply(&Command::Gain(Target::Input(0), -40.0, 0))
        .expect("a gain");
    o.running()
        .apply(&Command::Mute(Target::Input(0), true))
        .expect("a mute");
    let solo = |o: &mut Offline, on: bool| {
        let inputs = &o.running().inputs;
        inputs[0].controls.soloed.store(on, Ordering::Release);
        inputs[1].controls.soloed_out.store(on, Ordering::Release);
    };
    solo(&mut o, true);
    let (talk, phones) = run(&mut o, 60);
    assert!(
        (talk - 0.5).abs() < 2e-3,
        "the talk bus: music only: {talk}"
    );
    assert!(
        (phones - 0.25).abs() < 2e-3,
        "the monitor hears the mic at unity: {phones}"
    );

    // After the fader (AFL): the muted mic is silent there too.
    let said = o.running().apply(&Command::Pfl(false)).expect("afl");
    assert!(said[0].contains("AFL"), "{said:?}");
    let (_, phones) = run(&mut o, 40);
    assert!(phones.abs() < 2e-3, "afl: {phones}");

    // Back to PFL, and the solo ends: everything as the faders say.
    o.running().apply(&Command::Pfl(true)).expect("pfl");
    solo(&mut o, false);
    let (talk, phones) = run(&mut o, 40);
    assert!((talk - 0.5).abs() < 2e-3, "{talk}");
    assert!((phones - 0.5).abs() < 2e-3, "{phones}");

    // A profile keeps it.
    let spec = o.running().current();
    let text = profile::render(&spec);
    assert!(text.contains("pfl = true"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&spec));
    let merged = profile::merge(PROFILE, &spec);
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
}
