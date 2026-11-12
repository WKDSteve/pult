//! The sabotage probe of `clicks.rs`: the same sequence with the smoothing of all ramps switched
//! off must click at every change that only a ramp keeps smooth. Otherwise the clean result of
//! `clicks.rs` would prove nothing. A process of its own, since the switch holds for all of it.
//!
//! The switch also takes away the fade-in of a ring that primed again and the crossfade of a
//! skip, so the variants of `clicks.rs` must click where a device comes back and where the
//! level is aligned after settling; and their ramped steps must click as in the plain render.

mod sequence;

use pult_core::Clicks;
use sequence::{
    CHANNELS, Outage, RATE, SHIFTS, TONE_HZ, VARIANTS, Variant, frame_at, render_with, steps,
};

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
            render_with(shift, sequence::PLAIN)
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

/// Per shift of the tone, the frames of the clicks of `variant` on any bus.
fn clicks_of(variant: Variant) -> Vec<Vec<usize>> {
    let detector = Clicks::for_tone(TONE_HZ, RATE);
    let channels = usize::from(CHANNELS);
    SHIFTS
        .iter()
        .map(|&shift| {
            render_with(shift, variant)
                .buses
                .iter()
                .flat_map(|out| detector.find(out, channels))
                .map(|click| click.frame)
                .collect()
        })
        .collect()
}

/// Whether some render clicks between `from` and `to` seconds.
fn heard(clicks: &[Vec<usize>], from: f64, to: f64) -> bool {
    let window = frame_at(from)..frame_at(to);
    clicks
        .iter()
        .any(|frames| frames.iter().any(|frame| window.contains(frame)))
}

#[test]
fn without_the_smoothing_the_variants_click_where_they_must() {
    pult_core::ramp::sabotage_smoothing(true);
    let mut missed = Vec::new();
    let mut drift_clicks = Vec::new();
    for (i, variant) in VARIANTS.into_iter().enumerate() {
        let clicks = clicks_of(variant);
        for step in steps().iter().filter(|step| step.ramped) {
            let excused = variant.outage.is_some_and(|outage| {
                let (from, to) = outage.span();
                (from..to).contains(&step.at)
            });
            if !excused && !heard(&clicks, step.at, step.at + WITHIN_SECONDS) {
                missed.push(format!(
                    "{}: {} s {:?}",
                    variant.name, step.at, step.command
                ));
            }
        }
        // A device that comes back: the ring primes again and starts without its fade-in.
        if let Some(outage) = variant.outage {
            let (_, to) = outage.span();
            let back = heard(&clicks, to, to + 0.2);
            let what = match outage {
                Outage::Bus(..) => "the bus coming back",
                Outage::Input(..) => "the input coming back",
            };
            println!("{}: {what} clicks {back}", variant.name);
            if !back {
                missed.push(format!("{}: {what} at {to} s", variant.name));
            }
        }
        if i == 1 {
            drift_clicks = clicks;
        }
    }
    // The drifting clock leaves the ring above its band when settling ends (2 s after it
    // primed): the level is aligned with a skip, which without its crossfade jumps.
    let drift = VARIANTS[1];
    let aligned = heard(&drift_clicks, 2.0, 2.3);
    println!("{}: the skip after settling clicks {aligned}", drift.name);
    if !aligned {
        missed.push(format!("{}: the skip after settling", drift.name));
    }
    assert!(
        missed.is_empty(),
        "no click without the smoothing: {missed:#?}"
    );
}
