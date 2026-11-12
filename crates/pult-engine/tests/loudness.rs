//! The loudness of a bus in the whole route, rendered without devices: the tap of the mixer,
//! the meter (drained after every callback here) and the numbers of the bus.

use pult_engine::{Offline, profile};

const PROFILE: &str = r#"
latency_ms = 20

[[bus]]
name = "Stream"
device = "offline stream"

[[input]]
device = "tone"
buses = ["Stream"]
"#;

#[test]
fn a_bus_with_a_sine_at_minus_23_dbfs_reads_minus_23_lufs() {
    let spec = profile::parse(PROFILE).expect("valid profile");
    let mut o = Offline::new(&spec, 48_000, 2).expect("the route builds");
    let amplitude = 10_f32.powf(-23.0 / 20.0);
    let step = std::f32::consts::TAU * 1000.0 / 48_000.0;
    let mut feed = vec![0.0_f32; 480 * 2];
    let mut n = 0_u32;
    for _ in 0..1500 {
        for frame in feed.as_chunks_mut::<2>().0 {
            #[allow(clippy::cast_precision_loss)]
            frame.fill(amplitude * (step * n as f32).sin());
            n = (n + 1) % 48_000;
        }
        o.process(&[&feed], 480);
    }
    let stats = o.running().buses[0].loudness.clone().expect("a meter");
    let integrated = stats.integrated().expect("measured");
    assert!((integrated + 23.0).abs() < 0.2, "{integrated} LUFS");
    let short = stats.short_term().expect("measured");
    assert!((short + 23.0).abs() < 0.2, "{short} LUFS");
    assert!(stats.true_peak().expect("measured") < -22.0);
}
