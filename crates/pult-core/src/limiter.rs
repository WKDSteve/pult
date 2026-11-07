//! The limiter of a bus: what leaves it stays below a ceiling, however loud the sum gets.
//!
//! All channels share one gain, so the stereo image holds. With `look` frames of lookahead the
//! gain of a frame comes from three steps:
//!
//! 1. the gain each frame would need on its own (`ceiling / peak`, at most 1), held as the
//!    smallest of the last `look + 1` frames;
//! 2. a follower that goes down at once and comes back over [`RELEASE_MS`];
//! 3. the average of the last `look + 1` values of that follower.
//!
//! The sound itself is delayed by `look` frames, so a frame leaves when all the values of the
//! average have seen it, and each of them is at most the gain it needs. The ceiling therefore
//! holds for every signal and every lookahead; lookahead only decides how gently the gain
//! moves. Without any, a peak is met by a step in the gain, which still beats clipping.
//!
//! Like every other change of the sound nothing jumps: the ceiling slides, a new lookahead
//! crossfades between the old and the new delay. While either is under way the ceiling may be
//! passed for a moment, and the bus's clamp to full scale remains the last line of defence.
//! Switched off, the limiter slides out of the way, drops its delay and only keeps the delay line
//! filled for its return.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::level::{AtomicF32, linear_to_db, within};

/// Lowest and highest ceiling in dB.
pub const LIMIT_CEILING_DB: (f32, f32) = (-20.0, 0.0);

/// Shortest and longest lookahead in milliseconds.
pub const LIMIT_LOOKAHEAD_MS: (f32, f32) = (0.0, 5.0);

/// The ceiling of a bus nobody set one for, in dB: just below full scale, where a peak between
/// two samples still has some room in the device's converter.
pub const DEFAULT_CEILING_DB: f32 = -0.3;

/// The lookahead of a bus nobody set one for, in milliseconds: enough to round off a peak, little
/// against the latency of the path.
pub const DEFAULT_LOOKAHEAD_MS: f32 = 1.0;

/// Milliseconds in which the gain comes back by 63 % of the way after a peak.
const RELEASE_MS: f64 = 100.0;

/// How fast the ceiling slides, in dB per second: the whole range in 20 ms.
const CEILING_PACE: f32 = 1000.0;

/// Where the ceiling goes while switched off, in dB: out of the way of any real signal.
const OUT_OF_THE_WAY_DB: f32 = 24.0;

/// Milliseconds of the crossfade between two delays.
const FADE_MS: f64 = 5.0;

/// The most a limiter reports having turned down, in dB. Without this an infinite sample would
/// report infinity, which JSON cannot say: a client would not read the state any more.
const MOST_REPORTED_DB: f32 = 120.0;

/// How close to 1 the follower counts as back: no one hears the rest, and from there on the gain
/// is exactly 1 again.
const BACK: f64 = 1.0 - 1e-6;

/// One setting of a limiter.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LimitChange {
    /// Its switch.
    Switch(bool),
    /// Its ceiling in dB.
    Ceiling(f32),
    /// Its lookahead in milliseconds.
    Lookahead(f32),
}

impl LimitChange {
    /// The change when its value is inside its range: nothing is clamped, as with a gain.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise.
    pub fn checked(self) -> Result<Self, String> {
        let (what, value, (low, high), unit) = match self {
            Self::Switch(_) => return Ok(self),
            Self::Ceiling(db) => ("ceiling", db, LIMIT_CEILING_DB, "dB"),
            Self::Lookahead(ms) => ("lookahead", ms, LIMIT_LOOKAHEAD_MS, "ms"),
        };
        if (low..=high).contains(&value) {
            Ok(self)
        } else {
            Err(format!(
                "the {what} of a limiter must be {low} to {high} {unit}, not {value}"
            ))
        }
    }
}

/// The limiter of one bus as the control thread sets it. Off at first; a route switches it on.
#[derive(Debug)]
pub struct LimiterControls {
    on: AtomicBool,
    ceiling_db: AtomicF32,
    lookahead_ms: AtomicF32,
}

impl Default for LimiterControls {
    fn default() -> Self {
        Self {
            on: AtomicBool::new(false),
            ceiling_db: AtomicF32::new(DEFAULT_CEILING_DB),
            lookahead_ms: AtomicF32::new(DEFAULT_LOOKAHEAD_MS),
        }
    }
}

impl LimiterControls {
    /// Makes one change.
    pub fn change(&self, change: LimitChange) {
        match change {
            LimitChange::Switch(on) => self.switch(on),
            LimitChange::Ceiling(db) => self.set_ceiling(db),
            LimitChange::Lookahead(ms) => self.set_lookahead(ms),
        }
    }

    /// Sets everything at once: what a route starts with.
    pub fn set_all(&self, on: bool, ceiling_db: f32, lookahead_ms: f32) {
        self.set_ceiling(ceiling_db);
        self.set_lookahead(lookahead_ms);
        self.switch(on);
    }

    /// Sets the ceiling, clamped to [`LIMIT_CEILING_DB`]; a NaN becomes the default.
    pub fn set_ceiling(&self, db: f32) {
        self.ceiling_db
            .store(within(db, LIMIT_CEILING_DB, DEFAULT_CEILING_DB));
    }

    /// Sets the lookahead, clamped to [`LIMIT_LOOKAHEAD_MS`]; a NaN becomes the default.
    pub fn set_lookahead(&self, ms: f32) {
        self.lookahead_ms
            .store(within(ms, LIMIT_LOOKAHEAD_MS, DEFAULT_LOOKAHEAD_MS));
    }

    /// Switches it on or off; off, it keeps its settings.
    pub fn switch(&self, on: bool) {
        self.on.store(on, Ordering::Release);
    }

    /// Whether it is switched on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }

    /// The ceiling as it is set, in dB.
    #[must_use]
    pub fn ceiling_db(&self) -> f32 {
        self.ceiling_db.load()
    }

    /// The lookahead as it is set, in milliseconds.
    #[must_use]
    pub fn lookahead_ms(&self) -> f32 {
        self.lookahead_ms.load()
    }
}

/// A frame's own gain that still counts for the frames after it.
#[derive(Clone, Copy, Debug, Default)]
struct Hold {
    /// The frame's number.
    at: u64,
    gain: f64,
}

/// The limiter inside a bus's callback. Everything it needs is made beforehand.
#[derive(Debug)]
pub struct Limiter {
    /// Frames per second.
    rate: f64,
    channels: usize,
    /// Frames the delay line and the other windows hold: the longest lookahead and one.
    capacity: usize,
    /// The sound of the last `capacity` frames, interleaved.
    line: Vec<f32>,
    /// The frame in `line` to write next.
    write: usize,
    /// Nothing played yet, or not for a while: it stands at what it sees next.
    fresh: bool,
    on: bool,
    /// Where the ceiling is to go, and where it stands, in dB and as a linear factor.
    goal_db: f32,
    now_db: f32,
    ceiling: f64,
    /// Change of the ceiling per frame in dB.
    pace: f32,
    /// The lookahead to reach, and the one in use, in frames.
    goal_look: usize,
    look: usize,
    /// The delay a crossfade leaves, and its frames to go.
    fading_from: usize,
    fade_left: usize,
    fade_frames: usize,
    /// Weight of the rest of the way in the follower, per frame.
    release: f64,
    /// The follower.
    follower: f64,
    /// Frames the follower has been back at 1.
    back_for: usize,
    /// The frames' own gains below 1, ever larger from `head` on, as a ring.
    holds: Vec<Hold>,
    head: usize,
    held: usize,
    /// The number of the current frame.
    clock: u64,
    /// The last `look + 1` values of the follower, as a ring, and their sum.
    recent: Vec<f64>,
    place: usize,
    sum: f64,
}

impl Limiter {
    /// A limiter at `rate` frames per second for `channels` channels, which stands at what it sees
    /// first.
    #[must_use]
    pub fn new(rate: u32, channels: usize) -> Self {
        let rate = f64::from(rate.max(1));
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let capacity = (rate * f64::from(LIMIT_LOOKAHEAD_MS.1) / 1000.0).ceil() as usize + 1;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let fade_frames = ((rate * FADE_MS / 1000.0) as usize).max(1);
        #[allow(clippy::cast_possible_truncation)]
        let pace = (f64::from(CEILING_PACE) / rate) as f32;
        Self {
            rate,
            channels,
            capacity,
            line: vec![0.0; capacity * channels],
            write: 0,
            fresh: true,
            on: false,
            goal_db: OUT_OF_THE_WAY_DB,
            now_db: OUT_OF_THE_WAY_DB,
            ceiling: f64::from(10f32.powf(OUT_OF_THE_WAY_DB / 20.0)),
            pace,
            goal_look: 0,
            look: 0,
            fading_from: 0,
            fade_left: 0,
            fade_frames,
            release: 1.0 - (-1000.0 / (rate * RELEASE_MS)).exp(),
            follower: 1.0,
            back_for: usize::MAX,
            holds: vec![Hold::default(); capacity],
            head: 0,
            held: 0,
            clock: 0,
            recent: vec![1.0; capacity],
            place: 0,
            sum: 1.0,
        }
    }

    /// Gets ready for a device that opens, maybe with another layout; not for the callback, since
    /// it may allocate. It stands at what it sees next, with nothing remembered: nothing played
    /// for a while, so nothing can click.
    pub fn prepare(&mut self, channels: usize) {
        if channels != self.channels {
            self.channels = channels;
            self.line = vec![0.0; self.capacity * channels];
        }
        self.fresh = true;
    }

    /// Limits `data` (interleaved, `channels` per frame) in place as `controls` say. Returns how
    /// far it turned down at most, in dB; 0 when it did not.
    pub fn process(
        &mut self,
        controls: &LimiterControls,
        data: &mut [f32],
        channels: usize,
    ) -> f32 {
        // Another layout comes with `prepare`; the bus's clamp holds until then.
        if channels == 0 || channels != self.channels {
            return 0.0;
        }
        self.follow(controls);
        if self.rests() {
            self.remember(data);
            return 0.0;
        }
        let mut lowest = 1.0_f64;
        for frame in data.chunks_exact_mut(channels) {
            let gain = self.step(frame);
            lowest = lowest.min(gain);
        }
        #[allow(clippy::cast_possible_truncation)]
        let lowest = lowest as f32;
        // Not `-0.0`, and nothing to do with a gain of exactly 1.
        if lowest < 1.0 {
            (-linear_to_db(lowest)).min(MOST_REPORTED_DB)
        } else {
            0.0
        }
    }

    /// Reads the settings.
    fn follow(&mut self, controls: &LimiterControls) {
        self.on = controls.is_on();
        self.goal_db = if self.on {
            controls.ceiling_db()
        } else {
            OUT_OF_THE_WAY_DB
        };
        self.goal_look = if self.on {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let frames = (f64::from(controls.lookahead_ms()) * self.rate / 1000.0).round() as usize;
            frames.min(self.capacity - 1)
        } else {
            0
        };
        if std::mem::take(&mut self.fresh) {
            self.line.fill(0.0);
            self.write = 0;
            self.now_db = self.goal_db;
            self.ceiling = ceiling_of(self.now_db);
            self.look = self.goal_look;
            self.fade_left = 0;
            self.follower = 1.0;
            self.back_for = usize::MAX;
            self.held = 0;
            self.fill_recent();
        }
    }

    /// Whether it is switched off and out of the way: all that is left is the delay line.
    fn rests(&self) -> bool {
        #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
        let out_of_the_way = !self.on && self.now_db == OUT_OF_THE_WAY_DB;
        out_of_the_way && self.look == 0 && self.fade_left == 0 && self.settled()
    }

    /// Whether the gain is exactly 1: the follower has been back for a whole window.
    fn settled(&self) -> bool {
        self.back_for > self.look
    }

    /// Writes `data` into the delay line, untouched.
    fn remember(&mut self, data: &[f32]) {
        for frame in data.chunks_exact(self.channels) {
            let at = self.write * self.channels;
            self.line[at..at + self.channels].copy_from_slice(frame);
            self.write = (self.write + 1) % self.capacity;
        }
    }

    /// One frame: returns the gain it left with.
    fn step(&mut self, frame: &mut [f32]) -> f64 {
        self.glide();
        if self.fade_left == 0 && self.look != self.goal_look {
            self.fading_from = self.look;
            self.look = self.goal_look;
            self.fade_left = self.fade_frames;
            self.fill_recent();
        }
        let at = self.write * self.channels;
        self.line[at..at + self.channels].copy_from_slice(frame);
        let gain = self.gain_for(frame);
        let new = self.tap(self.look);
        #[allow(clippy::cast_precision_loss)]
        let old = (self.fade_left > 0).then(|| {
            let share = self.fade_left as f64 / self.fade_frames as f64;
            (self.tap(self.fading_from), share)
        });
        for (channel, sample) in frame.iter_mut().enumerate() {
            let delayed = f64::from(self.line[new + channel]);
            let sound = match old {
                Some((old, share)) => {
                    delayed * (1.0 - share) + f64::from(self.line[old + channel]) * share
                }
                None => delayed,
            };
            // A NaN stays one here and the bus silences it.
            #[allow(clippy::cast_possible_truncation)]
            let out = (sound * gain) as f32;
            *sample = out;
        }
        self.fade_left = self.fade_left.saturating_sub(1);
        self.write = (self.write + 1) % self.capacity;
        gain
    }

    /// Where in `line` the frame `delay` frames before the one being written starts.
    fn tap(&self, delay: usize) -> usize {
        (self.write + self.capacity - delay) % self.capacity * self.channels
    }

    /// Moves the ceiling a frame's way towards its goal.
    fn glide(&mut self) {
        #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
        if self.now_db != self.goal_db {
            self.now_db = if self.now_db < self.goal_db {
                (self.now_db + self.pace).min(self.goal_db)
            } else {
                (self.now_db - self.pace).max(self.goal_db)
            };
            self.ceiling = ceiling_of(self.now_db);
        }
    }

    /// The gain of the frame that leaves now, given the one that comes in.
    fn gain_for(&mut self, frame: &[f32]) -> f64 {
        // `max` passes over a NaN: a broken sample does not move the gain.
        let peak = f64::from(frame.iter().fold(0.0_f32, |max, s| max.max(s.abs())));
        self.clock += 1;
        if peak > self.ceiling {
            self.hold(self.ceiling / peak);
        }
        while self.held > 0 && self.clock - self.holds[self.head].at > self.look as u64 {
            self.head = (self.head + 1) % self.capacity;
            self.held -= 1;
        }
        self.follower += (1.0 - self.follower) * self.release;
        if self.follower >= BACK {
            self.follower = 1.0;
        }
        if self.held > 0 {
            self.follower = self.follower.min(self.holds[self.head].gain);
        }
        #[allow(clippy::float_cmp)] // Exactly: set to 1 just above.
        if self.follower == 1.0 {
            self.back_for = self.back_for.saturating_add(1);
        } else {
            self.back_for = 0;
        }
        self.sum += self.follower - self.recent[self.place];
        self.recent[self.place] = self.follower;
        self.place = (self.place + 1) % (self.look + 1);
        if self.settled() {
            // Just there: whatever rounding left in the sum goes here.
            if self.back_for == self.look + 1 {
                self.fill_recent();
            }
            1.0
        } else {
            #[allow(clippy::cast_precision_loss)]
            let window = (self.look + 1) as f64;
            self.sum / window
        }
    }

    /// Adds a frame's own gain: those it undercuts no longer matter.
    fn hold(&mut self, gain: f64) {
        while self.held > 0 {
            let last = (self.head + self.held - 1) % self.capacity;
            if self.holds[last].gain < gain {
                break;
            }
            self.held -= 1;
        }
        // Frames that passed already leave in `gain_for`, so there is always room: a window
        // is at most `capacity` frames.
        if self.held == self.capacity {
            self.head = (self.head + 1) % self.capacity;
            self.held -= 1;
        }
        let place = (self.head + self.held) % self.capacity;
        self.holds[place] = Hold {
            at: self.clock,
            gain,
        };
        self.held += 1;
    }

    /// Starts the average over at where the follower stands.
    fn fill_recent(&mut self) {
        self.recent[..=self.look].fill(self.follower);
        self.place = 0;
        #[allow(clippy::cast_precision_loss)]
        let window = (self.look + 1) as f64;
        self.sum = self.follower * window;
    }
}

/// The ceiling at `db` as a linear factor.
fn ceiling_of(db: f32) -> f64 {
    10f64.powf(f64::from(db) / 20.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    fn linear(db: f32) -> f32 {
        crate::level::db_to_linear(db)
    }

    fn limiter(ceiling_db: f32, lookahead_ms: f32, channels: usize) -> (Limiter, LimiterControls) {
        let controls = LimiterControls::default();
        controls.set_all(true, ceiling_db, lookahead_ms);
        (Limiter::new(RATE, channels), controls)
    }

    /// Pseudo-noise with loud peaks, reproducible.
    fn noise(frames: usize, channels: usize, loudest: f32) -> Vec<f32> {
        let mut state = 1_u32;
        (0..frames * channels)
            .map(|_| {
                state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                let sample = (state >> 9) as f32 / 4_194_304.0 - 1.0;
                // Every now and then a peak far above the rest.
                if state.is_multiple_of(101) {
                    sample * loudest
                } else {
                    sample * 0.2
                }
            })
            .collect()
    }

    fn peak(samples: &[f32]) -> f32 {
        samples.iter().fold(0.0, |max, s| max.max(s.abs()))
    }

    #[test]
    fn the_ceiling_holds_for_every_lookahead_and_ceiling() {
        for lookahead_ms in [0.0, 0.1, 1.0, 2.5, 5.0] {
            for ceiling_db in [0.0, -0.3, -6.0, -20.0] {
                let (mut limiter, controls) = limiter(ceiling_db, lookahead_ms, 2);
                let mut data = noise(48_000, 2, 30.0);
                for block in data.chunks_mut(960) {
                    limiter.process(&controls, block, 2);
                }
                let allowed = linear(ceiling_db) * (1.0 + 1e-6);
                assert!(
                    peak(&data) <= allowed,
                    "{} above {allowed} at {lookahead_ms} ms, {ceiling_db} dB",
                    peak(&data)
                );
            }
        }
    }

    #[test]
    fn the_ceiling_holds_however_long_the_callbacks_are() {
        for block in [1, 64, 441, 4096] {
            let (mut limiter, controls) = limiter(-3.0, 1.0, 2);
            let mut data = noise(20_000, 2, 10.0);
            for part in data.chunks_mut(block * 2) {
                limiter.process(&controls, part, 2);
            }
            assert!(peak(&data) <= linear(-3.0) * (1.0 + 1e-6));
        }
    }

    #[test]
    fn below_the_ceiling_the_sound_only_comes_later() {
        let (mut limiter, controls) = limiter(-0.3, 1.0, 2);
        let input: Vec<f32> = noise(4800, 2, 3.0).iter().map(|s| s * 0.2).collect();
        assert!(peak(&input) < 0.9);
        let mut data = input.clone();
        assert_eq!(limiter.process(&controls, &mut data, 2), 0.0);
        let look = 48;
        assert_eq!(limiter.look, look);
        assert!(data[..look * 2].iter().all(|&s| s == 0.0));
        for (out, original) in data[look * 2..].iter().zip(&input) {
            assert!((out - original).abs() < 1e-6, "{out} against {original}");
        }
    }

    #[test]
    fn without_lookahead_it_adds_no_delay_and_changes_no_sample_below_the_ceiling() {
        let (mut limiter, controls) = limiter(0.0, 0.0, 2);
        let input = vec![0.25, -0.5, 0.75, -0.125];
        let mut data = input.clone();
        assert_eq!(limiter.process(&controls, &mut data, 2), 0.0);
        assert_eq!(data, input);
    }

    #[test]
    fn all_channels_share_the_gain() {
        let (mut limiter, controls) = limiter(-6.0, 0.0, 2);
        // A loud left channel turns the quiet right one down with it.
        let mut data = vec![1.0, 0.1];
        limiter.process(&controls, &mut data, 2);
        let gain = data[0];
        assert!((gain - linear(-6.0)).abs() < 1e-5);
        assert!((data[1] - 0.1 * gain).abs() < 1e-5);
    }

    #[test]
    fn it_reports_how_far_it_turned_down() {
        let (mut limiter, controls) = limiter(-6.0, 0.0, 1);
        // 0 dB against a ceiling of -6 dB.
        let reduced = limiter.process(&controls, &mut [1.0, 0.0], 1);
        assert!((reduced - 6.0).abs() < 0.01, "{reduced}");
        assert_eq!(
            Limiter::new(RATE, 1).process(&LimiterControls::default(), &mut [1.0], 1),
            0.0
        );
    }

    #[test]
    fn the_gain_comes_back_within_its_release_and_ends_at_exactly_one() {
        let (mut limiter, controls) = limiter(-6.0, 0.0, 1);
        limiter.process(&controls, &mut [1.0], 1);
        // One release time later 63 % of the way is done.
        let mut quiet = vec![0.1; 4800];
        limiter.process(&controls, &mut quiet, 1);
        let gain = f64::from(quiet[4799]) / 0.1;
        let start = ceiling_of(-6.0);
        let expected = start + (1.0 - start) * (1.0 - (-1.0_f64).exp());
        assert!((gain - expected).abs() < 0.01, "{gain} against {expected}");
        // Much later nothing is left at all.
        let mut quiet = vec![0.1; 96_000];
        limiter.process(&controls, &mut quiet, 1);
        assert_eq!(quiet[95_999], 0.1);
    }

    /// The largest step the tone takes when a spike at 2.0 passes through a tone at 0.4: the spike
    /// itself left out, so that the frames on either side of it count as neighbours.
    fn largest_step_around_a_spike(lookahead_ms: f32) -> f32 {
        let (mut limiter, controls) = limiter(-6.0, lookahead_ms, 1);
        let spike = 2400;
        let mut data = vec![0.4; 4800];
        data[spike] = 2.0;
        limiter.process(&controls, &mut data, 1);
        let look = limiter.look;
        let at = spike + look;
        assert!(data[at] <= linear(-6.0) * (1.0 + 1e-6));
        let across = (data[at + 1] - data[at - 1]).abs();
        // From where the sound starts: before it, the delay line was silent.
        data[look..at]
            .windows(2)
            .chain(data[at + 1..].windows(2))
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(across, f32::max)
    }

    #[test]
    fn the_gain_moves_without_a_step_given_lookahead() {
        // Without lookahead the tone drops at once from 0.4 to 0.1.
        assert!(largest_step_around_a_spike(0.0) > 0.25);
        // With it the same way takes 240 frames.
        let gentle = largest_step_around_a_spike(5.0);
        assert!(gentle < 0.3 / 240.0 * 1.5, "{gentle}");
    }

    #[test]
    fn switched_off_it_gets_out_of_the_way_and_drops_its_delay() {
        let (mut limiter, controls) = limiter(-6.0, 2.0, 1);
        let mut data = vec![1.0; 4800];
        limiter.process(&controls, &mut data, 1);
        assert!(peak(&data) <= linear(-6.0) * (1.0 + 1e-6));
        controls.switch(false);
        // Long enough for the slide, the crossfade and the release.
        let mut data = vec![0.9; 96_000];
        limiter.process(&controls, &mut data, 1);
        // Nothing is limited or delayed any more, and it rests.
        assert!(limiter.rests());
        let mut data = vec![0.5, 1.5, -2.5];
        assert_eq!(limiter.process(&controls, &mut data, 1), 0.0);
        assert_eq!(data, [0.5, 1.5, -2.5]);
    }

    #[test]
    fn switched_on_again_it_plays_what_it_kept_without_a_gap() {
        let (mut limiter, controls) = limiter(-6.0, 2.0, 1);
        controls.switch(false);
        let mut data = vec![0.1; 100];
        limiter.process(&controls, &mut data, 1);
        controls.switch(true);
        let mut data = vec![0.1; 4800];
        limiter.process(&controls, &mut data, 1);
        assert!(data.iter().all(|&s| (s - 0.1).abs() < 1e-6));
    }

    #[test]
    fn a_new_lookahead_crossfades_and_arrives() {
        let (mut limiter, controls) = limiter(-0.3, 1.0, 1);
        let mut data = vec![0.5; 480];
        limiter.process(&controls, &mut data, 1);
        controls.set_lookahead(5.0);
        // A constant tone stays constant through the crossfade: it does not dip.
        let mut data = vec![0.5; 4800];
        limiter.process(&controls, &mut data, 1);
        assert!(data.iter().all(|&s| (s - 0.5).abs() < 1e-5));
        assert_eq!(limiter.look, 240);
    }

    #[test]
    fn a_new_ceiling_slides() {
        let (mut limiter, controls) = limiter(0.0, 0.0, 1);
        let mut data = vec![1.0; 480];
        limiter.process(&controls, &mut data, 1);
        controls.set_ceiling(-12.0);
        let mut data = vec![1.0; 4800];
        limiter.process(&controls, &mut data, 1);
        // No frame takes more than a small part of the 12 dB ...
        for pair in data.windows(2) {
            assert!((pair[1] - pair[0]).abs() < 0.01);
        }
        // ... and it arrives.
        assert!((data[4799] - linear(-12.0)).abs() < 1e-5);
    }

    #[test]
    fn broken_samples_do_not_move_the_gain() {
        let (mut limiter, controls) = limiter(-6.0, 0.0, 2);
        let mut data = vec![f32::NAN, 0.25];
        assert_eq!(limiter.process(&controls, &mut data, 2), 0.0);
        assert!(data[0].is_nan());
        assert_eq!(data[1], 0.25);
    }

    #[test]
    fn an_infinite_sample_turns_it_down_and_it_recovers() {
        let (mut limiter, controls) = limiter(-6.0, 0.0, 1);
        let reported = limiter.process(&controls, &mut [f32::INFINITY], 1);
        assert_eq!(reported, MOST_REPORTED_DB, "a number JSON can say");
        let mut quiet = vec![0.1; 96_000];
        limiter.process(&controls, &mut quiet, 1);
        assert!(quiet[95_999].is_finite());
        assert!((quiet[95_999] - 0.1).abs() < 1e-5);
    }

    #[test]
    fn another_layout_is_passed_over_until_it_is_prepared() {
        let (mut limiter, controls) = limiter(-6.0, 0.0, 2);
        let mut data = vec![1.0, 1.0, 1.0];
        assert_eq!(limiter.process(&controls, &mut data, 3), 0.0);
        assert_eq!(data, [1.0, 1.0, 1.0]);
        limiter.prepare(3);
        limiter.process(&controls, &mut data, 3);
        assert!(peak(&data) <= linear(-6.0) * (1.0 + 1e-6));
    }

    #[test]
    fn starting_over_stands_at_the_settings_with_nothing_remembered() {
        let (mut limiter, controls) = limiter(-6.0, 1.0, 1);
        let mut data = vec![1.0; 480];
        limiter.process(&controls, &mut data, 1);
        limiter.prepare(1);
        controls.set_all(true, -12.0, 2.0);
        // No slide, no crossfade, and the old sound is gone.
        let mut data = vec![0.1; 96];
        assert_eq!(limiter.process(&controls, &mut data, 1), 0.0);
        assert!(data.iter().all(|&s| s == 0.0));
        assert_eq!(limiter.look, 96);
        assert!((limiter.ceiling - ceiling_of(-12.0)).abs() < 1e-9);
    }

    #[test]
    fn settings_are_kept_in_their_ranges() {
        let controls = LimiterControls::default();
        assert!(!controls.is_on());
        controls.set_ceiling(6.0);
        assert_eq!(controls.ceiling_db(), LIMIT_CEILING_DB.1);
        controls.set_ceiling(f32::NAN);
        assert_eq!(controls.ceiling_db(), DEFAULT_CEILING_DB);
        controls.set_lookahead(-1.0);
        assert_eq!(controls.lookahead_ms(), LIMIT_LOOKAHEAD_MS.0);
        controls.set_lookahead(f32::NAN);
        assert_eq!(controls.lookahead_ms(), DEFAULT_LOOKAHEAD_MS);
    }

    #[test]
    fn the_longest_lookahead_fits_at_every_rate() {
        for rate in [8_000, 44_100, 48_000, 96_000, 192_000, 384_000] {
            let controls = LimiterControls::default();
            controls.set_all(true, -6.0, LIMIT_LOOKAHEAD_MS.1);
            let mut limiter = Limiter::new(rate, 2);
            let mut data = noise(rate as usize / 10, 2, 8.0);
            limiter.process(&controls, &mut data, 2);
            assert!(peak(&data) <= linear(-6.0) * (1.0 + 1e-6));
        }
    }
}
