//! The delay of a bus in the whole route, rendered without devices: behind the limiter, set by
//! commands, kept by the profile, and gliding from one delay to another without a click.

use pult_core::Clicks;
use pult_core::control::Command;
use pult_engine::{Offline, profile};

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Boxen"
device = "offline speakers"
limit_on = false
delay_ms = 10

[[bus]]
name = "Phones"
device = "offline phones"
limit_on = false

[[input]]
device = "music"
buses = ["Boxen", "Phones"]
"#;

/// Renders `callbacks` callbacks of 480 frames of `feed(n)` for frame `n`; per bus what it
/// played.
fn render(o: &mut Offline, callbacks: usize, feed: impl Fn(usize) -> f32) -> Vec<Vec<f32>> {
    let mut buses = vec![Vec::new(); 2];
    for callback in 0..callbacks {
        let data: Vec<f32> = (0..480).map(|i| feed(callback * 480 + i)).collect();
        for (all, out) in buses.iter_mut().zip(o.process(&[&data], 480)) {
            all.extend_from_slice(out);
        }
    }
    buses
}

/// The first frame at which `out` is louder than half of `level`.
fn onset(out: &[f32], level: f32) -> usize {
    out.iter()
        .position(|s| s.abs() > level / 2.0)
        .expect("the sound arrives")
}

#[test]
fn a_bus_with_a_delay_plays_later_than_one_without() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert_eq!(
        (spec.buses[0].delay_ms, spec.buses[1].delay_ms),
        (10.0, 0.0)
    );
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    // Silence, then a step that the rings carry to both buses at the same time.
    let buses = render(&mut o, 40, |n| if n >= 9600 { 0.5 } else { 0.0 });
    let (boxen, phones) = (onset(&buses[0], 0.5), onset(&buses[1], 0.5));
    assert_eq!(
        boxen - phones,
        480,
        "10 ms at 48 kHz: {boxen} against {phones}"
    );
}

#[test]
fn a_command_sets_the_delay_and_a_change_does_not_click() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let shown = o
        .running()
        .apply(&Command::BusDelay(1, 7.5))
        .expect("a delay");
    assert!(shown[0].contains("7.5 ms"), "{shown:?}");
    assert!(o.running().apply(&Command::BusDelay(1, 300.0)).is_err());
    assert!(o.running().apply(&Command::BusDelay(1, -1.0)).is_err());

    // A tone that runs on while the delay of the speakers changes twice.
    let tone = 103.0_f32;
    let step = std::f32::consts::TAU * tone / 48_000.0;
    #[allow(clippy::cast_precision_loss)]
    let wave = move |n: usize| 0.25 * (step * n as f32).sin();
    let mut out = render(&mut o, 100, wave).swap_remove(0);
    o.running()
        .apply(&Command::BusDelay(0, 23.0))
        .expect("a delay");
    out.extend(render(&mut o, 50, wave).swap_remove(0));
    o.running()
        .apply(&Command::BusDelay(0, 0.0))
        .expect("no delay");
    out.extend(render(&mut o, 50, wave).swap_remove(0));
    // From one second on, when the rings have long settled.
    let clicks = Clicks::for_tone(tone, 48_000).find(&out[48_000..], 1);
    assert!(
        clicks.is_empty(),
        "{} clicks, first at {:?}",
        clicks.len(),
        clicks.first()
    );
}

#[test]
fn a_profile_keeps_the_delay() {
    let mut spec = profile::parse(PROFILE).expect("valid profile");
    spec.buses[1].delay_ms = 2.5;
    assert_eq!(profile::parse(&profile::render(&spec)).as_ref(), Ok(&spec));
    let merged = profile::merge(PROFILE, &spec);
    assert!(merged.contains("delay_ms = 2.5"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
    let bad = PROFILE.replace("delay_ms = 10", "delay_ms = 900");
    assert!(profile::parse(&bad).unwrap_err().0.contains("delay"));
}
