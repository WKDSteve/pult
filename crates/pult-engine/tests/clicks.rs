//! No change pult makes clicks: a profile and a sequence of changes (ramps, scene changes, the
//! limiter's lookahead and switch, the equalizer's switch) rendered without devices, from the
//! start of the route on, every step from one sample to the next checked against what the test
//! tone can do at its level (`pult_core::clicks`). `clicks_sabotage.rs` shows the check fails
//! without the ramps.
//!
//! The same sequence also runs as the devices of a real route behave ([`sequence::VARIANTS`]):
//! inputs at 44.1 kHz into buses at 48 kHz, input clocks that drift, a bus whose device goes and
//! comes back, an input that runs dry and returns. The resampler starts like the plain render,
//! after the ring has primed and with the fade-in in front of it, so nothing at the start is
//! excused; only the time a device is away is (its loss is no change of pult's), and what
//! follows its return is checked like the rest.

mod sequence;

use pult_core::Clicks;
use pult_core::clicks::MARGIN;
use sequence::{CHANNELS, PLAIN, RATE, SHIFTS, TONE_HZ, VARIANTS, Variant, render_with};

/// Renders `variant` with both phases of the tone and asserts that no bus clicks outside its
/// excused gaps.
fn plays_without_a_click(variant: Variant) {
    let detector = Clicks::for_tone(TONE_HZ, RATE);
    let channels = usize::from(CHANNELS);
    for shift in SHIFTS {
        let render = render_with(shift, variant);
        let name = variant.name;
        for (bus, out) in render.buses.iter().enumerate() {
            // Something played: the check is not passed by silence.
            let peak = out.iter().fold(0.0_f32, |peak, s| peak.max(s.abs()));
            assert!(
                peak > 0.1,
                "{name}, bus {}, shift {shift}: peak {peak}",
                bus + 1
            );
            let clicks: Vec<_> = detector
                .find(out, channels)
                .into_iter()
                .filter(|click| !render.excused(bus, click.frame))
                .collect();
            let steepest = detector.steepest(out, channels);
            println!(
                "{name}, bus {}, shift {shift}: steepest step {steepest:.2} times the tone's own, \
                 a click from {MARGIN}",
                bus + 1
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
                "{name}, bus {}, shift {shift}: {} clicks, first {shown:#?}",
                bus + 1,
                clicks.len()
            );
        }
    }
}

#[test]
fn the_sequence_plays_without_a_click() {
    plays_without_a_click(PLAIN);
}

#[test]
fn the_sequence_plays_without_a_click_at_another_rate() {
    plays_without_a_click(VARIANTS[0]);
}

#[test]
fn the_sequence_plays_without_a_click_on_a_drifting_clock() {
    plays_without_a_click(VARIANTS[1]);
}

#[test]
fn a_bus_whose_device_comes_back_starts_without_a_click() {
    plays_without_a_click(VARIANTS[2]);
}

#[test]
fn an_input_that_ran_dry_comes_back_without_a_click() {
    plays_without_a_click(VARIANTS[3]);
}

/// The rate control follows a drifting clock without a jump: from one callback to the next its
/// correction moves by little, and it heads for the drift it has to make up.
#[test]
fn the_correction_of_a_drifting_clock_does_not_jump() {
    let variant = VARIANTS[1];
    let render = render_with(0.0, variant);
    let largest = render
        .corrections
        .windows(2)
        .map(|pair| (pair[1] - pair[0]).abs())
        .fold(0.0_f32, f32::max);
    let last = *render.corrections.last().expect("callbacks ran");
    println!("largest step of the correction {largest:.2} ppm, at the end {last:.1} ppm");
    // A step of 20 ppm per 10 ms callback is far below anything audible as a change of pitch
    // (0.03 cent) and far above what the control does; a jump of the plan would move it by
    // hundreds.
    assert!(largest < 20.0, "the correction jumped by {largest} ppm");
    // The control steers slowly on purpose (seconds); by the end it has come a good part of
    // the way and not overshot.
    #[allow(clippy::cast_possible_truncation)]
    let drift = variant.drift_ppm as f32;
    assert!(
        (drift / 3.0..drift * 1.5).contains(&last),
        "the correction ends at {last} ppm for a drift of {drift} ppm"
    );
}
