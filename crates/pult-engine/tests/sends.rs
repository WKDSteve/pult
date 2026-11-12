//! Sends in the whole route, rendered without devices: one input on two buses, with a mix of
//! its own on each.

use pult_core::control::{Command, Target};
use pult_engine::{Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "A"
device = "offline stream"

[[bus]]
name = "B"
device = "offline headphones"

[[input]]
device = "mic"
buses = ["A", "B"]
sends = [{ bus = "B", gain_db = -6.0206, pre = true }]
"#;

fn run(offline: &mut Offline, callbacks: usize) -> (f32, f32) {
    let feed = vec![0.5_f32; PERIOD];
    let mut last = (0.0, 0.0);
    for _ in 0..callbacks {
        let outs = offline.process(&[&feed], PERIOD);
        last = (outs[0][PERIOD - 1], outs[1][PERIOD - 1]);
    }
    last
}

#[test]
fn a_pre_send_keeps_the_headphones_when_the_stream_fader_goes_down() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    let mut o = Offline::new(&spec, RATE, 1).expect("the route builds");
    let (a, b) = run(&mut o, 100);
    assert!((a - 0.5).abs() < 2e-3 && (b - 0.25).abs() < 2e-3, "{a} {b}");

    // The fader of the input down by 20 dB: the stream follows, the pre send does not.
    o.running()
        .apply(&Command::Gain(Target::Input(0), -20.0, 0))
        .expect("a gain in range");
    let (a, b) = run(&mut o, 50);
    assert!((a - 0.05).abs() < 2e-3, "stream {a}");
    assert!((b - 0.25).abs() < 2e-3, "headphones {b}");

    // A send of its own on the stream bus, and the headphones after the fader again.
    o.running()
        .apply(&Command::Send(0, 0, -6.0206, 0))
        .expect("a send");
    o.running()
        .apply(&Command::SendPre(0, 1, false))
        .expect("post");
    let (a, b) = run(&mut o, 50);
    assert!((a - 0.025).abs() < 2e-3, "stream {a}");
    assert!((b - 0.025).abs() < 2e-3, "headphones after the fader {b}");

    // What the route keeps, as `show` lists it: the send to A, and B back at its default.
    let shown = o.running().apply(&Command::Show).expect("show").join(
        "
",
    );
    assert!(shown.contains("send A -6.0 dB"), "{shown}");
    assert!(!shown.contains(" pre"), "{shown}");
}

#[test]
fn a_send_out_of_range_is_refused() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    let mut o = Offline::new(&spec, RATE, 1).expect("the route builds");
    let refused = o
        .running()
        .apply(&Command::Send(0, 1, 40.0, 0))
        .unwrap_err();
    assert!(refused.contains("a send must be"), "{refused}");
    assert!(o.running().apply(&Command::Send(0, 9, -6.0, 0)).is_err());
}
