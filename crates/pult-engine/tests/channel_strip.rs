//! The channel strip of an input in the whole route, rendered without devices: polarity and
//! filters before the gate, set by commands, shown, and kept by the profile.

use pult_core::control::Command;
use pult_core::strip::StripChange;
use pult_engine::{Offline, profile};

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Main"
device = "offline main"
limit_on = false

[[input]]
device = "mic"
buses = ["Main"]
polarity = true
"#;

/// The last sample of `callbacks` callbacks of the constant `level`.
fn last(o: &mut Offline, level: f32, callbacks: usize) -> f32 {
    let feed = vec![level; 480];
    let mut out = 0.0;
    for _ in 0..callbacks {
        out = *o.process(&[&feed], 480)[0].last().expect("a callback");
    }
    out
}

#[test]
fn polarity_and_the_high_pass_work_on_the_input() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert!(spec.inputs[0].strip.polarity);
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let turned = last(&mut o, 0.5, 100);
    assert!((turned + 0.5).abs() < 1e-3, "turned over: {turned}");

    o.running()
        .apply(&Command::Strip(0, StripChange::Polarity(false)))
        .expect("polarity back");
    let back = last(&mut o, 0.5, 100);
    assert!((back - 0.5).abs() < 1e-3, "as it comes: {back}");

    // A constant level is as low as a sound gets: the high-pass takes it away.
    let shown = o
        .running()
        .apply(&Command::Strip(0, StripChange::Hpf(80.0)))
        .expect("a high-pass");
    assert!(shown[0].contains("high-pass 80 Hz"), "{shown:?}");
    let filtered = last(&mut o, 0.5, 200);
    assert!(
        filtered.abs() < 1e-3,
        "no DC through a high-pass: {filtered}"
    );

    assert!(
        o.running()
            .apply(&Command::Strip(0, StripChange::Lpf(100.0)))
            .is_err()
    );
    assert!(
        o.running()
            .apply(&Command::Strip(3, StripChange::Delay(1.0)))
            .is_err()
    );
    let listed = o.running().apply(&Command::Show).expect("a list");
    assert!(listed[0].contains("hpf 80 Hz"), "{listed:?}");
}

#[test]
fn a_profile_keeps_the_channel_strip() {
    let mut spec = profile::parse(PROFILE).expect("valid profile");
    spec.inputs[0].strip.hpf_hz = 100.0;
    spec.inputs[0].strip.lpf_hz = 12_000.0;
    spec.inputs[0].strip.delay_ms = 12.5;
    spec.inputs[0].strip.width = 0.0;
    assert_eq!(profile::parse(&profile::render(&spec)).as_ref(), Ok(&spec));
    let merged = profile::merge(PROFILE, &spec);
    assert!(merged.contains("delay_ms = 12.5"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
    // Back to neutral: a switch the file had says so, the numbers go.
    spec.inputs[0].strip = pult_core::strip::StripSettings::default();
    let merged = profile::merge(PROFILE, &spec);
    assert!(merged.contains("polarity = false"), "{merged}");
    assert!(!merged.contains("hpf_hz"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
    let bad = PROFILE.replace("polarity = true", "width = 300");
    assert!(profile::parse(&bad).unwrap_err().0.contains("width"));
}
