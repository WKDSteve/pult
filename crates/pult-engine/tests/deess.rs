//! The de-esser of an input in the whole route, rendered without devices: it takes the band of
//! the sibilants down and leaves the voice, set by the profile and by commands, shown, and kept.

use pult_core::control::Command;
use pult_core::deess::DeessChange;
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
device = "voice"
buses = ["A1"]
deess_db = -30
deess_hz = 5000

[[input]]
device = "other"
buses = ["A1"]
"#;

fn offline(text: &str) -> Offline {
    let spec = profile::parse(text).expect("the profile is valid");
    Offline::new(&spec, RATE, 1).expect("the route builds")
}

/// Runs `callbacks` of a sine of `hz` at half scale into the first input; returns the peak it
/// put into the mix in the last of them and the most its de-esser took there, in dB.
fn played(offline: &mut Offline, hz: f32, callbacks: usize) -> (f32, f32) {
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
        offline.running().inputs[0].controls.deess_reduction.take();
        offline.running().inputs[0].routes[0].stats.peak.take();
        offline.process(&[&tone, &silence], PERIOD);
        taken = offline.running().inputs[0].controls.deess_reduction.take();
    }
    let peak = offline.running().inputs[0].routes[0].stats.peak.take();
    (peak, taken)
}

#[test]
fn a_sibilant_goes_down_and_the_voice_does_not() {
    let mut o = offline(PROFILE);
    let (hiss, taken) = played(&mut o, 7300.0, 100);
    assert!(hiss < 0.13, "the sibilant in the mix: {hiss}");
    assert!(taken > 12.0, "what the de-esser took: {taken} dB");
    let (voice, taken) = played(&mut o, 300.0, 100);
    assert!((voice - 0.5).abs() < 0.01, "the voice in the mix: {voice}");
    assert!(taken < 0.1, "nothing to take: {taken} dB");
    // Off, the sibilant passes as it came.
    let said = o
        .running()
        .apply(&Command::Deess(0, DeessChange::Threshold(0.0)))
        .expect("off");
    assert_eq!(said, ["[1] voice: deess off"]);
    let (hiss, taken) = played(&mut o, 7300.0, 100);
    assert!((hiss - 0.5).abs() < 0.01, "untouched: {hiss}");
    assert!(taken.abs() < f32::EPSILON, "{taken}");
}

#[test]
fn the_corner_decides_what_counts_as_a_sibilant() {
    let mut o = offline(PROFILE);
    // 3 kHz is below the corner at 5 kHz.
    let (below, _) = played(&mut o, 3000.0, 100);
    assert!(below > 0.4, "below the corner: {below}");
    let said = o
        .running()
        .apply(&Command::Deess(0, DeessChange::Freq(2000.0)))
        .expect("a lower corner");
    assert_eq!(said, ["[1] voice: deess above 2000 Hz"]);
    let (above, taken) = played(&mut o, 3000.0, 100);
    assert!(above < 0.2, "above it now: {above}");
    assert!(taken > 8.0, "{taken} dB");
}

#[test]
fn commands_refuse_what_does_not_fit_and_the_profile_keeps_what_was_set() {
    let mut o = offline(PROFILE);
    for bad in [
        Command::Deess(0, DeessChange::Threshold(3.0)),
        Command::Deess(0, DeessChange::Threshold(-90.0)),
        Command::Deess(0, DeessChange::Freq(500.0)),
        Command::Deess(0, DeessChange::Freq(f32::NAN)),
        Command::Deess(5, DeessChange::Threshold(-20.0)),
    ] {
        assert!(o.running().apply(&bad).is_err(), "{bad:?}");
    }
    o.running()
        .apply(&Command::Deess(1, DeessChange::Threshold(-24.0)))
        .expect("a threshold");
    let listed = o.running().apply(&Command::Show).expect("a list");
    assert!(
        listed[0].contains("deess -30 dB above 5000 Hz"),
        "{listed:?}"
    );
    assert!(
        listed[1].contains("deess -24 dB above 6000 Hz"),
        "{listed:?}"
    );

    let now = o.running().current();
    let text = profile::render(&now);
    assert!(text.contains("deess_db = -30"), "{text}");
    assert!(text.contains("deess_hz = 5000"), "{text}");
    assert!(text.contains("deess_db = -24"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&now));
    let merged = profile::merge(PROFILE, &now);
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
    // Switched off again, the merged file loses the threshold and keeps the corner.
    o.running()
        .apply(&Command::Deess(0, DeessChange::Threshold(0.0)))
        .expect("off");
    let now = o.running().current();
    let merged = profile::merge(&merged, &now);
    assert!(!merged.contains("deess_db = -30"), "{merged}");
    assert!(merged.contains("deess_hz = 5000"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
}

#[test]
fn a_profile_refuses_a_number_that_does_not_fit() {
    for (bad, why) in [
        ("deess_db = 4", "threshold"),
        ("deess_db = -30\ndeess_hz = 90", "frequency"),
    ] {
        let text = PROFILE.replace("deess_db = -30\ndeess_hz = 5000", bad);
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
    assert!(desk.view().inputs[0].deess.is_on());
    desk.act(pult_core::view::Action::InputDeess {
        input: 0,
        change: DeessChange::Freq(9000.0),
    })
    .expect("a corner");
    let view = desk.view();
    assert!((view.inputs[0].deess.freq_hz - 9000.0).abs() < f32::EPSILON);
    assert!(!view.inputs[1].deess.is_on());
}
