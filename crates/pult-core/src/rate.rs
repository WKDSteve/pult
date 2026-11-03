//! Steers the ratio of an input's resampler so that its ring stays at the target level.
//!
//! Every device runs on its own clock: a virtual cable and a USB interface both claim
//! 48 kHz but differ by a few samples per second. The output of a bus sets the pace; each
//! input ring in front of it runs through a resampler that takes slightly more or fewer
//! input frames per output frame. This control picks that factor from the ring's (smoothed)
//! level: a proportional term pulls the level back to the target, an integral term learns
//! the clock difference itself, so the level ends up on the target without a lasting offset.
//!
//! The control works in seconds, not frames, so it behaves the same at every sample rate.
//! With the proportional time `tau_p` and the integral time `tau_i = 4 * tau_p` the loop is
//! critically damped: an offset decays within a few `tau_p`. On the way it dips below the
//! target by at most about 14 % of the offset, since the integral has to give back what it
//! collected; the settling phase of [`crate::drift`] keeps such offsets small anyway.

/// The PI control of one input ring.
#[derive(Debug, Clone)]
pub struct RateControl {
    /// Seconds in which the proportional term alone would remove a level error.
    tau_p: f64,
    /// Seconds over which the integral term builds up.
    tau_i: f64,
    /// Largest correction in either direction (0.005 is 0.5 %).
    limit: f64,
    /// Integral of the level error over time, in seconds squared.
    integral: f64,
    /// The latest correction.
    correction: f64,
}

impl RateControl {
    /// A control with proportional time `tau_p` and integral time `tau_i` (both in seconds,
    /// above zero) whose correction stays within `±limit`.
    #[must_use]
    pub fn new(tau_p: f64, tau_i: f64, limit: f64) -> Self {
        Self {
            tau_p: tau_p.max(f64::MIN_POSITIVE),
            tau_i: tau_i.max(f64::MIN_POSITIVE),
            limit: limit.abs(),
            integral: 0.0,
            correction: 0.0,
        }
    }

    /// Forgets the learned clock difference, for an input whose device changed.
    pub fn reset(&mut self) {
        self.integral = 0.0;
        self.correction = 0.0;
    }

    /// The latest correction: how many more input frames per output frame the resampler
    /// takes than the nominal rates say (positive drains the ring, negative fills it).
    #[must_use]
    pub fn correction(&self) -> f64 {
        self.correction
    }

    /// Takes the level error (level minus target, in seconds of input) after `dt` seconds of
    /// output and returns the new correction.
    pub fn update(&mut self, error: f64, dt: f64) -> f64 {
        if !error.is_finite() || !dt.is_finite() {
            return self.correction;
        }
        // The integral alone may never ask for more than the limit, or it would wind up
        // while the correction is saturated and overshoot afterwards.
        let bound = self.limit * self.tau_p * self.tau_i;
        self.integral = (self.integral + error * dt).clamp(-bound, bound);
        self.correction =
            ((error + self.integral / self.tau_i) / self.tau_p).clamp(-self.limit, self.limit);
        self.correction
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A ring between two clocks: the input delivers `1 + drift` frames per output frame,
    /// the resampler takes `1 + correction`. Returns the level error in seconds after each
    /// of `steps` callbacks of `dt` seconds at `rate` frames per second.
    fn simulate(control: &mut RateControl, start: f64, drift: f64, steps: usize) -> Vec<f64> {
        let (rate, dt) = (48_000.0, 0.01);
        let mut error = start;
        let mut errors = Vec::with_capacity(steps);
        for _ in 0..steps {
            let c = control.update(error / rate, dt);
            error += rate * dt * (drift - c);
            errors.push(error / rate);
        }
        errors
    }

    #[test]
    fn learns_a_clock_difference_and_settles_on_the_target() {
        let mut control = RateControl::new(10.0, 40.0, 0.005);
        // 100 ppm: the input runs fast; after two minutes the level is back on the target
        // and the correction equals the drift.
        let errors = simulate(&mut control, 0.0, 100e-6, 12_000);
        assert!(errors.last().unwrap().abs() < 1e-4, "{:?}", errors.last());
        assert!((control.correction() - 100e-6).abs() < 2e-6);
    }

    #[test]
    fn removes_an_offset_with_a_small_dip() {
        let mut control = RateControl::new(10.0, 40.0, 0.005);
        // 480 frames (10 ms) too many: gone after a minute, never more than 15 % below.
        let errors = simulate(&mut control, 480.0, 0.0, 30_000);
        let lowest = errors.iter().copied().fold(f64::INFINITY, f64::min);
        assert!(lowest > -0.15 * 0.01, "dips to {lowest}");
        assert!(errors[6_000].abs() < 0.2 * 0.01, "{}", errors[6_000]);
        assert!(errors.last().unwrap().abs() < 0.01 * 0.01);
    }

    #[test]
    fn stays_within_the_limit_and_does_not_wind_up() {
        let mut control = RateControl::new(10.0, 40.0, 0.005);
        // A huge error saturates the correction ...
        for _ in 0..10_000 {
            assert!(control.update(1.0, 0.01).abs() <= 0.005);
        }
        // ... but once the error is gone, the integral alone asks for at most the limit.
        assert!(control.update(0.0, 0.01) <= 0.005 + 1e-12);
    }

    #[test]
    fn ignores_invalid_input_and_resets() {
        let mut control = RateControl::new(10.0, 40.0, 0.005);
        let c = control.update(0.01, 0.01);
        assert_eq!(control.update(f64::NAN, 0.01), c);
        assert_eq!(control.update(0.01, f64::INFINITY), c);
        control.reset();
        assert_eq!(control.correction(), 0.0);
        assert_eq!(control.update(0.0, 0.01), 0.0);
    }
}
