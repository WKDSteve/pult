//! No change pult makes clicks: a profile and a sequence of changes (ramps, scene changes, the
//! limiter's lookahead and switch, the equalizer's switch) rendered without devices, from the
//! start of the route on, every step from one sample to the next checked against what the test
//! tone can do at its level (`pult_core::clicks`). `clicks_sabotage.rs` shows the check fails
//! without the ramps.

mod sequence;

use pult_core::Clicks;
use pult_core::clicks::MARGIN;
use sequence::{CHANNELS, RATE, SHIFTS, TONE_HZ, render};

#[test]
fn the_sequence_plays_without_a_click() {
    let detector = Clicks::for_tone(TONE_HZ, RATE);
    let channels = usize::from(CHANNELS);
    for shift in SHIFTS {
        let render = render(shift);
        for (bus, out) in render.buses.iter().enumerate() {
            let bus = bus + 1;
            // Something played: the check is not passed by silence.
            let peak = out.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
            assert!(peak > 0.1, "bus {bus}, shift {shift}: peak {peak}");
            let clicks = detector.find(out, channels);
            let steepest = detector.steepest(out, channels);
            println!(
                "bus {bus}, shift {shift}: steepest step {steepest:.2} times the tone's own, \
                 a click from {MARGIN}"
            );
            let shown: Vec<String> = clicks
                .iter()
                .take(10)
                .map(|click| {
                    format!(
                        "{:.4} s ch {}: {:.4} > {:.4}",
                        click.frame as f64 / f64::from(RATE),
                        click.channel,
                        click.step,
                        click.allowed
                    )
                })
                .collect();
            assert!(
                clicks.is_empty(),
                "bus {bus}, shift {shift}: {} clicks, first {shown:#?}",
                clicks.len()
            );
        }
    }
}
