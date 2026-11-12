//! The input stage in the whole route, rendered without devices: an input on two buses passes
//! its gate and compressor once, before its rings, and its PRE meter reads what arrives.

use std::sync::atomic::Ordering;

use pult_core::{CompressorParams, GateParams};
use pult_engine::{Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;
const CHANNELS: u16 = 2;

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "A1"
device = "offline A1"

[[bus]]
name = "A2"
device = "offline A2"

[[input]]
device = "mic"
buses = ["A1", "A2"]
"#;

fn offline() -> Offline {
    let spec = profile::parse(PROFILE).expect("the profile is valid");
    Offline::new(&spec, RATE, CHANNELS).expect("the route builds")
}

fn run(offline: &mut Offline, value: f32, callbacks: usize) -> Vec<Vec<f32>> {
    let feed = vec![value; PERIOD * usize::from(CHANNELS)];
    let mut last = Vec::new();
    for _ in 0..callbacks {
        last = offline.process(&[&feed], PERIOD).to_vec();
    }
    last
}

#[test]
fn both_buses_get_the_input_compressed_once() {
    let mut o = offline();
    let controls = std::sync::Arc::clone(&o.running().inputs[0].controls);
    controls.compressor.set(CompressorParams {
        threshold_db: -26.0,
        ratio: 4.0,
        ..CompressorParams::default()
    });
    controls.compressor.switch(true);
    let outs = run(&mut o, 0.5, 300);
    let (a1, a2) = (outs[0][outs[0].len() - 1], outs[1][outs[1].len() - 1]);
    // -6 dB is 20 dB above the threshold; three quarters of that go.
    let want = 0.5 * 10_f32.powf(-0.75 * (20.0 * 0.5_f32.log10() + 26.0) / 20.0);
    assert!((a1 - want).abs() < 2e-3, "A1 {a1}, want {want}");
    assert!(
        (a1 - a2).abs() < 1e-6,
        "one compressor for both buses: {a1} {a2}"
    );
}

#[test]
fn the_pre_meter_reads_what_arrives_while_the_gate_is_closed() {
    let mut o = offline();
    let controls = std::sync::Arc::clone(&o.running().inputs[0].controls);
    controls.gate.set(GateParams {
        threshold_db: -20.0,
        hold_ms: 0.0,
        release_ms: 5.0,
        ..GateParams::default()
    });
    controls.gate.switch(true);
    controls.meter_pre_fader.store(true, Ordering::Relaxed);
    let outs = run(&mut o, 0.01, 50);
    assert!(
        outs.iter().all(|out| out[out.len() - 1] == 0.0),
        "the gate is closed"
    );
    controls.pre_peak.take();
    o.running().inputs[0].routes[0].stats.peak.take();
    run(&mut o, 0.01, 2);
    assert!((controls.pre_peak.take() - 0.01).abs() < 1e-4);
    let ring = o.running().inputs[0].routes[0].stats.peak.take();
    assert_eq!(ring, 0.0, "nothing goes into the mix");
}
