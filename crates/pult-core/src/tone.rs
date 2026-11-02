//! A sine test tone, to check a chain of devices without any program playing.

use std::f32::consts::TAU;

/// A sine oscillator with a fixed frequency and gain.
#[derive(Debug, Clone)]
pub struct Sine {
    phase: f32,
    step: f32,
    gain: f32,
}

impl Sine {
    /// A tone of `frequency` Hz at `sample_rate` Hz, scaled by the linear `gain`, which is
    /// held to full scale (0 to 1).
    #[must_use]
    pub fn new(frequency: f32, sample_rate: u32, gain: f32) -> Self {
        Self {
            phase: 0.0,
            step: TAU * frequency / sample_rate.max(1) as f32,
            gain: if gain.is_nan() {
                0.0
            } else {
                gain.clamp(0.0, 1.0)
            },
        }
    }

    /// Overwrites `out` (interleaved, `channels` per frame) with the next frames, the same
    /// sample on every channel.
    pub fn fill(&mut self, out: &mut [f32], channels: usize) {
        for frame in out.chunks_exact_mut(channels.max(1)) {
            frame.fill(self.phase.sin() * self.gain);
            // Wrapping keeps the phase small, so precision does not decay over hours.
            self.phase = (self.phase + self.step) % TAU;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quarter_rate_tone_walks_the_quadrants_on_both_channels() {
        let mut sine = Sine::new(12_000.0, 48_000, 0.5);
        let mut out = [9.0; 8];
        sine.fill(&mut out, 2);
        let expected = [0.0, 0.0, 0.5, 0.5, 0.0, 0.0, -0.5, -0.5];
        assert!(out.iter().zip(expected).all(|(o, e)| (o - e).abs() < 1e-5));
    }

    #[test]
    fn gain_never_exceeds_full_scale() {
        let mut sine = Sine::new(12_000.0, 48_000, 10.0);
        let mut out = [0.0; 2];
        sine.fill(&mut out, 1);
        assert!((out[1] - 1.0).abs() < 1e-5, "{out:?}");
    }

    #[test]
    fn phase_stays_bounded_and_peak_matches_gain() {
        let mut sine = Sine::new(440.0, 48_000, 0.25);
        let mut out = vec![0.0; 48_000];
        for _ in 0..100 {
            sine.fill(&mut out, 1);
        }
        assert!(sine.phase < TAU);
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 0.25).abs() < 1e-3);
    }
}
