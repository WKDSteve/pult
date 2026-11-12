//! The bus compressor in the whole route, rendered without devices: on the sum, before the
//! fader, with its parallel mix, set by commands and kept by the profile.

use pult_core::control::Command;
use pult_engine::{Offline, profile};

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Stream"
device = "offline stream"
limit_on = false
comp = 6
comp_mix = 100
comp_hpf_hz = 20

[[input]]
device = "music"
buses = ["Stream"]
"#;

/// The peak of the last of `callbacks` callbacks of a 1 kHz sine at `level` (a sidechain
/// high-pass takes a constant level away, so a test signal must move).
fn last(o: &mut Offline, level: f32, callbacks: usize) -> f32 {
    let step = std::f32::consts::TAU * 1000.0 / 48_000.0;
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
fn the_sum_is_compressed_and_the_mix_brings_back_the_dry_sum() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    assert_eq!(
        (
            spec.buses[0].comp,
            spec.buses[0].comp_mix,
            spec.buses[0].comp_hpf_hz
        ),
        (6.0, 100.0, 20.0)
    );
    let mut o = Offline::new(&spec, 48_000, 1).expect("the route builds");
    let wet = last(&mut o, 0.9, 200);
    assert!(wet < 0.9 * 0.9, "compressed: {wet}");

    o.running()
        .apply(&Command::BusCompMix(0, 0.0))
        .expect("a mix");
    let dry = last(&mut o, 0.9, 100);
    assert!((dry - 0.9).abs() < 2e-3, "all dry: {dry}");

    o.running().apply(&Command::BusComp(0, 0.0)).expect("off");
    o.running()
        .apply(&Command::BusCompMix(0, 100.0))
        .expect("a mix");
    let off = last(&mut o, 0.9, 100);
    assert!((off - 0.9).abs() < 2e-3, "off: {off}");

    assert!(o.running().apply(&Command::BusCompMix(0, 120.0)).is_err());
    assert!(o.running().apply(&Command::BusCompHpf(0, 5.0)).is_err());
    let shown = o
        .running()
        .apply(&Command::BusCompHpf(0, 150.0))
        .expect("a filter");
    assert!(shown[0].contains("150 Hz"), "{shown:?}");
}

#[test]
fn a_profile_keeps_the_bus_compressor() {
    let mut spec = profile::parse(PROFILE).expect("valid profile");
    spec.buses[0].comp_mix = 40.0;
    spec.buses[0].comp_hpf_hz = 120.0;
    assert_eq!(profile::parse(&profile::render(&spec)).as_ref(), Ok(&spec));
    let merged = profile::merge(PROFILE, &spec);
    assert!(merged.contains("comp_mix = 40"), "{merged}");
    assert_eq!(profile::parse(&merged).as_ref(), Ok(&spec));
    let bad = PROFILE.replace("comp_hpf_hz = 20", "comp_hpf_hz = 900");
    assert!(profile::parse(&bad).unwrap_err().0.contains("comp_hpf_hz"));
}
