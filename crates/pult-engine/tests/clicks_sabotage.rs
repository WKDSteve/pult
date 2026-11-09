//! The sabotage probe of `clicks.rs`: the same sequence with the smoothing of all ramps switched
//! off must click at every change that only a ramp keeps smooth. Otherwise the clean result of
//! `clicks.rs` would prove nothing. A process of its own, since the switch holds for all of it.

mod sequence;

use pult_core::Clicks;
use sequence::{CHANNELS, RATE, SHIFTS, TONE_HZ, frame_at, render, steps};

/// How long after its step a click still belongs to it: a change shows within the callback
/// after it, behind the limiter's lookahead.
const WITHIN_SECONDS: f64 = 0.05;

#[test]
fn without_the_ramps_every_ramped_change_clicks() {
    pult_core::ramp::sabotage_smoothing(true);
    let detector = Clicks::for_tone(TONE_HZ, RATE);
    let channels = usize::from(CHANNELS);
    // Per shift of the tone, the frames of all clicks on any bus.
    let clicks: Vec<Vec<usize>> = SHIFTS
        .iter()
        .map(|&shift| {
            render(shift)
                .buses
                .iter()
                .flat_map(|out| detector.find(out, channels))
                .map(|click| click.frame)
                .collect()
        })
        .collect();
    let mut missed = Vec::new();
    for step in steps() {
        let window = frame_at(step.at)..frame_at(step.at + WITHIN_SECONDS);
        // A change may meet a zero crossing of the tone in one render, not in both.
        let heard: Vec<bool> = clicks
            .iter()
            .map(|frames| frames.iter().any(|frame| window.contains(frame)))
            .collect();
        println!("{:5.1} s {:?}: clicks {heard:?}", step.at, step.command);
        if step.ramped && !heard.contains(&true) {
            missed.push(format!("{} s {:?}", step.at, step.command));
        }
    }
    assert!(missed.is_empty(), "no click without the ramp: {missed:#?}");
}
