//! Drift control between two ideal clocks: an input and a bus on exactly the same 48 kHz, one
//! callback of each after the other, rendered without devices. There is no drift to learn, so the
//! correction may only work off where priming left the level, and has to go towards zero.

use pult_engine::{Offline, profile};

const RATE: u32 = 48_000;
const PERIOD: usize = 480;
const CHANNELS: u16 = 2;

const PROFILE: &str = r#"
latency_ms = 40

[[bus]]
name = "A1"
device = "offline A1"

[[input]]
device = "tone"
buses = ["A1"]
"#;

/// The correction in ppm after each whole second of `seconds`.
fn corrections(seconds: usize) -> Vec<f32> {
    let spec = profile::parse(PROFILE).expect("the profile is valid");
    let mut offline = Offline::new(&spec, RATE, CHANNELS).expect("the route builds");
    let feed = vec![0.1_f32; PERIOD * usize::from(CHANNELS)];
    let per_second = RATE as usize / PERIOD;
    let mut out = Vec::with_capacity(seconds);
    for callback in 0..seconds * per_second {
        offline.process(&[&feed], PERIOD);
        if (callback + 1) % per_second == 0 {
            let route = &offline.running().inputs[0].routes[0];
            out.push(route.stats.correction_ppm.load());
        }
    }
    let stats = &offline.running().inputs[0].routes[0].stats;
    assert_eq!(stats.underruns.get(), 0, "no underrun between ideal clocks");
    assert_eq!(stats.resyncs.get(), 0, "no resync between ideal clocks");
    out
}

#[test]
fn ideal_clocks_steer_the_correction_to_zero() {
    let ppm = corrections(240);
    let at = |s: usize| ppm[s - 1].abs();
    // Priming leaves the level about one callback above the target, inside the band, so the
    // settling phase does not align it; the PI loop (15 s proportional, 60 s integral) works it
    // off instead: about 660 ppm at first, 370 after 14 s (what the click test showed), through
    // zero within the first minute, a dip of a few percent below, and nothing left after four.
    assert!(at(3) > at(14) && at(14) > at(60), "{ppm:?}");
    let dip = ppm.iter().copied().fold(0.0_f32, f32::min);
    assert!(dip > -0.05 * at(3), "dips to {dip} ppm: {ppm:?}");
    assert!(at(240) < 2.0, "{} ppm after 4 min: {ppm:?}", at(240));
}
