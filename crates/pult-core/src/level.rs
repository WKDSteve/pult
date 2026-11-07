//! Levels and counters shared between the audio threads and the control thread.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// An `f32` that one thread writes and another reads, without a lock.
#[derive(Debug, Default)]
pub struct AtomicF32(AtomicU32);

impl AtomicF32 {
    /// A new value.
    #[must_use]
    pub fn new(value: f32) -> Self {
        Self(AtomicU32::new(value.to_bits()))
    }

    /// The current value.
    pub fn load(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    /// Replaces the value.
    pub fn store(&self, value: f32) {
        self.0.store(value.to_bits(), Ordering::Relaxed);
    }
}

/// A monotonic event counter, bumped by an audio thread and read by the control thread.
#[derive(Debug, Default)]
pub struct Counter(AtomicU64);

impl Counter {
    /// Adds `n` events.
    pub fn add(&self, n: u64) {
        self.0.fetch_add(n, Ordering::Relaxed);
    }

    /// The number of events so far.
    pub fn get(&self) -> u64 {
        self.0.load(Ordering::Relaxed)
    }
}

/// Decibels to a linear factor (0 dB is 1.0, -6 dB is about 0.5).
#[must_use]
pub fn db_to_linear(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// A linear factor to decibels; silence is negative infinity.
#[must_use]
pub fn linear_to_db(linear: f32) -> f32 {
    if linear > 0.0 {
        20.0 * linear.log10()
    } else {
        f32::NEG_INFINITY
    }
}

/// `value` inside `range`, or `fallback` for a NaN.
pub(crate) fn within(value: f32, range: (f32, f32), fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(range.0, range.1)
    }
}

/// The highest absolute sample since the last [`take`](Self::take).
///
/// The audio thread records, the control thread takes; NaN samples are ignored.
#[derive(Debug, Default)]
pub struct PeakMeter(AtomicU32);

impl PeakMeter {
    /// Records the peak of `samples` scaled by `gain`.
    pub fn record_scaled(&self, samples: &[f32], gain: f32) {
        let peak = samples.iter().fold(0.0f32, |max, s| max.max(s.abs())) * gain.abs();
        if peak > 0.0 {
            // Bit patterns of non-negative floats order like their values, so the integer
            // maximum is the float maximum.
            self.0.fetch_max(peak.to_bits(), Ordering::Relaxed);
        }
    }

    /// Records `value`, which is not negative, as it is.
    pub fn record(&self, value: f32) {
        if value > 0.0 {
            self.0.fetch_max(value.to_bits(), Ordering::Relaxed);
        }
    }

    /// The peak so far, without starting a new measurement.
    pub fn peek(&self) -> f32 {
        f32::from_bits(self.0.load(Ordering::Relaxed))
    }

    /// The peak since the last call, and starts a new measurement.
    pub fn take(&self) -> f32 {
        f32::from_bits(self.0.swap(0, Ordering::Relaxed))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decibels_round_trip() {
        assert!((db_to_linear(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_linear(-20.0) - 0.1).abs() < 1e-6);
        assert!((linear_to_db(0.5) + 6.0206).abs() < 1e-3);
        assert_eq!(linear_to_db(0.0), f32::NEG_INFINITY);
    }

    #[test]
    fn peak_keeps_the_maximum_until_taken() {
        let meter = PeakMeter::default();
        meter.record_scaled(&[0.1, -0.7, 0.3], 1.0);
        meter.record_scaled(&[0.2], 1.0);
        assert!((meter.take() - 0.7).abs() < 1e-6);
        assert_eq!(meter.take(), 0.0);
    }

    #[test]
    fn peak_applies_gain_and_ignores_nan() {
        let meter = PeakMeter::default();
        meter.record_scaled(&[f32::NAN, 0.5], 0.5);
        assert!((meter.take() - 0.25).abs() < 1e-6);
    }

    #[test]
    fn atomic_f32_stores_and_loads() {
        let value = AtomicF32::new(1.5);
        value.store(-2.25);
        assert_eq!(value.load(), -2.25);
    }
}
