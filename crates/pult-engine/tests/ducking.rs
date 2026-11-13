//! Ducking in the whole route, rendered without devices: the music gets quieter while the
//! microphone sounds, and comes back after it.

use pult_core::control::Command;
use pult_core::duck::{DuckChange, DuckSettings};
use pult_core::view::Desk as _;
use pult_engine::{EngineDesk, Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;
const CHANNELS: u16 = 1;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "A1"
device = "offline A1"

[[input]]
device = "music"
buses = ["A1"]
duck_by = 2

[[input]]
device = "mic"
buses = ["A1"]
"#;

fn offline(text: &str) -> Offline {
    let spec = profile::parse(text).expect("the profile is valid");
    Offline::new(&spec, RATE, CHANNELS).expect("the route builds")
}

/// Runs `callbacks` with the constant levels `music` and `mic`; returns what the music put into
/// the mix in the last callback, in dB against what it delivered.
fn run(offline: &mut Offline, music: f32, mic: f32, callbacks: usize) -> f32 {
    let feeds = [vec![music; PERIOD], vec![mic; PERIOD]];
    let refs: Vec<&[f32]> = feeds.iter().map(Vec::as_slice).collect();
    for _ in 0..callbacks {
        offline.running().inputs[0].routes[0].stats.peak.take();
        offline.process(&refs, PERIOD);
    }
    let peak = offline.running().inputs[0].routes[0].stats.peak.take();
    20.0 * (peak / music).log10()
}

#[test]
fn the_music_is_down_while_the_microphone_sounds_and_back_after_it() {
    let mut o = offline(PROFILE);
    let alone = run(&mut o, 0.2, 0.0, 100);
    assert!(alone.abs() < 0.05, "nothing ducks it yet: {alone} dB");
    // The key alone in the profile: 12 dB.
    let under = run(&mut o, 0.2, 0.3, 100);
    assert!(
        (under + 12.0).abs() < 0.1,
        "under the microphone: {under} dB"
    );
    assert!(
        (o.running().inputs[0].controls.duck_reduction.take() - 12.0).abs() < 0.1,
        "the stage says how much it takes"
    );
    // Silence: 400 ms of hold, 800 ms of release, and the latency of the ring.
    let back = run(&mut o, 0.2, 0.0, 200);
    assert!(back.abs() < 0.05, "back after it: {back} dB");
}

#[test]
fn the_turn_down_never_clicks() {
    let mut o = offline(&PROFILE.replace("duck_by = 2", "duck_by = 2\nduck_db = 40"));
    let step = std::f32::consts::TAU * 440.0 / RATE as f32;
    let mut phase = 0.0_f32;
    let mut played = Vec::new();
    for callback in 0..400 {
        let music: Vec<f32> = (0..PERIOD)
            .map(|_| {
                phase = (phase + step) % std::f32::consts::TAU;
                0.5 * phase.sin()
            })
            .collect();
        // The microphone speaks in bursts of half a second.
        let mic = vec![if (callback / 50) % 2 == 1 { 0.3 } else { 0.0 }; PERIOD];
        // Only the music is heard: the microphone keys, muted on the bus by its send.
        let out = o.process(&[&music, &mic], PERIOD)[0].clone();
        if callback == 0 {
            o.running()
                .apply(&Command::Send(1, 0, -96.0, 0))
                .expect("the microphone off the bus");
        }
        played.extend_from_slice(&out);
    }
    let peak = played.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(peak > 0.4, "the music played: {peak}");
    let quietest = played[played.len() - 30 * PERIOD..played.len() - 20 * PERIOD]
        .iter()
        .fold(0.0_f32, |peak, s| peak.max(s.abs()));
    assert!(quietest < 0.02, "and was ducked by 40 dB: {quietest}");
    let clicks = pult_core::Clicks::for_tone(440.0, RATE).find(&played, 1);
    assert!(
        clicks.is_empty(),
        "ducking a sine by 40 dB clicks: {} times, first at frame {}",
        clicks.len(),
        clicks[0].frame
    );
}

#[test]
fn a_muted_microphone_ducks_nothing_and_two_inputs_can_duck_each_other() {
    let mut o = offline(PROFILE);
    o.running()
        .apply(&Command::Mute(pult_core::control::Target::Input(1), true))
        .expect("mute");
    let under = run(&mut o, 0.2, 0.3, 100);
    assert!(under.abs() < 0.05, "a muted key: {under} dB");
    o.running()
        .apply(&Command::Mute(pult_core::control::Target::Input(1), false))
        .expect("unmute");
    // The microphone ducks by the music too: the key is taken before the ducker, so both are
    // down by their depth and neither lets the other back up.
    o.running()
        .apply(&Command::Duck(1, DuckChange::Key(1)))
        .expect("the other way round");
    let under = run(&mut o, 0.2, 0.3, 100);
    assert!((under + 12.0).abs() < 0.1, "{under} dB");
    let mic = o.running().inputs[1].controls.duck_reduction.take();
    assert!(
        (mic - 12.0).abs() < 0.1,
        "the microphone is down too: {mic}"
    );
}

#[test]
fn commands_set_it_while_the_route_runs_and_refuse_what_does_not_fit() {
    let mut o = offline(PROFILE);
    let said = o
        .running()
        .apply(&Command::Duck(0, DuckChange::Depth(20.0)))
        .expect("a depth");
    assert_eq!(said, ["[1] music: duck depth 20.0 dB"]);
    let under = run(&mut o, 0.2, 0.3, 100);
    assert!((under + 20.0).abs() < 0.1, "{under} dB");
    o.running()
        .apply(&Command::Duck(0, DuckChange::Key(0)))
        .expect("off");
    let off = run(&mut o, 0.2, 0.3, 200);
    assert!(off.abs() < 0.05, "without a key: {off} dB");
    let running = o.running();
    let mut refused = |change| running.apply(&Command::Duck(0, change)).unwrap_err();
    assert!(refused(DuckChange::Key(1)).contains("itself"));
    assert!(refused(DuckChange::Key(3)).contains("no input 3"));
    assert!(refused(DuckChange::Depth(90.0)).contains("0 to 60"));
    assert!(
        o.running()
            .apply(&Command::Duck(5, DuckChange::Depth(3.0)))
            .is_err()
    );
    let show = o.running().apply(&Command::Show).expect("show");
    assert!(
        !show[0].contains("duck"),
        "off, and not in show: {}",
        show[0]
    );
    o.running()
        .apply(&Command::Duck(0, DuckChange::Key(2)))
        .expect("on again");
    let show = o.running().apply(&Command::Show).expect("show");
    assert!(show[0].contains("duck by [2] 20.0 dB"), "{}", show[0]);
}

#[test]
fn the_view_shows_the_ducker_and_takes_its_actions() {
    let spec = profile::parse(PROFILE).expect("valid");
    let mut o = Offline::new(&spec, RATE, CHANNELS).expect("the route builds");
    run(&mut o, 0.2, 0.3, 100);
    let mut desk = EngineDesk::new(o.into_running(), None);
    let view = desk.view();
    assert_eq!(view.inputs[0].duck.key, 2);
    assert_eq!(view.inputs[0].duck.depth_db, 12.0);
    assert!(view.inputs[0].duck_reduction_db > 11.0);
    assert!(!view.inputs[1].duck.is_on());
    let set = |address: &str, number: f32| pult_core::protocol::Op::Set {
        address: address.to_owned(),
        value: pult_core::protocol::OpValue::Number(number),
        ramp_ms: None,
    };
    let action = set("input/1/duck", 18.0).action(&view).expect("an action");
    desk.act(action).expect("done");
    assert_eq!(desk.view().inputs[0].duck.depth_db, 18.0);
    let key = set("input/1/duck/key", 1.0);
    let refused = desk.act(key.action(&view).expect("an action")).unwrap_err();
    assert!(refused.contains("itself"), "{refused}");
}

#[test]
fn a_profile_reads_checks_and_renders_the_ducker() {
    let text = PROFILE.replace(
        "duck_by = 2",
        "duck_by = 2\nduck_db = 18\nduck_threshold_db = -30\nduck_attack_ms = 20\n\
         duck_hold_ms = 0\nduck_release_ms = 1500",
    );
    let spec = profile::parse(&text).expect("valid profile");
    assert_eq!(
        spec.inputs[0].duck,
        DuckSettings {
            key: 2,
            depth_db: 18.0,
            threshold_db: -30.0,
            attack_ms: 20.0,
            hold_ms: 0.0,
            release_ms: 1500.0,
        }
    );
    assert_eq!(profile::parse(&profile::render(&spec)).as_ref(), Ok(&spec));
    // A key alone brings 12 dB, and saving into the file that says so changes nothing in it.
    let alone = profile::parse(PROFILE).expect("valid profile");
    assert_eq!(alone.inputs[0].duck.depth_db, 12.0);
    assert_eq!(
        profile::parse(&profile::render(&alone)).as_ref(),
        Ok(&alone)
    );
    assert_eq!(profile::merge(PROFILE, &alone), PROFILE);
    // Switched off by its depth, the key stays, and the file says both.
    let mut off = alone.clone();
    off.inputs[0].duck.apply(DuckChange::Depth(0.0));
    let merged = profile::merge(PROFILE, &off);
    assert!(merged.contains("duck_by = 2") && merged.contains("duck_db = 0"));
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&off));
    assert_eq!(profile::parse(&profile::render(&off)).as_ref(), Ok(&off));
    // Without its key the line goes.
    let mut none = alone.clone();
    none.inputs[0].duck.apply(DuckChange::Key(0));
    let merged = profile::merge(PROFILE, &none);
    assert!(!merged.contains("duck_by"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&none));
    for (bad, why) in [
        ("duck_by = 1", "itself"),
        ("duck_by = 5", "no input 5"),
        ("duck_by = 2\nduck_db = 90", "0 to 60"),
        ("duck_by = 2\nduck_release_ms = 1", "10 to 5000"),
    ] {
        let error = profile::parse(&PROFILE.replace("duck_by = 2", bad)).unwrap_err();
        assert!(error.0.contains(why), "{bad}: {}", error.0);
    }
}
