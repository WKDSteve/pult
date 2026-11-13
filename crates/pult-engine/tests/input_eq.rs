//! The equalizer and the limiter of an input in the whole route, rendered without devices: set by
//! the profile and by commands, before or behind gate and compressor, shown, and kept.

use pult_core::control::Command;
use pult_core::eq::BandChange;
use pult_core::limiter::LimitChange;
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
limit_on = true
limit_db = -12

[[input.eq]]
band = 1
gain_db = 6
"#;

/// The last sample of `callbacks` callbacks of the constant `level` (as low as a sound gets: the
/// low shelf of band 1 acts on it).
fn last(o: &mut Offline, level: f32, callbacks: usize) -> f32 {
    let feed = vec![level; 480];
    let mut out = 0.0;
    for _ in 0..callbacks {
        out = *o.process(&[&feed], 480)[0].last().expect("a callback");
    }
    out
}

fn db(linear: f32) -> f32 {
    20.0 * linear.log10()
}

#[test]
fn the_equalizer_boosts_and_the_limiter_holds_the_input() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert!((spec.inputs[0].eq[0].gain_db - 6.0).abs() < f32::EPSILON);
    assert!(spec.inputs[0].limit_on);
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    // Quiet: 6 dB up, far below the ceiling.
    let quiet = last(&mut o, 0.05, 200);
    assert!((db(quiet) - db(0.05) - 6.0).abs() < 0.2, "boosted: {quiet}");
    // Loud: the limiter of the input holds -12 dB.
    let loud = last(&mut o, 0.5, 200);
    assert!((db(loud) + 12.0).abs() < 0.3, "at its ceiling: {loud}");

    let said = o
        .running()
        .apply(&Command::InputLimit(0, LimitChange::Switch(false)))
        .expect("the limiter off");
    assert!(said[0].contains("limit off"), "{said:?}");
    let free = last(&mut o, 0.5, 200);
    assert!(
        (db(free) - db(0.5) - 6.0).abs() < 0.2,
        "only boosted: {free}"
    );

    o.running()
        .apply(&Command::InputEq(0, false))
        .expect("the equalizer off");
    let flat = last(&mut o, 0.5, 200);
    assert!((flat - 0.5).abs() < 2e-3, "as it comes: {flat}");
}

#[test]
fn commands_set_bands_and_the_place_and_refuse_what_does_not_fit() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let said = o
        .running()
        .apply(&Command::InputEqBand(0, 2, BandChange::Gain(-4.5)))
        .expect("a band");
    assert!(
        said[0].contains("eq 3:") && said[0].contains("-4.5 dB"),
        "{said:?}"
    );
    let said = o
        .running()
        .apply(&Command::InputEqPre(0, true))
        .expect("before the compressor");
    assert!(
        said[0].contains("eq before gate and compressor"),
        "{said:?}"
    );
    for bad in [
        Command::InputEqBand(0, 7, BandChange::Gain(1.0)),
        Command::InputEqBand(0, 0, BandChange::Gain(99.0)),
        Command::InputLimit(0, LimitChange::Ceiling(3.0)),
        Command::InputLimit(0, LimitChange::Lookahead(1.0)),
        Command::InputEq(5, true),
    ] {
        assert!(o.running().apply(&bad).is_err(), "{bad:?}");
    }
    let listed = o.running().apply(&Command::Show).expect("a list");
    assert!(listed[0].contains("eq pre"), "{listed:?}");
    assert!(listed[0].contains("limit -12.0 dB"), "{listed:?}");

    // What was set is what a saved profile holds, and what it reads back.
    let now = o.running().current();
    assert!(now.inputs[0].eq_pre);
    assert!((now.inputs[0].eq[2].gain_db + 4.5).abs() < f32::EPSILON);
    let text = profile::render(&now);
    assert!(text.contains("eq_pre = true"), "{text}");
    assert!(text.contains("[[input.eq]]"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&now));
    let merged = profile::merge(PROFILE, &now);
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&now));
}

#[test]
fn a_profile_refuses_a_ceiling_or_a_band_out_of_range() {
    let bad = PROFILE.replace("limit_db = -12", "limit_db = 6");
    assert!(profile::parse(&bad).unwrap_err().0.contains("ceiling"));
    let bad = PROFILE.replace("band = 1", "band = 9");
    assert!(profile::parse(&bad).is_err());
}
