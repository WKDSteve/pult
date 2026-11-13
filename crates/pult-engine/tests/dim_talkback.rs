//! Dim and talkback in the whole route, rendered without devices: a dimmed bus is down by its
//! dim level, a talkback microphone is silent until someone talks, and talking dims the monitor
//! bus but not the others.

use pult_core::control::Command;
use pult_engine::{Offline, profile};

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Room"
device = "offline room"
limit_on = false
monitor = true
dim_db = 12

[[bus]]
name = "Cue"
device = "offline cue"
limit_on = false

[[input]]
device = "music"
buses = ["Room", "Cue"]

[[input]]
device = "talk mic"
buses = ["Cue"]
talkback = true
"#;

/// The last sample of both buses after `callbacks` callbacks of music at 0.5 and the talkback
/// microphone at 0.25.
fn last(o: &mut Offline, callbacks: usize) -> (f32, f32) {
    let (music, mic) = (vec![0.5; 480], vec![0.25; 480]);
    let mut out = (0.0, 0.0);
    for _ in 0..callbacks {
        let outs = o.process(&[&music, &mic], 480);
        out = (
            *outs[0].last().expect("a callback"),
            *outs[1].last().expect("a callback"),
        );
    }
    out
}

fn near(got: f32, wanted: f32) -> bool {
    (got - wanted).abs() < 2e-3
}

#[test]
fn a_dimmed_bus_is_down_by_its_level_and_comes_back() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert!((spec.buses[0].dim_db - 12.0).abs() < f32::EPSILON);
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let (room, cue) = last(&mut o, 60);
    assert!(near(room, 0.5) && near(cue, 0.5), "{room} {cue}");

    let said = o.running().apply(&Command::Dim(0, true)).expect("dim");
    assert!(said[0].contains("dimmed"), "{said:?}");
    let (room, cue) = last(&mut o, 60);
    // 12 dB down is a quarter.
    assert!(near(room, 0.5 * 0.251), "dimmed: {room}");
    assert!(near(cue, 0.5), "the other bus stays: {cue}");

    // The fader moves under the dim, and the level follows the new dim level.
    o.running()
        .apply(&Command::DimLevel(0, 6.0))
        .expect("a level");
    let (room, _) = last(&mut o, 60);
    assert!(near(room, 0.5 * 0.501), "6 dB: {room}");
    assert!(o.running().apply(&Command::DimLevel(0, 90.0)).is_err());
    assert!(o.running().apply(&Command::Dim(7, true)).is_err());

    o.running().apply(&Command::Dim(0, false)).expect("back");
    let (room, _) = last(&mut o, 60);
    assert!(near(room, 0.5), "back: {room}");
}

#[test]
fn a_talkback_microphone_is_silent_until_someone_talks_and_talking_dims_the_monitor() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert!(spec.inputs[1].talkback);
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let (room, cue) = last(&mut o, 60);
    assert!(near(room, 0.5), "{room}");
    assert!(near(cue, 0.5), "only the music: {cue}");

    let said = o.running().apply(&Command::Talk(true)).expect("talk");
    assert_eq!(said, ["talk on"]);
    let (room, cue) = last(&mut o, 60);
    assert!(near(room, 0.5 * 0.251), "the monitor is dimmed: {room}");
    assert!(near(cue, 0.75), "music and the microphone: {cue}");
    let listed = o.running().apply(&Command::Show).expect("a list");
    assert!(listed[1].contains("talkback (open)"), "{listed:?}");
    assert!(listed[2].contains("dimmed 12 dB"), "{listed:?}");

    o.running().apply(&Command::Talk(false)).expect("talk off");
    let (room, cue) = last(&mut o, 60);
    assert!(near(room, 0.5) && near(cue, 0.5), "{room} {cue}");

    // A plain input again: it sounds without anyone talking.
    o.running()
        .apply(&Command::Talkback(1, false))
        .expect("no talkback");
    let (_, cue) = last(&mut o, 60);
    assert!(near(cue, 0.75), "{cue}");
    // Its own mute still counts while someone talks.
    o.running()
        .apply(&Command::Talkback(1, true))
        .expect("talkback");
    o.running()
        .apply(&Command::Mute(pult_core::control::Target::Input(1), true))
        .expect("mute");
    o.running().apply(&Command::Talk(true)).expect("talk");
    let (_, cue) = last(&mut o, 60);
    assert!(near(cue, 0.5), "muted stays muted: {cue}");
}

#[test]
fn a_profile_keeps_the_dim_level_and_the_talkback() {
    let mut spec = profile::parse(PROFILE).expect("valid profile");
    spec.buses[1].dim_db = 30.0;
    assert_eq!(profile::parse(&profile::render(&spec)).as_ref(), Ok(&spec));
    let merged = profile::merge(PROFILE, &spec);
    assert!(merged.contains("dim_db = 30"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
    let bad = PROFILE.replace("dim_db = 12", "dim_db = 70");
    assert!(profile::parse(&bad).unwrap_err().0.contains("dim level"));
}
