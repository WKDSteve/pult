//! The sequence of the click test: a profile with two buses, two inputs and two scenes, rendered
//! without devices while ramps, scene changes, the limiter's lookahead and switch and the
//! equalizer's switch change what it plays. Shared by `clicks.rs` and `clicks_sabotage.rs`, which
//! run in processes of their own (the sabotage switches the smoothing off for its whole process).

use std::f32::consts::TAU;
use std::ops::Range;

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
    /// Per bus, the frames whose clicks are no change's fault: from the moment a device went
    /// away (a bus that falls silent, an input that runs dry) until it is back. What follows its
    /// return is checked like the rest.
    #[allow(dead_code)] // Read by the click check, not by the sabotage probe.
    pub excused: Vec<Option<Range<usize>>>,
    /// Per callback, the correction of input 1's ring on A1 in ppm (the only bus it feeds).
    #[allow(dead_code)] // Read by the drift check only.
    pub corrections: Vec<f32>,
}

impl Render {
    /// Whether a click at `frame` on `bus` is excused.
    #[allow(dead_code)] // Used by the click check, not by the sabotage probe.
    #[must_use]
    pub fn excused(&self, bus: usize, frame: usize) -> bool {
        self.excused[bus]
            .as_ref()
            .is_some_and(|gap| gap.contains(&frame))
    }
}

/// A device that is away for a while: from `from` to `to` seconds.
#[derive(Debug, Clone, Copy)]
pub enum Outage {
    /// The device of a bus (index) is gone; the route suspends the bus and resumes it.
    Bus(usize, f64, f64),
    /// An input (index) delivers nothing; its rings run dry, then prime and fade in again.
    Input(usize, f64, f64),
}

impl Outage {
    /// When it starts and ends, in seconds.
    #[allow(dead_code)] // Used by the sabotage probe, not by the click check.
    #[must_use]
    pub fn span(self) -> (f64, f64) {
        match self {
            Self::Bus(_, from, to) | Self::Input(_, from, to) => (from, to),
        }
    }
}

/// How the devices behave around the sequence.
#[derive(Debug, Clone, Copy)]
pub struct Variant {
    /// What the output of a test calls it.
    pub name: &'static str,
    /// The rate of both inputs; the buses run at [`RATE`].
    pub input_rate: u32,
    /// How much faster the clock of both inputs runs than the buses', in ppm.
    pub drift_ppm: f64,
    /// A device that goes and comes back.
    pub outage: Option<Outage>,
}

/// Every device at [`RATE`] on one ideal clock, none goes away.
pub const PLAIN: Variant = Variant {
    name: "plain",
    input_rate: RATE,
    drift_ppm: 0.0,
    outage: None,
};

/// The other ways the sequence is rendered: an input rate other than the buses' (44.1 kHz into
/// 48 kHz, the resampler with its real ratio), a clock that drifts (the rate control steers),
/// a bus whose device goes and comes back in the middle of the sequence (between the scene
/// "leise" and the next mute, no step falls into it), and an input that runs dry while input 2
/// is routed to A1 as well (between 12.2 and 13.0 s, no step falls into it either).
pub const VARIANTS: [Variant; 4] = [
    Variant {
        name: "44.1 kHz into 48 kHz",
        input_rate: 44_100,
        ..PLAIN
    },
    Variant {
        name: "inputs 300 ppm fast",
        drift_ppm: 300.0,
        ..PLAIN
    },
    Variant {
        name: "A1 gone 9.3 to 9.8 s",
        outage: Some(Outage::Bus(0, 9.3, 9.8)),
        ..PLAIN
    },
    Variant {
        name: "input 1 dry 12.4 to 12.8 s",
        outage: Some(Outage::Input(0, 12.4, 12.8)),
        ..PLAIN
    },
];

/// The frame a time falls on, as the callbacks count.
#[must_use]
pub fn frame_at(seconds: f64) -> usize {
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let frame = (seconds * f64::from(RATE)) as usize;
    frame
}

/// The first frame of the callback a time falls into.
#[must_use]
pub fn callback_at(seconds: f64) -> usize {
    frame_at(seconds) / PERIOD * PERIOD
}

/// The two starting phases of the tone each check renders with, in turns: a change that meets a
/// zero crossing of the tone in one (where even a jump is no step) meets a crest in the other.
pub const SHIFTS: [f32; 2] = [0.0, 0.25];

/// Renders the profile through the sequence, one callback of [`PERIOD`] frames after another; a
/// change is made between the two callbacks around its time, as the control thread would. The
/// tone starts `shift` turns into its period. The inputs deliver what their rate and drift owe
/// per callback (441 frames at 44.1 kHz), and `variant`'s outage takes a device away.
///
/// # Panics
///
/// When the profile or a command is refused.
#[must_use]
pub fn render_with(shift: f32, variant: Variant) -> Render {
    let spec = profile::parse(PROFILE).expect("the test profile is valid");
    let rates = vec![variant.input_rate; spec.inputs.len()];
    let mut offline = Offline::with_rates(&spec, &rates, RATE, CHANNELS).expect("the route builds");
    let channels = usize::from(CHANNELS);
    let steps = steps();
    let mut next = 0;
    // Input 2 is out of phase with input 1 by a third of a turn, so they never cross zero
    // together.
    let mut phases = [TAU * shift, TAU * (shift + 1.0 / 3.0)];
    let advance = TAU * TONE_HZ / variant.input_rate as f32;
    let levels = [0.25_f32, 0.25];
    let per_callback = PERIOD as f64 * f64::from(variant.input_rate) / f64::from(RATE)
        * (1.0 + variant.drift_ppm * 1e-6);
    let mut owed = 0.0_f64;
    let mut feeds = vec![Vec::new(); 2];
    let mut buses = vec![Vec::new(); spec.buses.len()];
    // A bus that is gone, or one an input that runs dry feeds at the time (input 2 is on A1
    // during the dry spell of input 1, so the gap counts on every bus).
    let excused = match variant.outage {
        Some(Outage::Bus(bus, from, to)) => (0..spec.buses.len())
            .map(|b| (b == bus).then(|| frame_at(from)..frame_at(to)))
            .collect(),
        Some(Outage::Input(_, from, to)) => {
            vec![Some(frame_at(from)..frame_at(to)); spec.buses.len()]
        }
        None => vec![None; spec.buses.len()],
    };
    let mut corrections = Vec::new();
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
        let mut dry = None;
        match variant.outage {
            Some(Outage::Bus(bus, from, _)) if frame == callback_at(from) => offline.suspend(bus),
            Some(Outage::Bus(bus, _, to)) if frame == callback_at(to) => offline.resume(bus),
            Some(Outage::Input(input, from, to))
                if (callback_at(from)..callback_at(to)).contains(&frame) =>
            {
                dry = Some(input);
            }
            _ => {}
        }
        owed += per_callback;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = owed.floor() as usize;
        owed -= frames as f64;
        for (i, ((feed, phase), level)) in feeds.iter_mut().zip(&mut phases).zip(levels).enumerate()
        {
            feed.resize(frames * channels, 0.0);
            for out in feed.chunks_exact_mut(channels) {
                out.fill(level * phase.sin());
                *phase = (*phase + advance) % TAU;
            }
            if dry == Some(i) {
                // The tone runs on meanwhile: a device that comes back does not resume where it
                // left off.
                feed.clear();
            }
        }
        let inputs: Vec<&[f32]> = feeds.iter().map(Vec::as_slice).collect();
        let outputs = offline.process(&inputs, PERIOD);
        for (all, out) in buses.iter_mut().zip(outputs) {
            all.extend_from_slice(out);
        }
        corrections.push(
            offline.running().inputs[0].routes[0]
                .stats
                .correction_ppm
                .load(),
        );
    }
    assert_eq!(next, steps.len(), "every step ran");
    Render {
        buses,
        excused,
        corrections,
    }
}
