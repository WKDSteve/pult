//! Gate and compressor of an input: what keeps a microphone quiet between words and even
//! while it speaks.
//!
//! The control thread writes the settings into [`GateControls`] and [`CompressorControls`]
//! (atomics); a [`Gate`] and a [`Compressor`] in each ring of the input follow them, before
//! the fader. Both measure the loudest channel of a frame and turn all channels by the same
//! amount, so a stereo image stays where it is. Switched off they return to unity along their
//! own times and then cost nothing.
//!
//! Each has its single settings and a one-knob form ([`GateParams::simple`],
//! [`CompressorParams::simple`]) that sets them all from one amount.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::level::{AtomicF32, db_to_linear, within};

/// Lowest and highest threshold of a gate in dB.
pub const GATE_THRESHOLD_DB: (f32, f32) = (-80.0, 0.0);

/// How far a closed gate turns down, in dB; the lowest value closes it completely.
pub const GATE_RANGE_DB: (f32, f32) = (-80.0, 0.0);

/// Shortest and longest time a gate takes to open, in milliseconds.
pub const GATE_ATTACK_MS: (f32, f32) = (0.1, 100.0);

/// Shortest and longest time a gate stays open below its threshold, in milliseconds.
pub const GATE_HOLD_MS: (f32, f32) = (0.0, 2000.0);

/// Shortest and longest time a gate takes to close, in milliseconds.
pub const GATE_RELEASE_MS: (f32, f32) = (5.0, 4000.0);

/// Lowest and highest threshold of a compressor in dB.
pub const COMPRESSOR_THRESHOLD_DB: (f32, f32) = (-60.0, 0.0);

/// Gentlest and hardest ratio of a compressor; 1 does nothing.
pub const COMPRESSOR_RATIO: (f32, f32) = (1.0, 20.0);

/// Shortest and longest time a compressor takes to turn down, in milliseconds.
pub const COMPRESSOR_ATTACK_MS: (f32, f32) = (0.1, 200.0);

/// Shortest and longest time a compressor takes to let go, in milliseconds.
pub const COMPRESSOR_RELEASE_MS: (f32, f32) = (5.0, 4000.0);

/// Least and most gain a compressor adds after turning down, in dB.
pub const COMPRESSOR_MAKEUP_DB: (f32, f32) = (0.0, 24.0);

/// Highest amount of the one-knob forms; 0 does nothing.
pub const SIMPLE_MAX: f32 = 10.0;

/// A gate closes this far below the level that opens it, so a level at the threshold does not
/// flutter.
const GATE_HYSTERESIS_DB: f32 = 3.0;

/// How fast the measured level of a gate falls after a peak, in milliseconds. A hold only starts
/// once it is below the threshold, so this adds to the hold: it stays short.
const GATE_LEVEL_FALL_MS: f32 = 10.0;

/// The shortest time a gate stays open below its threshold, in milliseconds, whatever its hold
/// says: half a wave of 20 Hz, so that it sees a tone and not single waves.
const GATE_LEAST_HOLD_MS: f32 = 25.0;

/// A gate's times are the times after which it has all but arrived: 99 % of the way, 40 dB.
/// That is this many time constants (the natural logarithm of 100).
const GATE_ARRIVED: f32 = 4.605;

/// Width of the bend around a compressor's threshold in dB.
const KNEE_DB: f32 = 6.0;

/// Time a compressor's make-up gain takes to follow a new setting, in milliseconds.
const MAKEUP_MS: f32 = 10.0;

/// The level a compressor takes for silence, in dB: the logarithm of nothing has no value.
const SILENCE_DB: f32 = -120.0;

/// The part of the distance to a goal a value covers per frame, for a time constant of `ms`
/// milliseconds at `rate` frames per second.
///
/// In `f64`, like what it moves: in `f32` a slow value stops short of its goal, where a step
/// is smaller than the numbers around it can tell apart.
fn pace(ms: f32, rate: f32) -> f64 {
    1.0 - (-1000.0 / (f64::from(ms.max(0.01)) * f64::from(rate))).exp()
}

/// The loudest sample of a frame; a NaN or an infinity counts as silence, so that it cannot
/// stay in what is measured.
#[inline]
fn loudest(frame: &[f32]) -> f32 {
    frame.iter().fold(0.0, |max: f32, sample| {
        if sample.is_finite() {
            max.max(sample.abs())
        } else {
            max
        }
    })
}

/// The settings of a gate.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GateParams {
    /// The level in dB above which it opens.
    pub threshold_db: f32,
    /// How far it turns down when closed, in dB; at the lowest value it closes completely.
    pub range_db: f32,
    /// Milliseconds it takes to open (99 % of the way).
    pub attack_ms: f32,
    /// Milliseconds it stays open after the level fell below the threshold; at least 25.
    pub hold_ms: f32,
    /// Milliseconds it takes to close (99 % of the way).
    pub release_ms: f32,
}

impl Default for GateParams {
    fn default() -> Self {
        Self {
            threshold_db: -50.0,
            range_db: GATE_RANGE_DB.0,
            attack_ms: 1.0,
            hold_ms: 150.0,
            release_ms: 200.0,
        }
    }
}

impl GateParams {
    /// The one-knob form: `amount` from 0 to [`SIMPLE_MAX`] raises the threshold from -60 to
    /// -20 dB, with the other settings fit for speech.
    #[must_use]
    pub fn simple(amount: f32) -> Self {
        let amount = within(amount, (0.0, SIMPLE_MAX), 0.0);
        Self {
            threshold_db: -60.0 + 4.0 * amount,
            ..Self::default()
        }
    }

    /// The same settings with every value inside its range; a NaN becomes the default.
    #[must_use]
    pub fn clamped(self) -> Self {
        let default = Self::default();
        Self {
            threshold_db: within(self.threshold_db, GATE_THRESHOLD_DB, default.threshold_db),
            range_db: within(self.range_db, GATE_RANGE_DB, default.range_db),
            attack_ms: within(self.attack_ms, GATE_ATTACK_MS, default.attack_ms),
            hold_ms: within(self.hold_ms, GATE_HOLD_MS, default.hold_ms),
            release_ms: within(self.release_ms, GATE_RELEASE_MS, default.release_ms),
        }
    }
}

/// The gate of one input as the control thread sets it; off at first.
#[derive(Debug)]
pub struct GateControls {
    on: AtomicBool,
    threshold_db: AtomicF32,
    range_db: AtomicF32,
    attack_ms: AtomicF32,
    hold_ms: AtomicF32,
    release_ms: AtomicF32,
}

impl Default for GateControls {
    fn default() -> Self {
        let params = GateParams::default();
        Self {
            on: AtomicBool::new(false),
            threshold_db: AtomicF32::new(params.threshold_db),
            range_db: AtomicF32::new(params.range_db),
            attack_ms: AtomicF32::new(params.attack_ms),
            hold_ms: AtomicF32::new(params.hold_ms),
            release_ms: AtomicF32::new(params.release_ms),
        }
    }
}

impl GateControls {
    /// Sets the gate, clamped to its ranges.
    pub fn set(&self, params: GateParams) {
        let params = params.clamped();
        self.threshold_db.store(params.threshold_db);
        self.range_db.store(params.range_db);
        self.attack_ms.store(params.attack_ms);
        self.hold_ms.store(params.hold_ms);
        self.release_ms.store(params.release_ms);
    }

    /// The gate as it is set.
    #[must_use]
    pub fn params(&self) -> GateParams {
        GateParams {
            threshold_db: self.threshold_db.load(),
            range_db: self.range_db.load(),
            attack_ms: self.attack_ms.load(),
            hold_ms: self.hold_ms.load(),
            release_ms: self.release_ms.load(),
        }
    }

    /// Sets the gate by its one knob ([`GateParams::simple`]); at 0 it is off.
    pub fn set_simple(&self, amount: f32) {
        self.set(GateParams::simple(amount));
        self.switch(amount > 0.0);
    }

    /// Switches the gate on or off; off, it opens and keeps its settings.
    pub fn switch(&self, on: bool) {
        self.on.store(on, Ordering::Release);
    }

    /// Whether the gate is switched on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }
}

/// The gate inside one ring's callback.
#[derive(Debug)]
pub struct Gate {
    rate: f32,
    on: bool,
    params: GateParams,
    /// Linear level that opens it, and the lower one that keeps it open.
    opens_at: f32,
    stays_above: f32,
    /// Linear gain when closed.
    floor: f64,
    attack: f64,
    release: f64,
    hold_frames: u32,
    level_fall: f32,
    /// The measured level: every peak at once, falling slowly after it.
    level: f32,
    open: bool,
    /// Frames it still stays open below the threshold.
    hold: u32,
    /// Where its gain stands.
    gain: f64,
    /// Started over, and no callback yet: the next one stands closed if the gate is on.
    fresh: bool,
}

impl Gate {
    /// A gate at `rate` frames per second, switched off.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        let mut gate = Self {
            rate,
            on: false,
            params: GateParams::default(),
            opens_at: 0.0,
            stays_above: 0.0,
            floor: 0.0,
            attack: 0.0,
            release: 0.0,
            hold_frames: 0,
            level_fall: (-1000.0 / (GATE_LEVEL_FALL_MS * rate)).exp(),
            level: 0.0,
            open: false,
            hold: 0,
            gain: 1.0,
            fresh: false,
        };
        gate.configure(false, GateParams::default());
        gate
    }

    fn configure(&mut self, on: bool, params: GateParams) {
        self.on = on;
        self.params = params;
        self.opens_at = db_to_linear(params.threshold_db);
        self.stays_above = db_to_linear(params.threshold_db - GATE_HYSTERESIS_DB);
        self.floor = if params.range_db <= GATE_RANGE_DB.0 {
            0.0
        } else {
            f64::from(db_to_linear(params.range_db))
        };
        self.attack = pace(params.attack_ms / GATE_ARRIVED, self.rate);
        self.release = pace(params.release_ms / GATE_ARRIVED, self.rate);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let hold_frames = (params.hold_ms.max(GATE_LEAST_HOLD_MS) * self.rate / 1000.0) as u32;
        self.hold_frames = hold_frames;
    }

    /// Forgets what it measured, closed if it is on: a ring that starts to play opens it again
    /// with the first sound above the threshold.
    pub fn start_over(&mut self) {
        self.level = 0.0;
        self.open = false;
        self.hold = 0;
        // Where it stands then depends on the settings, which the next callback reads.
        self.fresh = true;
    }

    /// Gates `data` (interleaved, `channels` per frame) in place as `controls` say.
    pub fn process(&mut self, controls: &GateControls, data: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let (on, params) = (controls.is_on(), controls.params());
        if on != self.on || params != self.params {
            self.configure(on, params);
        }
        if std::mem::take(&mut self.fresh) {
            self.gain = if self.on { self.floor } else { 1.0 };
        }
        #[allow(clippy::float_cmp)] // Exactly: it is set to unity when it arrives there.
        if !self.on && self.gain == 1.0 {
            return;
        }
        for frame in data.chunks_exact_mut(channels) {
            self.level = loudest(frame).max(self.level * self.level_fall);
            let threshold = if self.open {
                self.stays_above
            } else {
                self.opens_at
            };
            if self.level >= threshold {
                self.open = true;
                self.hold = self.hold_frames;
            } else if self.hold > 0 {
                self.hold -= 1;
            } else {
                self.open = false;
            }
            let goal = if self.open || !self.on {
                1.0
            } else {
                self.floor
            };
            let pace = if goal > self.gain {
                self.attack
            } else {
                self.release
            };
            self.gain += (goal - self.gain) * pace;
            // Arrived, as far as anyone hears (120 dB down): stand there. In every frame, since
            // a long callback would otherwise reach numbers too small to compute fast.
            if (goal - self.gain).abs() < 1e-6 {
                self.gain = goal;
            }
            #[allow(clippy::cast_possible_truncation)]
            let gain = self.gain as f32;
            for sample in frame {
                *sample *= gain;
            }
        }
        if self.level < 1e-9 {
            self.level = 0.0;
        }
    }
}

/// The settings of a compressor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CompressorParams {
    /// The level in dB above which it turns down.
    pub threshold_db: f32,
    /// How hard: above the threshold, this many dB in become one dB out.
    pub ratio: f32,
    /// Milliseconds it takes to turn down, as a time constant (63 % of the way).
    pub attack_ms: f32,
    /// Milliseconds it takes to let go, as a time constant (63 % of the way).
    pub release_ms: f32,
    /// Gain in dB added after it, for the loudness it took.
    pub makeup_db: f32,
}

impl Default for CompressorParams {
    fn default() -> Self {
        Self {
            threshold_db: -18.0,
            ratio: 3.0,
            attack_ms: 10.0,
            release_ms: 150.0,
            makeup_db: 0.0,
        }
    }
}

impl CompressorParams {
    /// The one-knob form: `amount` from 0 (nothing) to [`SIMPLE_MAX`] lowers the threshold to
    /// -30 dB, raises the ratio to 8 and adds up to 12 dB after it. A steady level at full
    /// scale never comes out above full scale; a peak can, while the compressor still turns
    /// down or between the peaks of a wave, and the bus limits it.
    #[must_use]
    pub fn simple(amount: f32) -> Self {
        let amount = within(amount, (0.0, SIMPLE_MAX), 0.0);
        let threshold_db = -3.0 * amount;
        let ratio = 1.0 + 0.7 * amount;
        // Never more than it takes from full scale; low settings take little, in the bend.
        let taken = (1.0 - 1.0 / ratio) * bent(-threshold_db);
        Self {
            threshold_db,
            ratio,
            makeup_db: (1.2 * amount).min(taken),
            ..Self::default()
        }
    }

    /// The same settings with every value inside its range; a NaN becomes the default.
    #[must_use]
    pub fn clamped(self) -> Self {
        let default = Self::default();
        Self {
            threshold_db: within(
                self.threshold_db,
                COMPRESSOR_THRESHOLD_DB,
                default.threshold_db,
            ),
            ratio: within(self.ratio, COMPRESSOR_RATIO, default.ratio),
            attack_ms: within(self.attack_ms, COMPRESSOR_ATTACK_MS, default.attack_ms),
            release_ms: within(self.release_ms, COMPRESSOR_RELEASE_MS, default.release_ms),
            makeup_db: within(self.makeup_db, COMPRESSOR_MAKEUP_DB, default.makeup_db),
        }
    }
}

/// The compressor of one input as the control thread sets it; off at first.
#[derive(Debug)]
pub struct CompressorControls {
    on: AtomicBool,
    threshold_db: AtomicF32,
    ratio: AtomicF32,
    attack_ms: AtomicF32,
    release_ms: AtomicF32,
    makeup_db: AtomicF32,
}

impl Default for CompressorControls {
    fn default() -> Self {
        let params = CompressorParams::default();
        Self {
            on: AtomicBool::new(false),
            threshold_db: AtomicF32::new(params.threshold_db),
            ratio: AtomicF32::new(params.ratio),
            attack_ms: AtomicF32::new(params.attack_ms),
            release_ms: AtomicF32::new(params.release_ms),
            makeup_db: AtomicF32::new(params.makeup_db),
        }
    }
}

impl CompressorControls {
    /// Sets the compressor, clamped to its ranges.
    pub fn set(&self, params: CompressorParams) {
        let params = params.clamped();
        self.threshold_db.store(params.threshold_db);
        self.ratio.store(params.ratio);
        self.attack_ms.store(params.attack_ms);
        self.release_ms.store(params.release_ms);
        self.makeup_db.store(params.makeup_db);
    }

    /// The compressor as it is set.
    #[must_use]
    pub fn params(&self) -> CompressorParams {
        CompressorParams {
            threshold_db: self.threshold_db.load(),
            ratio: self.ratio.load(),
            attack_ms: self.attack_ms.load(),
            release_ms: self.release_ms.load(),
            makeup_db: self.makeup_db.load(),
        }
    }

    /// Sets the compressor by its one knob ([`CompressorParams::simple`]); at 0 it is off.
    pub fn set_simple(&self, amount: f32) {
        self.set(CompressorParams::simple(amount));
        self.switch(amount > 0.0);
    }

    /// Switches the compressor on or off; off, it lets go and keeps its settings.
    pub fn switch(&self, on: bool) {
        self.on.store(on, Ordering::Release);
    }

    /// Whether the compressor is switched on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }
}

/// The compressor inside one ring's callback.
#[derive(Debug)]
pub struct Compressor {
    rate: f32,
    on: bool,
    params: CompressorParams,
    /// What it works with: the settings while on, nothing to do while off.
    threshold_db: f32,
    /// The part of the level above the threshold it takes away: `1 - 1/ratio`.
    taken: f32,
    makeup_goal_db: f64,
    attack: f64,
    release: f64,
    makeup_pace: f64,
    /// How far it has turned down, in dB.
    reduction_db: f64,
    /// Where its make-up gain stands, in dB.
    makeup_db: f64,
}

impl Compressor {
    /// A compressor at `rate` frames per second, switched off.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let rate = rate.max(1) as f32;
        let mut compressor = Self {
            rate,
            on: false,
            params: CompressorParams::default(),
            threshold_db: 0.0,
            taken: 0.0,
            makeup_goal_db: 0.0,
            attack: 0.0,
            release: 0.0,
            makeup_pace: pace(MAKEUP_MS, rate),
            reduction_db: 0.0,
            makeup_db: 0.0,
        };
        compressor.configure(false, CompressorParams::default());
        compressor
    }

    fn configure(&mut self, on: bool, params: CompressorParams) {
        self.on = on;
        self.params = params;
        self.threshold_db = params.threshold_db;
        self.taken = if on { 1.0 - 1.0 / params.ratio } else { 0.0 };
        self.makeup_goal_db = if on { f64::from(params.makeup_db) } else { 0.0 };
        self.attack = pace(params.attack_ms, self.rate);
        self.release = pace(params.release_ms, self.rate);
    }

    /// Lets go at once: a ring that starts to play is not turned down for what played before.
    pub fn start_over(&mut self) {
        self.reduction_db = 0.0;
    }

    /// Compresses `data` (interleaved, `channels` per frame) in place as `controls` say.
    pub fn process(&mut self, controls: &CompressorControls, data: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        let (on, params) = (controls.is_on(), controls.params());
        if on != self.on || params != self.params {
            self.configure(on, params);
        }
        #[allow(clippy::float_cmp)] // Exactly: both are set to their goal when they arrive.
        let resting = self.reduction_db == 0.0 && self.makeup_db == self.makeup_goal_db;
        if resting {
            // Nothing above the bend in all of it: one gain for everything, without measuring
            // frame by frame.
            let quiet =
                self.taken == 0.0 || level_db(loudest(data)) <= self.threshold_db - KNEE_DB / 2.0;
            if quiet {
                if self.makeup_db != 0.0 {
                    let gain = gain_of(self.makeup_db);
                    for sample in data.iter_mut() {
                        *sample *= gain;
                    }
                }
                return;
            }
        }
        let mut wanted = 0.0;
        for frame in data.chunks_exact_mut(channels) {
            wanted = f64::from(self.taken * bent(level_db(loudest(frame)) - self.threshold_db));
            let pace = if wanted > self.reduction_db {
                self.attack
            } else {
                self.release
            };
            self.reduction_db += (wanted - self.reduction_db) * pace;
            self.makeup_db += (self.makeup_goal_db - self.makeup_db) * self.makeup_pace;
            let gain = gain_of(self.makeup_db - self.reduction_db);
            for sample in frame {
                *sample *= gain;
            }
        }
        // Arrived, as far as anyone hears: stand there.
        if wanted == 0.0 && self.reduction_db < 1e-3 {
            self.reduction_db = 0.0;
        }
        if (self.makeup_goal_db - self.makeup_db).abs() < 1e-3 {
            self.makeup_db = self.makeup_goal_db;
        }
    }
}

/// The linear factor of `db` decibels.
#[inline]
fn gain_of(db: f64) -> f32 {
    #[allow(clippy::cast_possible_truncation)]
    let db = db as f32;
    db_to_linear(db)
}

/// A linear level in dB, silence as [`SILENCE_DB`].
#[inline]
fn level_db(linear: f32) -> f32 {
    if linear > 1e-6 {
        20.0 * linear.log10()
    } else {
        SILENCE_DB
    }
}

/// How many dB above the threshold count, for a level `over` dB above it: none well below,
/// all of them well above, and a bend of [`KNEE_DB`] between instead of a corner.
#[inline]
fn bent(over: f32) -> f32 {
    let half = KNEE_DB / 2.0;
    if over <= -half {
        0.0
    } else if over >= half {
        over
    } else {
        (over + half) * (over + half) / (2.0 * KNEE_DB)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// `ms` milliseconds of the constant `value` on `channels` channels.
    fn steady(value: f32, ms: usize, channels: usize) -> Vec<f32> {
        vec![value; RATE as usize / 1000 * ms * channels]
    }

    /// The largest step between neighbouring samples.
    fn largest_step(data: &[f32]) -> f32 {
        data.windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    fn gate(params: GateParams) -> (GateControls, Gate) {
        let controls = GateControls::default();
        controls.set(params);
        controls.switch(true);
        (controls, Gate::new(RATE))
    }

    fn compressor(params: CompressorParams) -> (CompressorControls, Compressor) {
        let controls = CompressorControls::default();
        controls.set(params);
        controls.switch(true);
        (controls, Compressor::new(RATE))
    }

    #[test]
    fn switched_off_both_change_nothing() {
        let original: Vec<f32> = (0..960).map(|n| (n as f32 * 0.01).sin()).collect();
        let mut data = original.clone();
        Gate::new(RATE).process(&GateControls::default(), &mut data, 2);
        Compressor::new(RATE).process(&CompressorControls::default(), &mut data, 2);
        assert_eq!(data, original);
    }

    #[test]
    fn a_gate_lets_the_loud_through_and_closes_on_the_quiet() {
        let (controls, mut gate) = gate(GateParams {
            threshold_db: -40.0,
            ..GateParams::default()
        });
        // -6 dB is above the threshold: open after a few milliseconds, and then untouched.
        let mut loud = steady(0.5, 50, 1);
        gate.process(&controls, &mut loud, 1);
        assert_eq!(loud[loud.len() - 1], 0.5);
        // -60 dB is below: it holds for 150 ms, then closes within a second, completely.
        let mut quiet = steady(0.001, 100, 1);
        gate.process(&controls, &mut quiet, 1);
        assert_eq!(quiet[quiet.len() - 1], 0.001, "still held open");
        let mut quiet = steady(0.001, 3000, 1);
        gate.process(&controls, &mut quiet, 1);
        assert_eq!(quiet[quiet.len() - 1], 0.0);
        assert!(largest_step(&quiet) < 1e-5, "{}", largest_step(&quiet));
    }

    #[test]
    fn a_gate_keeps_a_floor_when_its_range_says_so() {
        let (controls, mut gate) = gate(GateParams {
            threshold_db: -40.0,
            range_db: -20.0,
            hold_ms: 0.0,
            release_ms: 20.0,
            ..GateParams::default()
        });
        let mut quiet = steady(0.001, 500, 1);
        gate.process(&controls, &mut quiet, 1);
        assert!((quiet[quiet.len() - 1] - 0.0001).abs() < 1e-6);
    }

    #[test]
    fn a_gate_does_not_flutter_around_its_threshold() {
        let (controls, mut gate) = gate(GateParams {
            threshold_db: -40.0,
            hold_ms: 0.0,
            release_ms: 5.0,
            ..GateParams::default()
        });
        // Opened at -38 dB, the level then sinks to -42 dB: inside the 3 dB it stays open.
        let mut data = steady(db_to_linear(-38.0), 50, 1);
        gate.process(&controls, &mut data, 1);
        let level = db_to_linear(-42.0);
        let mut data = steady(level, 500, 1);
        gate.process(&controls, &mut data, 1);
        assert_eq!(data[data.len() - 1], level);
        // From closed, the same level does not open it.
        gate.start_over();
        let mut data = steady(level, 500, 1);
        gate.process(&controls, &mut data, 1);
        assert_eq!(data[data.len() - 1], 0.0);
    }

    #[test]
    fn a_gate_does_not_flutter_on_a_low_tone() {
        for freq in [20.0, 50.0, 100.0] {
            let (controls, mut gate) = gate(GateParams {
                threshold_db: -40.0,
                hold_ms: 0.0,
                release_ms: 5.0,
                ..GateParams::default()
            });
            // Hardly above the threshold, without a hold of its own: open between the half waves.
            let peak = db_to_linear(-39.9);
            let step = std::f32::consts::TAU * freq / RATE as f32;
            let original: Vec<f32> = (0..RATE as usize)
                .map(|frame| peak * (step * frame as f32).sin())
                .collect();
            let mut data = original.clone();
            gate.process(&controls, &mut data, 1);
            let half = RATE as usize / 2;
            assert_eq!(data[half..], original[half..], "{freq} Hz");
        }
    }

    #[test]
    fn a_gate_holds_for_its_time_and_then_closes() {
        let (controls, mut gate) = gate(GateParams {
            threshold_db: -40.0,
            hold_ms: 150.0,
            release_ms: 20.0,
            ..GateParams::default()
        });
        // Just above the threshold, so the level is below it within milliseconds of the end.
        let mut near = steady(db_to_linear(-39.0), 50, 1);
        gate.process(&controls, &mut near, 1);
        let quiet = db_to_linear(-60.0);
        let mut held = steady(quiet, 120, 1);
        gate.process(&controls, &mut held, 1);
        assert_eq!(held[held.len() - 1], quiet, "held open");
        let mut closed = steady(quiet, 200, 1);
        gate.process(&controls, &mut closed, 1);
        assert_eq!(closed[closed.len() - 1], 0.0, "closed after the hold");
    }

    #[test]
    fn a_gate_measures_the_loudest_channel_and_turns_all_alike() {
        let (controls, mut gate) = gate(GateParams {
            threshold_db: -40.0,
            ..GateParams::default()
        });
        // Left loud, right quiet: open for both.
        let mut data: Vec<f32> = (0..4800).flat_map(|_| [0.5, 0.001]).collect();
        gate.process(&controls, &mut data, 2);
        assert_eq!(&data[data.len() - 2..], &[0.5, 0.001]);
    }

    #[test]
    fn a_gate_switched_off_opens_without_a_step() {
        let (controls, mut gate) = gate(GateParams {
            threshold_db: -20.0,
            hold_ms: 0.0,
            release_ms: 5.0,
            attack_ms: 10.0,
            ..GateParams::default()
        });
        let mut data = steady(0.01, 500, 1);
        gate.process(&controls, &mut data, 1);
        assert_eq!(data[data.len() - 1], 0.0);
        controls.switch(false);
        let mut data = steady(0.01, 500, 1);
        gate.process(&controls, &mut data, 1);
        assert_eq!(data[data.len() - 1], 0.01);
        assert!(largest_step(&data) < 0.01 / 50.0, "{}", largest_step(&data));
    }

    #[test]
    fn a_compressor_turns_down_what_is_above_its_threshold() {
        let (controls, mut compressor) = compressor(CompressorParams {
            threshold_db: -26.0,
            ratio: 4.0,
            ..CompressorParams::default()
        });
        // -6 dB is 20 dB above: three quarters of that go, 15 dB.
        let mut loud = steady(0.5, 2000, 1);
        compressor.process(&controls, &mut loud, 1);
        let expected = 0.5 * db_to_linear(-0.75 * (linear_db(0.5) + 26.0));
        let last = loud[loud.len() - 1];
        assert!((last - expected).abs() < 1e-4, "{last} against {expected}");
        // It took its attack time: the first sample is hardly touched.
        assert!(loud[0] > 0.49);
        assert!(largest_step(&loud) < 0.01);
    }

    fn linear_db(linear: f32) -> f32 {
        20.0 * linear.log10()
    }

    #[test]
    fn a_compressor_leaves_what_is_below_and_adds_its_makeup() {
        let (controls, mut compressor) = compressor(CompressorParams {
            threshold_db: -26.0,
            ratio: 4.0,
            ..CompressorParams::default()
        });
        let original = steady(0.01, 100, 2);
        let mut data = original.clone();
        compressor.process(&controls, &mut data, 2);
        assert_eq!(data, original);

        controls.set(CompressorParams {
            makeup_db: 6.0,
            ..controls.params()
        });
        let mut data = steady(0.01, 200, 2);
        compressor.process(&controls, &mut data, 2);
        let last = data[data.len() - 1];
        assert!((last - 0.01 * db_to_linear(6.0)).abs() < 1e-6, "{last}");
        assert!(largest_step(&data) < 1e-4, "{}", largest_step(&data));
    }

    #[test]
    fn a_compressor_bends_around_its_threshold() {
        assert_eq!(bent(-3.0), 0.0);
        assert_eq!(bent(3.0), 3.0);
        assert!((bent(0.0) - 0.75).abs() < 1e-6);
        assert_eq!(bent(-40.0), 0.0);
        assert_eq!(bent(20.0), 20.0);
    }

    #[test]
    fn a_compressor_switched_off_lets_go_without_a_step() {
        let (controls, mut compressor) = compressor(CompressorParams {
            threshold_db: -26.0,
            ratio: 4.0,
            makeup_db: 6.0,
            ..CompressorParams::default()
        });
        let mut data = steady(0.5, 1000, 1);
        compressor.process(&controls, &mut data, 1);
        controls.switch(false);
        let mut data = steady(0.5, 3000, 1);
        compressor.process(&controls, &mut data, 1);
        assert_eq!(data[data.len() - 1], 0.5);
        assert!(largest_step(&data) < 1e-3, "{}", largest_step(&data));
        // The settings are kept for when it comes back.
        assert_eq!(controls.params().makeup_db, 6.0);
    }

    #[test]
    fn a_compressor_lets_go_at_once_when_a_ring_starts_over() {
        let (controls, mut compressor) = compressor(CompressorParams {
            threshold_db: -26.0,
            ratio: 4.0,
            ..CompressorParams::default()
        });
        let mut data = steady(0.5, 1000, 1);
        compressor.process(&controls, &mut data, 1);
        compressor.start_over();
        let mut data = steady(0.01, 1, 1);
        compressor.process(&controls, &mut data, 1);
        assert_eq!(data[0], 0.01);
    }

    #[test]
    fn the_one_knob_forms_do_nothing_at_zero_and_never_clip() {
        let nothing = CompressorParams::simple(0.0);
        assert_eq!((nothing.ratio, nothing.makeup_db), (1.0, 0.0));
        assert_eq!(GateParams::simple(0.0).threshold_db, -60.0);
        assert_eq!(GateParams::simple(99.0).threshold_db, -20.0);
        assert_eq!(GateParams::simple(f32::NAN), GateParams::simple(0.0));
        // In steps of a tenth: the low settings work in the bend, where little is taken.
        for tenth in 0..=100 {
            let amount = tenth as f32 / 10.0;
            let params = CompressorParams::simple(amount);
            assert_eq!(params, params.clamped(), "inside the ranges");
            let (controls, mut compressor) = compressor(params);
            let mut data = steady(1.0, 2000, 1);
            compressor.process(&controls, &mut data, 1);
            let last = data[data.len() - 1];
            assert!(last <= 1.0 + 1e-4, "{amount}: {last}");
        }
        assert_eq!(CompressorParams::simple(10.0).makeup_db, 12.0);
    }

    #[test]
    fn values_out_of_range_are_clamped_and_nan_is_the_default() {
        let controls = GateControls::default();
        controls.set(GateParams {
            threshold_db: 20.0,
            range_db: -200.0,
            attack_ms: 0.0,
            hold_ms: -1.0,
            release_ms: f32::NAN,
        });
        assert_eq!(
            controls.params(),
            GateParams {
                threshold_db: 0.0,
                range_db: -80.0,
                attack_ms: 0.1,
                hold_ms: 0.0,
                release_ms: 200.0,
            }
        );
        let controls = CompressorControls::default();
        controls.set(CompressorParams {
            threshold_db: -99.0,
            ratio: 0.0,
            attack_ms: 1e9,
            release_ms: 0.0,
            makeup_db: f32::NAN,
        });
        assert_eq!(
            controls.params(),
            CompressorParams {
                threshold_db: -60.0,
                ratio: 1.0,
                attack_ms: 200.0,
                release_ms: 5.0,
                makeup_db: 0.0,
            }
        );
    }

    #[test]
    fn a_nan_or_an_infinity_does_not_stay_in_what_they_measure() {
        let (gate_controls, mut gate) = gate(GateParams::default());
        let (controls, mut compressor) = compressor(CompressorParams::default());
        let mut broken = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5];
        gate.process(&gate_controls, &mut broken, 1);
        compressor.process(&controls, &mut broken, 1);
        // Afterwards a quiet level is as if nothing had been: the gate closes, the compressor
        // leaves it alone.
        let mut quiet = steady(0.0005, 3000, 1);
        gate.process(&gate_controls, &mut quiet, 1);
        assert_eq!(quiet[quiet.len() - 1], 0.0);
        let mut quiet = steady(0.0005, 3000, 1);
        compressor.process(&controls, &mut quiet, 1);
        assert_eq!(quiet[quiet.len() - 1], 0.0005);
    }

    #[test]
    fn both_work_without_allocating() {
        let (gate_controls, mut gate) = gate(GateParams::default());
        let (controls, mut compressor) = compressor(CompressorParams::default());
        let mut loud = steady(0.5, 100, 2);
        let mut quiet = steady(0.0001, 100, 2);
        assert_no_alloc::assert_no_alloc(|| {
            for round in 0..20 {
                match round {
                    5 => controls.set(CompressorParams::simple(7.0)),
                    8 => gate_controls.set(GateParams::simple(7.0)),
                    12 => controls.switch(false),
                    15 => gate_controls.switch(false),
                    _ => {}
                }
                let data = if round % 2 == 0 {
                    &mut loud
                } else {
                    &mut quiet
                };
                gate.process(&gate_controls, data, 2);
                compressor.process(&controls, data, 2);
            }
        });
    }
}
