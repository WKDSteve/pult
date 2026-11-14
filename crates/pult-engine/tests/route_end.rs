//! The end of a route does not click: before its streams fall every bus fades to silence, at the
//! very end of its chain (behind the limiter's lookahead and the delay of the bus). Rendered
//! without devices; what a stopped stream plays is silence, so the render is followed by it.

use std::f32::consts::TAU;

use pult_core::Clicks;
use pult_engine::{Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;
const CHANNELS: usize = 2;
const TONE_HZ: f32 = 103.0;

/// One bus with everything that holds sound back behind its gain, one without.
const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Boxen"
device = "offline speakers"
limit_on = true
limit_db = -6
limit_lookahead_ms = 5
delay_ms = 30

[[bus]]
name = "Phones"
device = "offline phones"
limit_on = false

[[input]]
device = "tone"
buses = ["Boxen", "Phones"]
"#;

/// A route that has played the tone for half a second, what its buses played, and the tone's
/// phase.
fn playing() -> (Offline, Vec<Vec<f32>>, f32) {
    let spec = profile::parse(PROFILE).expect("the test profile is valid");
    let mut offline = Offline::new(&spec, RATE, 2).expect("the route builds");
    let mut buses = vec![Vec::new(); 2];
    let mut phase = 0.0;
    play(&mut offline, &mut buses, &mut phase, 50);
    (offline, buses, phase)
}

/// `callbacks` more callbacks of the tone.
fn play(offline: &mut Offline, buses: &mut [Vec<f32>], phase: &mut f32, callbacks: usize) {
    #[allow(clippy::cast_precision_loss)]
    let advance = TAU * TONE_HZ / RATE as f32;
    let mut feed = vec![0.0_f32; PERIOD * CHANNELS];
    for _ in 0..callbacks {
        for frame in feed.as_chunks_mut::<CHANNELS>().0 {
            frame.fill(0.5 * phase.sin());
            *phase = (*phase + advance) % TAU;
        }
        for (all, out) in buses.iter_mut().zip(offline.process(&[&feed], PERIOD)) {
            all.extend_from_slice(out);
        }
    }
}

/// The clicks of `out` when silence follows it, as when its stream stops.
fn clicks_with_silence_behind(out: &[f32]) -> usize {
    let mut all = out.to_vec();
    all.resize(out.len() + PERIOD * CHANNELS, 0.0);
    Clicks::for_tone(TONE_HZ, RATE).find(&all, CHANNELS).len()
}

#[test]
fn a_route_that_fades_out_ends_without_a_click() {
    let (mut offline, mut buses, mut phase) = playing();
    assert!(!offline.running().faded_out());
    offline.running().fade_out();
    // The fade takes 5 ms; the callback after it reports it.
    play(&mut offline, &mut buses, &mut phase, 2);
    assert!(offline.running().faded_out(), "two callbacks are enough");
    for (bus, out) in buses.iter().enumerate() {
        let peak = out.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
        assert!(peak > 0.1, "bus {bus} played: peak {peak}");
        assert_eq!(clicks_with_silence_behind(out), 0, "bus {bus}");
        let tail = &out[out.len() - PERIOD * CHANNELS..];
        assert!(tail.iter().all(|s| *s == 0.0), "bus {bus} ends in silence");
    }
}

#[test]
fn without_the_fade_the_end_clicks() {
    // What the fade is for, and that the check above can fail: at least one of 20 ends lands
    // inside the wave.
    let (mut offline, mut buses, mut phase) = playing();
    let mut clicked = 0;
    for _ in 0..20 {
        play(&mut offline, &mut buses, &mut phase, 1);
        clicked += clicks_with_silence_behind(&buses[1]);
    }
    assert!(clicked > 0, "an end without a fade is a step");
}

#[test]
fn the_fade_is_the_last_thing_in_the_chain() {
    // The delay of "Boxen" holds 30 ms of sound; the fade is over after 5 ms all the same.
    let (mut offline, mut buses, mut phase) = playing();
    offline.running().fade_out();
    play(&mut offline, &mut buses, &mut phase, 1);
    let out = &buses[0];
    let last = &out[out.len() - PERIOD * CHANNELS..];
    let fade = RATE as usize * 5 / 1000;
    assert!(
        last[..CHANNELS].iter().any(|s| s.abs() > 0.01),
        "it starts loud"
    );
    assert!(
        last[fade * CHANNELS..].iter().all(|s| *s == 0.0),
        "silent after 5 ms"
    );
}

#[test]
fn a_bus_whose_device_is_gone_does_not_hold_the_end_up() {
    let (mut offline, mut buses, mut phase) = playing();
    offline.suspend(0);
    offline.running().fade_out();
    play(&mut offline, &mut buses, &mut phase, 2);
    assert!(offline.running().faded_out());
}
