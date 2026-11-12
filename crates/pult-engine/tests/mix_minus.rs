//! A mix-minus (N-1) bus, rendered without devices: the caller hears everything but their own
//! voice, and cannot be routed to it.

use pult_core::control::Command;
use pult_engine::{Offline, profile};

const PERIOD: usize = 480;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Stream"
device = "offline stream"

[[bus]]
name = "Call"
device = "offline call"
minus = 2

[[input]]
device = "mic"
buses = ["Stream", "Call"]

[[input]]
process = "Discord"
buses = ["Stream"]
"#;

#[test]
fn the_caller_hears_everything_but_their_own_voice() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert_eq!(spec.buses[1].minus, Some(1), "input 2, from 0");
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let (mic, caller) = (vec![0.25_f32; PERIOD], vec![0.5_f32; PERIOD]);
    let mut last = (0.0, 0.0);
    for _ in 0..60 {
        let outs = o.process(&[&mic, &caller], PERIOD);
        last = (outs[0][PERIOD - 1], outs[1][PERIOD - 1]);
    }
    assert!(
        (last.0 - 0.75).abs() < 2e-3,
        "the stream has both: {}",
        last.0
    );
    assert!(
        (last.1 - 0.25).abs() < 2e-3,
        "the call has only the mic: {}",
        last.1
    );

    let refused = o.running().apply(&Command::Route(1, 1, true)).unwrap_err();
    assert!(refused.contains("mix-minus"), "{refused}");

    let text = profile::render(&spec);
    assert!(text.contains("minus = 2"), "{text}");
    assert_eq!(profile::parse(&text).as_ref(), Ok(&spec));
}

#[test]
fn a_profile_that_feeds_the_caller_their_own_voice_is_refused() {
    let routed = PROFILE.replace(
        "process = \"Discord\"\nbuses = [\"Stream\"]",
        "process = \"Discord\"\nbuses = [\"Stream\", \"Call\"]",
    );
    assert!(
        profile::parse(&routed)
            .unwrap_err()
            .0
            .contains("must not feed it")
    );
    let nobody = PROFILE.replace("minus = 2", "minus = 7");
    assert!(
        profile::parse(&nobody)
            .unwrap_err()
            .0
            .contains("names no input")
    );
    let scene = format!(
        "{PROFILE}\n[[scene]]\nname = \"x\"\n[[scene.input]]\ninput = 2\nbuses = [\"Call\"]\n"
    );
    assert!(
        profile::parse(&scene)
            .unwrap_err()
            .0
            .contains("its mix-minus bus")
    );
}
