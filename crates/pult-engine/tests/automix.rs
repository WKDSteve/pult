//! The automixer in the whole route, rendered without devices: two microphones in the automix
//! group and one input outside it, on one bus.

use pult_engine::{Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;
const CHANNELS: u16 = 1;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "A1"
device = "offline A1"

[[input]]
device = "mic 1"
buses = ["A1"]
automix = true

[[input]]
device = "mic 2"
buses = ["A1"]
automix = true

[[input]]
device = "music"
buses = ["A1"]
"#;

fn offline() -> Offline {
    let spec = profile::parse(PROFILE).expect("the profile is valid");
    assert!(spec.inputs[0].automix && spec.inputs[1].automix && !spec.inputs[2].automix);
    Offline::new(&spec, RATE, CHANNELS).expect("the route builds")
}

/// Runs `callbacks` with the constant levels `levels` (one per input); returns what each input
/// put into the mix in the last callback, in dB.
fn run(offline: &mut Offline, levels: [f32; 3], callbacks: usize) -> [f32; 3] {
    let feeds: Vec<Vec<f32>> = levels.iter().map(|&l| vec![l; PERIOD]).collect();
    let refs: Vec<&[f32]> = feeds.iter().map(Vec::as_slice).collect();
    for _ in 0..callbacks {
        for input in &offline.running().inputs {
            input.routes[0].stats.peak.take();
        }
        offline.process(&refs, PERIOD);
    }
    let mut out = [0.0; 3];
    for (i, input) in offline.running().inputs.iter().enumerate() {
        out[i] = 20.0 * (input.routes[0].stats.peak.take() / levels[i]).log10();
    }
    out
}

#[test]
fn the_microphone_spoken_into_is_open_and_the_other_is_down() {
    let mut o = offline();
    let [talker, room, music] = run(&mut o, [0.3, 0.001, 0.2], 100);
    assert!(talker > -0.1, "the talker at {talker} dB");
    assert!(room < -40.0, "the room microphone at {room} dB");
    assert!(
        music.abs() < 0.01,
        "an input outside the group keeps its gain: {music} dB"
    );
}

#[test]
fn two_talkers_share_the_gain_and_silence_shares_alike() {
    let mut o = offline();
    let [a, b, _] = run(&mut o, [0.3, 0.3, 0.2], 100);
    assert!((a + 3.0).abs() < 0.1 && (b + 3.0).abs() < 0.1, "{a} {b}");
    // Both quiet alike: one open microphone, -3 dB each for two.
    let [a, b, _] = run(&mut o, [0.0005, 0.0005, 0.2], 200);
    assert!((a + 3.0).abs() < 0.2 && (b + 3.0).abs() < 0.2, "{a} {b}");
}

#[test]
fn a_member_taken_out_keeps_its_gain() {
    let mut o = offline();
    let controls = std::sync::Arc::clone(&o.running().inputs[1].controls);
    let (group, index) = controls.automix.get().expect("in the group");
    group.member(*index).set_on(false);
    let [talker, out, _] = run(&mut o, [0.3, 0.001, 0.2], 100);
    assert!(talker > -0.1, "{talker}");
    assert!(out.abs() < 0.01, "out of the sharing, unchanged: {out} dB");
}
