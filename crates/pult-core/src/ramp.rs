//! Values that glide instead of jumping: what a gain or a pan is asked to become, and how the
//! audio callback gets there.
//!
//! A controller gives a target and a time ([`Glide`]), not a stream of values. Every ring and
//! every bus follows on its own output clock with a [`Follower`], one small step per frame, so
//! nothing in a callback allocates or waits. A change without a time still takes
//! [`MIN_RAMP_MS`]: a jump in the gain is a click in the sound.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

/// The shortest change: long enough to take the click out of a step, too short to hear as a
/// fade.
pub const MIN_RAMP_MS: u32 = 5;

/// The longest ramp anyone may ask for.
pub const MAX_RAMP_MS: u32 = 60_000;

/// Set only by the sabotage probe of the click test (`pult-engine/tests/clicks_sabotage.rs`):
/// every ramp jumps to its goal at once, as if there were no smoothing. Nothing else sets it.
static UNSMOOTHED: AtomicBool = AtomicBool::new(false);

/// Switches the smoothing of all ramps in this process off (`true`) or on again: for the sabotage
/// probe of the click test only, which shows that its detector hears what a missing ramp does.
#[doc(hidden)]
pub fn sabotage_smoothing(off: bool) {
    UNSMOOTHED.store(off, Ordering::Relaxed);
}

/// A target and the time to reach it, as one word: the audio thread never sees a new value with
/// the time of an old one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Aim(u64);

impl Aim {
    fn new(value: f32, ms: u32) -> Self {
        Self(u64::from(value.to_bits()) << 32 | u64::from(ms))
    }

    /// The value to reach.
    #[must_use]
    pub fn value(self) -> f32 {
        #[allow(clippy::cast_possible_truncation)]
        f32::from_bits((self.0 >> 32) as u32)
    }

    /// Milliseconds to reach it in.
    #[must_use]
    pub fn ms(self) -> u32 {
        #[allow(clippy::cast_possible_truncation)]
        let ms = self.0 as u32;
        ms
    }
}

/// A value the control thread sets and the audio threads follow, without a lock.
#[derive(Debug)]
pub struct Glide(AtomicU64);

impl Glide {
    /// A new value.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self(AtomicU64::new(Aim::new(value, 0).0))
    }

    /// Asks for `value` as soon as that is free of clicks.
    pub fn store(&self, value: f32) {
        self.ramp_to(value, 0);
    }

    /// Asks for `value`, reached in `ms` milliseconds from wherever it is then.
    pub fn ramp_to(&self, value: f32, ms: u32) {
        self.0.store(Aim::new(value, ms).0, Ordering::Relaxed);
    }

    /// What is asked for now.
    pub fn load(&self) -> Aim {
        Aim(self.0.load(Ordering::Relaxed))
    }
}

/// A straight line from where the value is to a goal, one step per frame.
#[derive(Debug, Clone, Copy)]
pub struct Ramp {
    // In f64: over a minute of frames the steps of an f32 add up to an audible error, and the
    // last step would have to make up for it.
    current: f64,
    goal: f32,
    step: f64,
    left: u32,
    frames_per_ms: f64,
}

impl Ramp {
    /// A steady `value` at `rate` frames per second.
    #[must_use]
    pub fn new(rate: u32, value: f32) -> Self {
        Self {
            current: f64::from(value),
            goal: value,
            step: 0.0,
            left: 0,
            frames_per_ms: f64::from(rate) / 1000.0,
        }
    }

    /// Stands at `value` at once.
    pub fn snap(&mut self, value: f32) {
        self.current = f64::from(value);
        self.goal = value;
        self.left = 0;
    }

    /// Goes to `goal` from here, in `ms` milliseconds but at least [`MIN_RAMP_MS`]. Stays
    /// steady when it stands there already.
    pub fn aim(&mut self, goal: f32, ms: u32) {
        #[allow(clippy::float_cmp)] // Exactly: a ramp ends on its goal.
        if self.is_steady() && self.current == f64::from(goal) {
            return;
        }
        if UNSMOOTHED.load(Ordering::Relaxed) {
            self.snap(goal);
            return;
        }
        let frames = (f64::from(ms.max(MIN_RAMP_MS)) * self.frames_per_ms).round();
        // At least one frame; the cast saturates far beyond any time allowed here.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = frames.max(1.0) as u32;
        self.goal = goal;
        self.step = (f64::from(goal) - self.current) / f64::from(frames);
        self.left = frames;
    }

    /// Where it stands.
    #[must_use]
    pub fn value(&self) -> f32 {
        #[allow(clippy::cast_possible_truncation)]
        let value = self.current as f32;
        value
    }

    /// Whether it has arrived.
    #[must_use]
    pub fn is_steady(&self) -> bool {
        self.left == 0
    }

    /// The value for the next frame; ends exactly on the goal.
    pub fn next_value(&mut self) -> f32 {
        if self.left > 0 {
            self.left -= 1;
            self.current = if self.left == 0 {
                f64::from(self.goal)
            } else {
                self.current + self.step
            };
        }
        self.value()
    }
}

/// One callback's way of following a [`Glide`]: it stands at the first value it sees, and
/// ramps to every later one.
#[derive(Debug, Clone, Copy)]
pub struct Follower {
    /// The line it walks.
    pub ramp: Ramp,
    seen: Aim,
    sounding: bool,
    fresh: bool,
}

impl Follower {
    /// A follower at `rate` frames per second, which stands at what it sees first.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        Self {
            ramp: Ramp::new(rate, 0.0),
            seen: Aim::new(0.0, 0),
            sounding: false,
            fresh: true,
        }
    }

    /// Follows `aim`, or goes to silence while not `sounding`. A change of the aim takes the
    /// aim's time, a change of `sounding` takes `switch_ms`.
    pub fn follow(&mut self, aim: Aim, sounding: bool, switch_ms: u32) {
        let goal = if sounding { aim.value() } else { 0.0 };
        if self.fresh {
            self.fresh = false;
            self.ramp.snap(goal);
        } else {
            let moved = aim != self.seen;
            let switched = sounding != self.sounding;
            if moved || switched {
                let moved_ms = if moved { aim.ms() } else { 0 };
                let switched_ms = if switched { switch_ms } else { 0 };
                self.ramp.aim(goal, moved_ms.max(switched_ms));
            }
        }
        self.seen = aim;
        self.sounding = sounding;
    }

    /// Stands at silence, ready to ramp up to `aim` when it sounds: a ring that starts to play.
    /// How the aim got there does not count, it may be long ago.
    pub fn silence(&mut self, aim: Aim) {
        self.ramp.snap(0.0);
        self.seen = aim;
        self.sounding = false;
        self.fresh = false;
    }

    /// Nothing sounds for now: a way down to silence is over at once, since no one would hear
    /// it. Any other ramp waits for the sound.
    pub fn hold(&mut self) {
        if !self.sounding {
            self.ramp.snap(0.0);
        }
    }

    /// Stands at what it sees next: nothing played for a while, so nothing can click.
    pub fn start_over(&mut self) {
        self.fresh = true;
    }

    /// Whether it has arrived at silence.
    #[must_use]
    pub fn is_silent(&self) -> bool {
        #[allow(clippy::float_cmp)] // Exactly: a ramp ends on its goal.
        let silent = self.ramp.current == 0.0;
        self.ramp.is_steady() && silent
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_aim_keeps_value_and_time_together() {
        let glide = Glide::new(-0.5);
        assert_eq!(glide.load().value(), -0.5);
        assert_eq!(glide.load().ms(), 0);
        glide.ramp_to(0.25, MAX_RAMP_MS);
        assert_eq!(glide.load().value(), 0.25);
        assert_eq!(glide.load().ms(), MAX_RAMP_MS);
        glide.store(1.0);
        assert_eq!(glide.load().ms(), 0);
    }

    #[test]
    fn a_ramp_is_a_straight_line_that_ends_on_its_goal() {
        // 1000 frames per second: a frame is a millisecond, and the shortest ramp 5 frames.
        let mut ramp = Ramp::new(1000, 0.0);
        ramp.aim(1.0, 10);
        let values: Vec<f32> = (0..10).map(|_| ramp.next_value()).collect();
        for (frame, value) in values.iter().enumerate() {
            assert!(
                (value - (frame + 1) as f32 / 10.0).abs() < 1e-6,
                "{values:?}"
            );
        }
        assert!(ramp.is_steady());
        assert_eq!(ramp.value(), 1.0);
        assert_eq!(ramp.next_value(), 1.0);
    }

    #[test]
    fn no_change_is_shorter_than_the_shortest_ramp() {
        let mut ramp = Ramp::new(48_000, 1.0);
        ramp.aim(0.0, 0);
        let frames = (MIN_RAMP_MS * 48) as usize;
        for _ in 1..frames {
            ramp.next_value();
            assert!(!ramp.is_steady());
        }
        assert_eq!(ramp.next_value(), 0.0);
        assert!(ramp.is_steady());
    }

    #[test]
    fn a_new_aim_turns_from_where_it_stands() {
        let mut ramp = Ramp::new(1000, 0.0);
        ramp.aim(1.0, 100);
        for _ in 0..50 {
            ramp.next_value();
        }
        let halfway = ramp.value();
        assert!((halfway - 0.5).abs() < 1e-6);
        ramp.aim(0.0, 100);
        assert!((ramp.next_value() - halfway).abs() < 0.006);
    }

    #[test]
    fn the_longest_ramp_arrives_without_a_step_at_its_end() {
        let mut ramp = Ramp::new(96_000, 1.0);
        ramp.aim(0.001, MAX_RAMP_MS);
        let step = (1.0 - 0.001) / (f64::from(MAX_RAMP_MS) * 96.0);
        let mut before = ramp.value();
        let mut last = 0.0;
        while !ramp.is_steady() {
            let value = ramp.next_value();
            last = f64::from((value - before).abs());
            before = value;
        }
        assert_eq!(ramp.value(), 0.001);
        // Within the rounding of an f32 around the goal.
        assert!(last < step + 1e-6, "{last} against a step of {step}");
    }

    #[test]
    fn a_follower_stands_at_what_it_sees_first_and_ramps_to_the_rest() {
        let glide = Glide::new(0.5);
        let mut follower = Follower::new(1000);
        follower.follow(glide.load(), true, 0);
        assert_eq!(follower.ramp.value(), 0.5);
        assert!(follower.ramp.is_steady());

        // The same aim again changes nothing.
        follower.follow(glide.load(), true, 0);
        assert!(follower.ramp.is_steady());

        glide.ramp_to(1.0, 100);
        follower.follow(glide.load(), true, 0);
        assert!(!follower.ramp.is_steady());
        for _ in 0..100 {
            follower.ramp.next_value();
        }
        assert_eq!(follower.ramp.value(), 1.0);
    }

    #[test]
    fn a_follower_goes_to_silence_in_the_time_of_the_switch() {
        let glide = Glide::new(0.5);
        let mut follower = Follower::new(1000);
        follower.follow(glide.load(), true, 0);
        follower.follow(glide.load(), false, 20);
        assert!(!follower.is_silent());
        for _ in 0..19 {
            follower.ramp.next_value();
            assert!(!follower.is_silent());
        }
        follower.ramp.next_value();
        assert!(follower.is_silent());

        // A new gain while silent stays silent, and is there when it sounds again.
        glide.store(0.25);
        follower.follow(glide.load(), false, 0);
        assert!(follower.is_silent());
        follower.follow(glide.load(), true, 0);
        for _ in 0..5 {
            follower.ramp.next_value();
        }
        assert_eq!(follower.ramp.value(), 0.25);
    }

    #[test]
    fn a_follower_starting_from_silence_forgets_how_the_aim_got_there() {
        let glide = Glide::new(0.5);
        glide.ramp_to(1.0, MAX_RAMP_MS);
        let mut follower = Follower::new(1000);
        follower.silence(glide.load());
        // The time of the switch counts, not the minute of a ramp from long ago.
        follower.follow(glide.load(), true, 0);
        for _ in 0..5 {
            follower.ramp.next_value();
        }
        assert_eq!(follower.ramp.value(), 1.0);
    }

    #[test]
    fn holding_ends_only_a_way_down_to_silence() {
        let glide = Glide::new(1.0);
        let mut follower = Follower::new(1000);
        follower.follow(glide.load(), true, 0);
        glide.ramp_to(0.5, 100);
        follower.follow(glide.load(), true, 0);
        follower.hold();
        assert_eq!(follower.ramp.value(), 1.0);
        assert!(!follower.ramp.is_steady());

        follower.follow(glide.load(), false, 100);
        follower.hold();
        assert!(follower.is_silent());

        // A fade-in waits.
        follower.follow(glide.load(), true, 100);
        follower.hold();
        assert!(!follower.is_silent());
        assert_eq!(follower.ramp.value(), 0.0);
    }

    #[test]
    fn a_follower_starting_from_silence_ramps_and_starting_over_stands() {
        let glide = Glide::new(1.0);
        let mut follower = Follower::new(1000);
        follower.silence(glide.load());
        follower.follow(glide.load(), true, 0);
        assert_eq!(follower.ramp.value(), 0.0);
        assert!(!follower.ramp.is_steady());

        follower.start_over();
        follower.follow(glide.load(), true, 0);
        assert_eq!(follower.ramp.value(), 1.0);
        assert!(follower.ramp.is_steady());
    }
}
