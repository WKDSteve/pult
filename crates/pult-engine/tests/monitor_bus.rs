//! A monitor bus, rendered without devices: solo acts only there, and a talk bus keeps hearing
//! every source.

use std::sync::atomic::Ordering;

use pult_core::control::Command;
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
