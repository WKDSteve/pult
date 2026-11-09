//! The sequence of the click test: a profile with two buses, two inputs and two scenes, rendered
//! without devices while ramps, scene changes, the limiter's lookahead and switch and the
//! equalizer's switch change what it plays. Shared by `clicks.rs` and `clicks_sabotage.rs`, which
//! run in processes of their own (the sabotage switches the smoothing off for its whole process).

use std::f32::consts::TAU;

use pult_core::control::{Command, Target};
use pult_core::limiter::LimitChange;
use pult_engine::{Offline, profile};

/// The rate everything runs at.
pub const RATE: u32 = 48_000;

/// Frames per callback, as WASAPI asks at 48 kHz.
pub const PERIOD: usize = 480;

/// Channels of every input and bus.
pub const CHANNELS: u16 = 2;

/// The test tone of both inputs: low, so that a jump stands far above its steepest step (see
/// `pult_core::clicks`). The steps of the sequence come between callbacks, on a grid of 10 ms; a
/// tone with a whole number of periods in that grid would meet every change at the same phase,
/// maybe a zero crossing, where no jump shows. At 103 Hz the phase moves on by 0.03 of a turn per
/// callback and the changes meet the tone at ever other phases.
pub const TONE_HZ: f32 = 103.0;

/// The route: A1 limits at -6 dB with a bell of +6 dB at the tone, A2 has a cut at the tone that
/// starts switched off. Input 1 plays on A1, input 2 on A2; the scene "laut" turns input 1 up,
/// routes input 2 to A1 as well and switches A2's equalizer on, "leise" turns input 1 far down
/// at once, mutes input 2 and takes it back off A1.
pub const PROFILE: &str = r#"
latency_ms = 40

[[bus]]
name = "A1"
device = "offline A1"
limit_on = true
limit_db = -6
limit_lookahead_ms = 1

[[bus.eq]]
band = 1
kind = "bell"
freq_hz = 100
gain_db = 6
q = 1

[[bus]]
name = "A2"
device = "offline A2"
eq_on = false

[[bus.eq]]
band = 2
kind = "bell"
freq_hz = 100
gain_db = -9
q = 2

[[input]]
device = "tone 1"
buses = ["A1"]

[[input]]
device = "tone 2"
buses = ["A2"]
gain_db = -3
pan = 0.3

[[scene]]
name = "laut"
blend_ms = 300

[[scene.input]]
input = 1
gain_db = 4

[[scene.input]]
input = 2
buses = ["A1", "A2"]

[[scene.bus]]
bus = "A2"
eq_on = true

[[scene]]
name = "leise"

[[scene.input]]
input = 1
gain_db = -20
pan = -0.5

[[scene.input]]
input = 2
mute = true
buses = ["A2"]

[[scene.bus]]
bus = "A1"
gain_db = -2
"#;

/// The whole render.
pub const SECONDS: f64 = 14.0;

/// One change of the sequence: when, and which command.
pub struct Step {
    /// Seconds from the start.
    pub at: f64,
    /// What changes.
    pub command: Command,
    /// Whether it is a change that only a ramp keeps free of jumps: gains, pans, mutes, scenes,
    /// a matrix button switched off. One switched on fades in where the ring starts to play
    /// again, before the resampler, and the limiter and equalizer slide along their own ways.
    #[allow(dead_code)] // Read by the sabotage probe only.
    pub ramped: bool,
}

/// The changes, one at a time, each with room to settle before the next.
#[must_use]
pub fn steps() -> Vec<Step> {
    let step = |at, command, ramped| Step {
        at,
        command,
        ramped,
    };
    let (a1, a2, one, two) = (0, 1, 0, 1);
    vec![
        // A long ramp, a short one on the bus, a pan at once.
        step(3.0, Command::Gain(Target::Input(one), -6.0, 500), true),
        step(3.8, Command::Gain(Target::Bus(a1), -3.0, 0), true),
        step(4.2, Command::Pan(one, -0.7, 0), true),
        // Louder over 300 ms, input 2 joins A1, A2's equalizer comes on; A1 now limits.
        step(4.6, Command::Scene("laut".to_owned()), true),
        // The limiter's lookahead off and to its longest, the limiter off and on.
        step(5.5, Command::Limit(a1, LimitChange::Lookahead(0.0)), false),
        step(6.0, Command::Limit(a1, LimitChange::Lookahead(5.0)), false),
        step(6.5, Command::Limit(a1, LimitChange::Switch(false)), false),
        step(7.0, Command::Limit(a1, LimitChange::Switch(true)), false),
        // The equalizers off and on.
        step(7.5, Command::Eq(a1, false), false),
        step(8.0, Command::Eq(a1, true), false),
        step(8.5, Command::Eq(a2, false), false),
        // Far down at once, input 2 muted and off A1.
        step(9.0, Command::Scene("leise".to_owned()), true),
        // Mutes of an input and a bus.
        step(10.0, Command::Mute(Target::Input(one), true), true),
        step(10.5, Command::Mute(Target::Input(one), false), true),
        step(11.0, Command::Mute(Target::Bus(a1), true), true),
        step(11.5, Command::Mute(Target::Bus(a1), false), true),
        // Input 2 back, onto A1 and off it again: the matrix button.
        step(11.8, Command::Mute(Target::Input(two), false), true),
        step(12.2, Command::Route(two, a1, true), false),
        step(13.0, Command::Route(two, a1, false), true),
    ]
}

/// What the buses played, interleaved, one buffer each.
pub struct Render {
    /// One per bus.
    pub buses: Vec<Vec<f32>>,
}

/// The frame a time falls on, as the callbacks count.
#[must_use]
pub fn frame_at(seconds: f64) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frame = (seconds * f64::from(RATE)) as usize;
    frame
}

/// The two starting phases of the tone each check renders with, in turns: a change that meets a
/// zero crossing of the tone in one (where even a jump is no step) meets a crest in the other.
pub const SHIFTS: [f32; 2] = [0.0, 0.25];

/// Renders the profile through the sequence, one callback of [`PERIOD`] frames after another; a
/// change is made between the two callbacks around its time, as the control thread would. The
/// tone starts `shift` turns into its period.
///
/// # Panics
///
/// When the profile or a command is refused.
#[must_use]
pub fn render(shift: f32) -> Render {
    let spec = profile::parse(PROFILE).expect("the test profile is valid");
    let mut offline = Offline::new(&spec, RATE, CHANNELS).expect("the route builds");
    let channels = usize::from(CHANNELS);
    let steps = steps();
    let mut next = 0;
    // Input 2 is out of phase with input 1 by a third of a turn, so they never cross zero
    // together.
    let mut phases = [TAU * shift, TAU * (shift + 1.0 / 3.0)];
    let advance = TAU * TONE_HZ / RATE as f32;
    let levels = [0.25_f32, 0.25];
    let mut feeds = vec![vec![0.0_f32; PERIOD * channels]; 2];
    let mut buses = vec![Vec::new(); spec.buses.len()];
    let callbacks = frame_at(SECONDS) / PERIOD;
    for callback in 0..callbacks {
        let frame = callback * PERIOD;
        while next < steps.len() && frame_at(steps[next].at) <= frame {
            offline
                .running()
                .apply(&steps[next].command)
                .expect("the command fits the route");
            next += 1;
        }
        for ((feed, phase), level) in feeds.iter_mut().zip(&mut phases).zip(levels) {
            for out in feed.chunks_exact_mut(channels) {
                out.fill(level * phase.sin());
                *phase = (*phase + advance) % TAU;
            }
        }
        let inputs: Vec<&[f32]> = feeds.iter().map(Vec::as_slice).collect();
        let outputs = offline.process(&inputs, PERIOD);
        for (all, out) in buses.iter_mut().zip(outputs) {
            all.extend_from_slice(out);
        }
    }
    assert_eq!(next, steps.len(), "every step ran");
    Render { buses }
}
