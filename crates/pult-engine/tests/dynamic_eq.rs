//! Dynamic bands of the equalizers in the whole route, rendered without devices: a band of an
//! input and one of a bus that have their gain only above a threshold, set by the profile and
//! by commands, and kept.

use pult_core::control::Command;
use pult_core::eq::BandChange;
use pult_core::view::Desk as _;
use pult_engine::{EngineDesk, Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "A1"
device = "offline A1"
limit_on = false

[[input]]
device = "voice"
buses = ["A1"]

[[input.eq]]
band = 2
kind = "bell"
freq_hz = 1000
gain_db = -12
q = 1
threshold_db = -30
"#;

fn offline(text: &str) -> Offline {
    let spec = profile::parse(text).expect("the profile is valid");
    Offline::new(&spec, RATE, 1).expect("the route builds")
}

/// Runs `callbacks` of a sine of `hz` at `amplitude` into the input; returns what the route did
/// to its level in the last of them, in dB.
fn changed(offline: &mut Offline, hz: f32, amplitude: f32, callbacks: usize) -> f32 {
    let step = std::f32::consts::TAU * hz / RATE as f32;
    let mut phase = 0.0_f32;
    let mut peak = 0.0_f32;
    for _ in 0..callbacks {
        let tone: Vec<f32> = (0..PERIOD)
            .map(|_| {
                phase = (phase + step) % std::f32::consts::TAU;
                amplitude * phase.sin()
            })
            .collect();
        let out = offline.process(&[&tone], PERIOD);
        peak = out[0].iter().fold(0.0, |most, s| most.max(s.abs()));
    }
    20.0 * (peak / amplitude).log10()
}

#[test]
fn a_dynamic_band_of_an_input_cuts_only_what_is_loud() {
    let mut o = offline(PROFILE);
    let loud = changed(&mut o, 1000.0, 0.5, 150);
    assert!((loud + 12.0).abs() < 0.5, "loud: {loud} dB");
    let quiet = changed(&mut o, 1000.0, 0.01, 150);
    assert!(quiet.abs() < 0.2, "quiet: {quiet} dB");
    // Loud somewhere else does not bring it in.
    let bass = changed(&mut o, 100.0, 0.5, 150);
    assert!(bass.abs() < 0.2, "a bass: {bass} dB");
    // Without its threshold it is a plain band: the quiet tone goes down too.
    o.running()
        .apply(&Command::InputEqBand(0, 1, BandChange::Threshold(0.0)))
        .expect("the threshold off");
    let plain = changed(&mut o, 1000.0, 0.01, 150);
    assert!((plain + 12.0).abs() < 0.5, "plain: {plain} dB");
}

#[test]
fn a_band_of_a_bus_is_dynamic_too() {
    let mut o = offline(&PROFILE.replace("threshold_db = -30\n", ""));
    // The input cuts 12 dB always now; the bus gives 6 back, but only to what is loud.
    for change in [
        BandChange::Freq(1000.0),
        BandChange::Gain(6.0),
        BandChange::Threshold(-40.0),
    ] {
        o.running()
            .apply(&Command::EqBand(0, 1, change))
            .expect("a band of the bus");
    }
    // -6 dBFS in, -18 behind the input: 22 dB above the threshold of the bus.
    let loud = changed(&mut o, 1000.0, 0.5, 150);
    assert!((loud + 6.0).abs() < 0.5, "loud: {loud} dB");
    // -46 dBFS in, -58 behind the input: below it.
    let quiet = changed(&mut o, 1000.0, 0.005, 150);
    assert!((quiet + 12.0).abs() < 0.5, "quiet: {quiet} dB");
}

#[test]
fn commands_refuse_what_does_not_fit_and_the_profile_keeps_what_was_set() {
    let mut o = offline(PROFILE);
    for bad in [
        Command::InputEqBand(0, 1, BandChange::Threshold(2.0)),
        Command::InputEqBand(0, 1, BandChange::Threshold(-80.0)),
        Command::EqBand(0, 9, BandChange::Threshold(-20.0)),
    ] {
        assert!(o.running().apply(&bad).is_err(), "{bad:?}");
    }
    o.running()
        .apply(&Command::EqBand(0, 3, BandChange::Gain(-4.0)))
        .expect("a gain");
    o.running()
        .apply(&Command::EqBand(0, 3, BandChange::Threshold(-24.0)))
        .expect("a threshold");

    let now = o.running().current();
    assert!((now.inputs[0].eq[1].threshold_db + 30.0).abs() < f32::EPSILON);
    assert!((now.buses[0].eq[3].threshold_db + 24.0).abs() < f32::EPSILON);
    let text = profile::render(&now);
    assert!(text.contains("threshold_db = -30"), "{text}");
    assert!(text.contains("threshold_db = -24"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&now));
    let merged = profile::merge(PROFILE, &now);
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
    // Taken away again, the merged file loses the key.
    o.running()
        .apply(&Command::InputEqBand(0, 1, BandChange::Threshold(0.0)))
        .expect("off");
    let now = o.running().current();
    let merged = profile::merge(&merged, &now);
    assert!(!merged.contains("threshold_db = -30"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
}

#[test]
fn a_profile_refuses_a_threshold_that_does_not_fit() {
    let text = PROFILE.replace("threshold_db = -30", "threshold_db = 5");
    let said = profile::parse(&text).expect_err("refused").0;
    assert!(said.contains("threshold"), "{said}");
}

#[test]
fn the_desk_shows_it_and_takes_its_actions() {
    let spec = profile::parse(PROFILE).expect("the profile is valid");
    let offline = Offline::new(&spec, RATE, 1).expect("the route builds");
    let mut desk = EngineDesk::new(offline.into_running(), None);
    assert!(desk.view().inputs[0].eq[1].is_dynamic());
    desk.act(pult_core::view::Action::InputEqBand {
        input: 0,
        band: 1,
        change: BandChange::Threshold(-18.0),
    })
    .expect("a threshold");
    let view = desk.view();
    assert!((view.inputs[0].eq[1].threshold_db + 18.0).abs() < f32::EPSILON);
    assert!(!view.buses[0].eq[1].is_dynamic());
}
