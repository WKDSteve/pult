//! Keeps the fill level of an input's ring buffer near a target.
//!
//! Every device runs on its own clock: a virtual cable and a USB interface both claim
//! 48 kHz but differ by a few samples per second. Left alone, the ring between them drains
//! (dropouts) or fills up (growing delay). Until the adaptive resampler exists, this control
//! drops or repeats single frames when the (optionally smoothed) level leaves a band around
//! the target, which is inaudible at the rates real clocks drift, and resynchronises hard
//! only when far off.

/// What one output callback does with one input's ring, in frames.
///
/// `read + repeat + silence` is always the frames the callback needs, and `skip + read`
/// never exceeds the frames available.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Plan {
    /// Frames to discard before reading.
    pub skip: usize,
    /// Frames to read.
    pub read: usize,
    /// Copies of the last read frame to append.
    pub repeat: usize,
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
    /// One frame dropped to lower the level.
    Dropped,
    /// One frame repeated to raise the level.
    Repeated,
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
}

impl DriftControl {
    /// A control that aims at `target` frames left after each read and corrects when the
    /// level leaves `target ± band`; it reacts to every callback's level (no smoothing).
    #[must_use]
    pub fn new(target: usize, band: usize) -> Self {
        Self {
            target,
            band,
            primed: false,
            level: 0.0,
            alpha: 1.0,
        }
    }

    /// The same control judging single-frame corrections by a moving average of the level,
    /// each callback weighing `alpha` (0 < alpha <= 1). Devices that deliver in large
    /// blocks make the level a sawtooth; its average stays put while real drift moves it.
    /// Underruns and resyncs still follow the actual level.
    #[must_use]
    pub fn smoothed(mut self, alpha: f64) -> Self {
        self.alpha = alpha.clamp(f64::MIN_POSITIVE, 1.0);
        self
    }

    /// The level beyond which the control skips back to the target at once. Block jitter
    /// can lift the level to nearly twice the target without any drift, so the limit lies
    /// above that.
    #[must_use]
    pub fn hard_limit(&self) -> usize {
        2 * self.target + 4 * self.band
    }

    /// The plan for a callback that needs `needed` frames while `available` are buffered.
    pub fn plan(&mut self, available: usize, needed: usize) -> (Plan, Event) {
        if !self.primed {
            if available < self.target + needed {
                return (silence(needed), Event::Priming);
            }
            self.primed = true;
            // Start the average at the target: right after priming the level may sit on a
            // peak of the sawtooth, and starting there would drop frames the ring needs.
            self.level = self.target as f64;
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
            let plan = Plan {
                skip: after - self.target,
                ..read
            };
            return (plan, Event::Resync);
        }
        self.level += self.alpha * (after as f64 - self.level);
        let high = (self.target + self.band) as f64;
        let low = self.target as f64 - self.band as f64;
        if self.level > high && after > 0 {
            (Plan { skip: 1, ..read }, Event::Dropped)
        } else if self.level < low && needed > 0 {
            let plan = Plan {
                read: needed - 1,
                repeat: 1,
                ..Plan::default()
            };
            (plan, Event::Repeated)
        } else {
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
        assert_eq!(plan.read + plan.repeat + plan.silence, needed, "{plan:?}");
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
    }

    #[test]
    fn inside_the_band_reads_plainly() {
        let mut control = DriftControl::new(100, 25);
        control.plan(110, 10);
        for available in [85, 110, 135] {
            let (plan, event) = control.plan(available, 10);
            assert_eq!(event, Event::None, "available {available}");
            assert_eq!(plan.read, 10);
        }
    }

    #[test]
    fn above_the_band_drops_one_frame() {
        let mut control = DriftControl::new(100, 25);
        let (plan, event) = control.plan(136, 10);
        assert_eq!(event, Event::Dropped);
        assert_eq!((plan.skip, plan.read), (1, 10));
    }

    #[test]
    fn below_the_band_repeats_one_frame() {
        let mut control = DriftControl::new(100, 25);
        control.plan(200, 10);
        let (plan, event) = control.plan(84, 10);
        assert_eq!(event, Event::Repeated);
        assert_eq!((plan.read, plan.repeat), (9, 1));
    }

    #[test]
    fn far_above_skips_back_to_the_target() {
        let mut control = DriftControl::new(100, 25);
        let (plan, event) = control.plan(1000, 10);
        assert_eq!(event, Event::Resync);
        assert_eq!(1000 - plan.skip - plan.read, 100);
    }

    #[test]
    fn smoothing_ignores_a_sawtooth_but_follows_a_trend() {
        // Blocks of 300 frames arrive while the output takes 100 per callback: the level
        // after a read swings between 50 and 250 around the target of 150, far wider than
        // the band.
        let mut control = DriftControl::new(150, 25).smoothed(1.0 / 64.0);
        let mut available = 350;
        let mut fixes = 0;
        for i in 0..3000 {
            if i % 3 == 0 && i > 0 {
                available += 300;
            }
            let (plan, event) = control.plan(available, 100);
            assert_ne!(event, Event::Underrun, "callback {i}");
            fixes += usize::from(matches!(event, Event::Dropped | Event::Repeated));
            available -= plan.skip + plan.read;
        }
        assert!(fixes < 10, "{fixes} corrections for pure jitter");

        // One extra frame per callback is real drift: the control has to drop.
        let mut drops = 0;
        for i in 0..3000 {
            available += if i % 3 == 0 { 301 } else { 1 };
            let (plan, event) = control.plan(available, 100);
            drops += usize::from(event == Event::Dropped);
            available -= plan.skip + plan.read;
        }
        assert!(drops > 2500, "only {drops} drops against the drift");
    }

    #[test]
    fn every_plan_keeps_its_invariants() {
        for target in [0, 1, 100] {
            for band in [0, 3, 25] {
                for needed in [0, 1, 7, 480] {
                    let mut control = DriftControl::new(target, band).smoothed(0.1);
                    for available in (0..1500).step_by(7) {
                        let (plan, _) = control.plan(available, needed);
                        check_invariants(plan, available, needed);
                    }
                }
            }
        }
    }
}
