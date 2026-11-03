//! Plans each read from an input's ring and measures how far its level is off the target.
//!
//! Every device runs on its own clock: a virtual cable and a USB interface both claim
//! 48 kHz but differ by a few samples per second. Left alone, the ring between them drains
//! (dropouts) or fills up (growing delay). The steady correction is the resampler's job
//! ([`crate::rate`]); this control supplies its input, the (optionally smoothed) level error,
//! and keeps the safety net: priming before playing, silence after an underrun, and a hard
//! resync when the level is far too high.
//!
//! Right after the ring is primed the level settles where the two devices' start-up
//! buffering puts it, often well above the target. An optional settling phase measures that
//! offset and skips it once, instead of leaving the resampler to work it off slowly.

/// What one output callback does with one input's ring, in frames.
///
/// `read + silence` is always the frames the callback needs, and `skip + read` never
/// exceeds the frames available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Plan {
    /// Frames to discard before reading.
    pub skip: usize,
    /// Frames to read.
    pub read: usize,
    /// Frames of silence to append.
    pub silence: usize,
}

/// Why a plan differs from a plain read; the mixer counts these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    /// A plain read.
    None,
    /// Waiting for the ring to fill up to the target; silence meanwhile.
    Priming,
    /// The ring ran dry: the rest is silence, then it primes again.
    Underrun,
    /// The ring was far too full: skipped back to the target.
    Resync,
    /// The settling phase ended above the band: skipped `Plan::skip` frames back to the
    /// target once.
    Aligned,
}

/// The fill-level control of one input.
#[derive(Debug, Clone)]
pub struct DriftControl {
    target: usize,
    band: usize,
    primed: bool,
    /// The (smoothed) level after a read, in frames.
    level: f64,
    /// Weight of the newest level in the average; 1.0 is no smoothing.
    alpha: f64,
    /// Frames played after priming before the level counts as steady.
    settle: usize,
    /// Frames played since priming, up to `settle`.
    played: usize,
    /// Sum and count of the levels in the second half of the settling phase.
    settle_sum: f64,
    settle_count: u32,
}

impl DriftControl {
    /// A control that aims at `target` frames left after each read; a start-up offset above
    /// `target + band` is skipped once after settling. It follows every callback's level
    /// (no smoothing).
    #[must_use]
    pub fn new(target: usize, band: usize) -> Self {
        Self {
            target,
            band,
            primed: false,
            level: 0.0,
            alpha: 1.0,
            settle: 0,
            played: 0,
            settle_sum: 0.0,
            settle_count: 0,
        }
    }

    /// The same control measuring the level as a moving average, each callback weighing
    /// `alpha` (0 < alpha <= 1). Devices that deliver in large blocks make the level a
    /// sawtooth; its average stays put while real drift moves it. Underruns and resyncs
    /// still follow the actual level.
    #[must_use]
    pub fn smoothed(mut self, alpha: f64) -> Self {
        self.alpha = alpha.clamp(f64::MIN_POSITIVE, 1.0);
        self
    }

    /// The same control with a settling phase of `frames` played frames after every priming:
    /// no level error yet, the mean level of its second half measured, and at its end one
    /// skip back to the target if that mean lies above the band. A mean below the band is
    /// left to the resampler, since filling it at once would be a gap.
    #[must_use]
    pub fn settling(mut self, frames: usize) -> Self {
        self.settle = frames;
        self
    }

    /// The level beyond which the control skips back to the target at once. Block jitter
    /// can lift the level to nearly twice the target without any drift, so the limit lies
    /// above that. A smaller surplus that lasts (the average beyond
    /// [`Self::skip_threshold`]) is skipped too, but judged by the average.
    #[must_use]
    pub fn hard_limit(&self) -> usize {
        2 * self.target + 4 * self.band
    }

    /// How far the smoothed level may lie above the target, in frames, before the surplus
    /// counts as a backlog and is skipped: three times the band. Below it, a clock that
    /// changed its rate is left to the resampler.
    #[must_use]
    pub fn skip_threshold(&self) -> usize {
        3 * self.band
    }

    /// Starts over as if new: the next plans prime and settle again. For a ring that was
    /// emptied while its output was gone.
    pub fn reset(&mut self) {
        self.primed = false;
    }

    /// The smoothed level minus the target, in frames, once the ring is primed and settled;
    /// `None` while it primes or settles, when there is nothing to steer yet.
    #[must_use]
    pub fn steady_error(&self) -> Option<f64> {
        (self.primed && self.played >= self.settle).then_some(self.level - self.target as f64)
    }

    /// The plan for a callback that needs `needed` frames while `available` are buffered.
    pub fn plan(&mut self, available: usize, needed: usize) -> (Plan, Event) {
        if !self.primed {
            if available < self.target + needed {
                return (silence(needed), Event::Priming);
            }
            self.primed = true;
            // Start the average at the target: right after priming the level may sit on a
            // peak of the sawtooth, and starting there would steer against a mere peak.
            self.level = self.target as f64;
            self.played = 0;
            self.settle_sum = 0.0;
            self.settle_count = 0;
        }
        if available < needed {
            self.primed = false;
            let plan = Plan {
                read: available,
                silence: needed - available,
                ..Plan::default()
            };
            return (plan, Event::Underrun);
        }
        let after = available - needed;
        let read = Plan {
            read: needed,
            ..Plan::default()
        };
        if after > self.hard_limit() {
            self.level = self.target as f64;
            // A stalled device that catches up delivers more than one block: settle again,
            // so the rest of the burst is skipped once instead of steered off slowly.
            self.played = 0;
            self.settle_sum = 0.0;
            self.settle_count = 0;
            let plan = Plan {
                skip: after - self.target,
                ..read
            };
            return (plan, Event::Resync);
        }
        if self.played < self.settle {
            return self.settle_step(after, read);
        }
        self.level += self.alpha * (after as f64 - self.level);
        // A surplus the average still shows beyond the threshold is no drift but a backlog
        // (a busy machine held one side back). Steering it off would shift the pitch for
        // seconds; skipping it is one short jump. Never below the target, by the actual level.
        let limit = (self.target + self.skip_threshold()) as f64;
        if self.level > limit {
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let surplus = (self.level - self.target as f64).round() as usize;
            let skip = surplus.min(after.saturating_sub(self.target));
            if skip > 0 {
                self.level = self.target as f64;
                return (Plan { skip, ..read }, Event::Resync);
            }
        }
        (read, Event::None)
    }

    /// One callback of the settling phase, `after` frames left after the plain `read`.
    fn settle_step(&mut self, after: usize, read: Plan) -> (Plan, Event) {
        // A callback that needs nothing still counts, so the phase always ends.
        self.played = self.played.saturating_add(read.read.max(1));
        // The first half still carries the start-up transient; only the second is measured.
        if self.played > self.settle / 2 {
            self.settle_sum += after as f64;
            self.settle_count += 1;
        }
        if self.played < self.settle || self.settle_count == 0 {
            return (read, Event::None);
        }
        let mean = self.settle_sum / f64::from(self.settle_count);
        if mean > (self.target + self.band) as f64 {
            // The mean's surplus taken from the current level centres the level on the target;
            // capped at what this callback leaves, so the skip never reads past the ring.
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let offset = (mean.round() as usize)
                .saturating_sub(self.target)
                .min(after);
            self.level = self.target as f64;
            (
                Plan {
                    skip: offset,
                    ..read
                },
                Event::Aligned,
            )
        } else {
            self.level = mean;
            (read, Event::None)
        }
    }
}

fn silence(needed: usize) -> Plan {
    Plan {
        silence: needed,
        ..Plan::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn check_invariants(plan: Plan, available: usize, needed: usize) {
        assert_eq!(plan.read + plan.silence, needed, "{plan:?}");
        assert!(plan.skip + plan.read <= available, "{plan:?}");
    }

    #[test]
    fn primes_before_playing() {
        let mut control = DriftControl::new(100, 25);
        assert_eq!(control.plan(50, 10), (silence(10), Event::Priming));
        assert_eq!(control.plan(109, 10).1, Event::Priming);
        assert_eq!(control.plan(110, 10).1, Event::None);
    }

    #[test]
    fn underrun_reads_what_is_there_and_primes_again() {
        let mut control = DriftControl::new(100, 25);
        control.plan(200, 10);
        let (plan, event) = control.plan(4, 10);
        assert_eq!(event, Event::Underrun);
        assert_eq!((plan.read, plan.silence), (4, 6));
        assert_eq!(control.plan(50, 10).1, Event::Priming);
        assert_eq!(control.steady_error(), None);
    }

    #[test]
    fn reset_primes_and_settles_again() {
        let mut control = DriftControl::new(100, 25).settling(40);
        control.plan(110, 10);
        control.reset();
        assert_eq!(control.plan(50, 10).1, Event::Priming);
        // Settling again: far above the band, but within the hard limit, nothing is fixed.
        assert_eq!(control.plan(200, 10).1, Event::None);
    }

    #[test]
    fn reads_plainly_and_reports_the_level_error() {
        let mut control = DriftControl::new(100, 25);
        assert_eq!(control.steady_error(), None);
        for (available, error) in [(110, 0.0), (85, -25.0), (160, 50.0)] {
            let (plan, event) = control.plan(available, 10);
            assert_eq!(event, Event::None, "available {available}");
            assert_eq!(plan.read, 10);
            assert_eq!(control.steady_error(), Some(error));
        }
    }

    #[test]
    fn far_above_skips_back_to_the_target() {
        let mut control = DriftControl::new(100, 25);
        let (plan, event) = control.plan(1000, 10);
        assert_eq!(event, Event::Resync);
        assert_eq!(1000 - plan.skip - plan.read, 100);
        assert_eq!(control.steady_error(), Some(0.0));
    }

    #[test]
    fn a_lasting_surplus_is_skipped_once_but_a_sawtooth_is_not() {
        // Below the hard limit (300) but 80 frames above the target for good: the threshold
        // is 75, so the average crosses it and one skip takes the surplus back.
        let mut control = DriftControl::new(100, 25).smoothed(0.25);
        let mut available = 110;
        let mut events = Vec::new();
        for i in 0..60 {
            if i == 10 {
                available += 80;
            }
            let (plan, event) = control.plan(available, 10);
            if event != Event::None {
                events.push((event, plan.skip));
            }
            available = available - plan.skip - plan.read + 10;
        }
        assert_eq!(events.len(), 1, "{events:?}");
        assert_eq!(events[0].0, Event::Resync);
        // The average lags, so the skip takes what it shows; the rest is within the band and
        // left to the resampler.
        assert!(available - 110 <= 25, "{available}");

        // A sawtooth of ±75 around the target leaves the average alone.
        let mut control = DriftControl::new(150, 25).smoothed(1.0 / 64.0);
        let mut available = 350;
        for i in 0..3000 {
            if i % 3 == 0 && i > 0 {
                available += 300;
            }
            let (plan, event) = control.plan(available, 100);
            assert_eq!(event, Event::None, "callback {i}");
            available -= plan.skip + plan.read;
        }
    }

    #[test]
    fn a_resync_settles_again_and_skips_the_rest_of_a_burst() {
        let mut control = DriftControl::new(100, 25).settling(100);
        let mut available = 110;
        for _ in 0..20 {
            let (plan, _) = control.plan(available, 10);
            available = available - plan.skip - plan.read + 10;
        }
        assert_eq!(control.steady_error(), Some(0.0));
        // A burst: far too much at once, and 200 frames more right after the resync.
        let (plan, event) = control.plan(available + 500, 10);
        assert_eq!(event, Event::Resync);
        available = available + 500 - plan.skip - plan.read + 10 + 200;
        assert_eq!(control.steady_error(), None, "settling again");
        let mut skipped = 0;
        for _ in 0..12 {
            let (plan, event) = control.plan(available, 10);
            if event == Event::Aligned {
                skipped += plan.skip;
            }
            available = available - plan.skip - plan.read + 10;
        }
        assert_eq!(skipped, 200);
        assert_eq!(control.steady_error(), Some(0.0));
    }

    #[test]
    fn smoothing_flattens_a_sawtooth_but_follows_a_trend() {
        // Blocks of 300 frames arrive while the output takes 100 per callback: the level
        // after a read swings between 50 and 250 around the target of 150.
        let mut control = DriftControl::new(150, 25).smoothed(1.0 / 64.0);
        let mut available = 350;
        let mut worst: f64 = 0.0;
        for i in 0..3000 {
            if i % 3 == 0 && i > 0 {
                available += 300;
            }
            let (plan, event) = control.plan(available, 100);
            assert_ne!(event, Event::Underrun, "callback {i}");
            available -= plan.skip + plan.read;
            if i > 500 {
                worst = worst.max(control.steady_error().unwrap().abs());
            }
        }
        assert!(worst < 25.0, "the average swings by {worst} frames");

        // A level that stays 40 frames higher (below the skip threshold) shows up in the
        // average.
        for i in 0..3000 {
            available += if i % 3 == 0 { 300 } else { 0 } + usize::from(i == 0) * 40;
            let (plan, _) = control.plan(available, 100);
            available -= plan.skip + plan.read;
        }
        let error = control.steady_error().unwrap();
        assert!((error - 40.0).abs() < 25.0, "error {error}");
    }

    #[test]
    fn settling_skips_a_start_up_offset_once() {
        // The level sits 150 frames above the target (below the hard limit) from the start.
        let mut control = DriftControl::new(100, 25)
            .smoothed(1.0 / 64.0)
            .settling(200);
        let mut corrections = Vec::new();
        let mut available = 260;
        for _ in 0..40 {
            let (plan, event) = control.plan(available, 10);
            check_invariants(plan, available, 10);
            if event != Event::None {
                corrections.push((event, plan.skip));
            }
            available = available - plan.skip - plan.read + 10;
        }
        assert_eq!(corrections, [(Event::Aligned, 150)]);
        assert_eq!(available, 110);
    }

    #[test]
    fn settling_reports_no_error_until_it_ends() {
        let mut control = DriftControl::new(100, 25).settling(50);
        for _ in 0..4 {
            assert_eq!(control.plan(120, 10).1, Event::None);
            assert_eq!(control.steady_error(), None);
        }
        // The fifth callback ends the phase within the band: the mean becomes the level.
        assert_eq!(control.plan(120, 10).1, Event::None);
        assert_eq!(control.steady_error(), Some(10.0));
        assert_eq!(control.plan(3, 10).1, Event::Underrun);
        assert_eq!(control.plan(110, 10).1, Event::None);
        // Settling again: far above the band, but nothing is fixed until it ends.
        for _ in 0..3 {
            assert_eq!(control.plan(260, 10).1, Event::None);
        }
        let read = Plan {
            skip: 150,
            read: 10,
            ..Plan::default()
        };
        assert_eq!(control.plan(260, 10), (read, Event::Aligned));
    }

    #[test]
    fn every_plan_keeps_its_invariants() {
        for target in [0, 1, 100] {
            for band in [0, 3, 25] {
                for needed in [0, 1, 7, 480] {
                    let mut control = DriftControl::new(target, band).smoothed(0.1).settling(100);
                    for available in (0..1500).step_by(7) {
                        let (plan, _) = control.plan(available, needed);
                        check_invariants(plan, available, needed);
                    }
                }
            }
        }
    }
}
