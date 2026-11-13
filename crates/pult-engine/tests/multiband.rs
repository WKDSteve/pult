//! The multiband compressor in the whole route, rendered without devices: only the band that is
//! set compresses, set by commands, shown, and kept by the profile.

use pult_core::control::Command;
use pult_core::multiband::MultibandChange;
use pult_engine::{Offline, profile};

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Stream"
device = "offline stream"
limit_on = false
mb_low = 8

[[input]]
device = "music"
buses = ["Stream"]
"#;

/// The peak of the last of `callbacks` callbacks of a sine of `hz` at `level`.
fn last(o: &mut Offline, hz: f32, level: f32, callbacks: usize) -> f32 {
    let step = std::f32::consts::TAU * hz / 48_000.0;
    let mut out = 0.0;
    let mut n = 0_u32;
    for _ in 0..callbacks {
        let feed: Vec<f32> = (0..480)
            .map(|_| {
                n = (n + 1) % 48_000;
                #[allow(clippy::cast_precision_loss)]
                let s = level * (step * n as f32).sin();
                s
            })
            .collect();
        out = o.process(&[&feed], 480)[0]
            .iter()
            .fold(0.0_f32, |m, s| m.max(s.abs()));
    }
    out
}

#[test]
fn only_the_band_that_is_set_compresses() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert_eq!(spec.buses[0].multiband.bands, [8.0, 0.0, 0.0]);
    let mut bass = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let squeezed = last(&mut bass, 100.0, 0.9, 200);
    // The knob makes up for what it takes, so compare with a route that leaves the bass alone.
    let mut plain_spec = spec.clone();
    plain_spec.buses[0].multiband.bands = [0.0; 3];
    let mut plain = Offline::new(&plain_spec, 48_000, 1).expect("the route builds");
    let untouched = last(&mut plain, 100.0, 0.9, 200);
    assert!((untouched - 0.9).abs() < 5e-3, "off: {untouched}");
    assert!(
        (squeezed - untouched).abs() > 0.02,
        "the low band works on the bass: {squeezed} against {untouched}"
    );
    // A voice is in the middle band, which is off: it passes at its level.
    let mut voice = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let passed = last(&mut voice, 1000.0, 0.9, 200);
    assert!((passed - 0.9).abs() < 0.02, "the middle passes: {passed}");
}

#[test]
fn commands_set_it_and_refuse_what_does_not_fit() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let said = o
        .running()
        .apply(&Command::Multiband(0, MultibandChange::Band(1, 3.0)))
        .expect("a band");
    assert!(said[0].contains("multiband mid 3.0"), "{said:?}");
    let said = o
        .running()
        .apply(&Command::Multiband(0, MultibandChange::HighSplit(3000.0)))
        .expect("a split");
    assert!(said[0].contains("split high 3000 Hz"), "{said:?}");
    for bad in [
        Command::Multiband(0, MultibandChange::Band(0, 11.0)),
        Command::Multiband(0, MultibandChange::Band(3, 1.0)),
        Command::Multiband(0, MultibandChange::LowSplit(5.0)),
        Command::Multiband(4, MultibandChange::Band(0, 1.0)),
    ] {
        assert!(o.running().apply(&bad).is_err(), "{bad:?}");
    }
    let listed = o.running().apply(&Command::Show).expect("a list");
    assert!(
        listed.iter().any(|line| line.contains("mb 8.0/3.0/off")),
        "{listed:?}"
    );
}

#[test]
fn a_profile_keeps_the_multiband_compressor() {
    let mut spec = profile::parse(PROFILE).expect("valid profile");
    spec.buses[0].multiband.bands[2] = 2.5;
    spec.buses[0].multiband.low_hz = 150.0;
    assert_eq!(profile::parse(&profile::render(&spec)).as_ref(), Ok(&spec));
    let merged = profile::merge(PROFILE, &spec);
    assert!(merged.contains("mb_low_hz = 150"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
    let bad = PROFILE.replace("mb_low = 8", "mb_high_hz = 300");
    assert!(profile::parse(&bad).unwrap_err().0.contains("high split"));
}
