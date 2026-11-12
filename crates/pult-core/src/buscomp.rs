//! The compressor of a bus, as the bus compressor of an SSL console: on the sum, after its
//! equalizer and before its fader and limiter, with a high-pass filter in its sidechain (the
//! bass no longer pumps the mix) and a mix of the dry and the compressed sound (parallel
//! compression: the punch of a hard setting under the life of the dry sum).
//!
//! It works in pieces of [`PIECE_FRAMES`] frames through buffers made in [`BusCompressor::new`]
//! for up to [`MAX_CHANNELS`] channels; nothing is allocated in a callback. An output with more
//! channels passes untouched.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::dynamics::{Compressor, CompressorControls};
use crate::level::{AtomicF32, PeakMeter};
use crate::strip::Biquad;

/// Frames worked on at once.
pub const PIECE_FRAMES: usize = 1024;
/// The most channels the buffers are made for.
pub const MAX_CHANNELS: usize = 8;
/// Range of the sidechain high-pass in Hz; at its lower end it is as good as off.
pub const HPF_HZ: (f32, f32) = (20.0, 500.0);

/// The settings of a bus compressor, set by the control thread.
#[derive(Debug)]
pub struct BusCompControls {
    /// The compressor itself, switched and set by its one knob ([`CompressorControls::set_simple`]).
    pub comp: CompressorControls,
    /// How much of the compressed sound is in the output, 0 (dry) to 1 (all compressed).
    pub mix: AtomicF32,
    /// Corner of the sidechain high-pass in Hz.
    pub hpf_hz: AtomicF32,
    /// The most it took away since the desk last read it, in dB.
    pub reduction: PeakMeter,
    /// Changes of the settings, so the filter is computed again only when the corner moved.
    generation: AtomicU32,
}

impl Default for BusCompControls {
    fn default() -> Self {
        Self {
            comp: CompressorControls::default(),
            mix: AtomicF32::new(1.0),
            hpf_hz: AtomicF32::new(HPF_HZ.0),
            reduction: PeakMeter::default(),
            generation: AtomicU32::new(0),
        }
    }
}

impl BusCompControls {
    /// Sets the mix, 0 to 1 (outside clamped, no number: all compressed).
    pub fn set_mix(&self, mix: f32) {
        self.mix.store(if mix.is_finite() {
            mix.clamp(0.0, 1.0)
        } else {
            1.0
        });
    }

    /// Sets the corner of the sidechain high-pass, kept to [`HPF_HZ`].
    pub fn set_hpf(&self, hz: f32) {
        let hz = if hz.is_finite() {
            hz.clamp(HPF_HZ.0, HPF_HZ.1)
        } else {
            HPF_HZ.0
        };
        self.hpf_hz.store(hz);
        self.generation.fetch_add(1, Ordering::Release);
    }
}

/// The compressor of one bus, in its output callback.
#[derive(Debug)]
pub struct BusCompressor {
    rate: f32,
    comp: Compressor,
    filters: [Biquad; MAX_CHANNELS],
    seen: Option<u32>,
    dry: Vec<f32>,
    key: Vec<f32>,
}

impl BusCompressor {
    /// A bus compressor at `rate` frames per second.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate_f = rate.max(1) as f32;
        Self {
            rate: rate_f,
            comp: Compressor::new(rate),
            filters: [Biquad::default(); MAX_CHANNELS],
            seen: None,
            dry: vec![0.0; PIECE_FRAMES * MAX_CHANNELS],
            key: vec![0.0; PIECE_FRAMES * MAX_CHANNELS],
        }
    }

    /// Compresses `out` (interleaved, `channels` per frame) in place as `controls` say.
    pub fn process(&mut self, controls: &BusCompControls, out: &mut [f32], channels: usize) {
        if !controls.comp.is_on() || channels == 0 || channels > MAX_CHANNELS {
            return;
        }
        let generation = controls.generation.load(Ordering::Acquire);
        if self.seen != Some(generation) {
            let hz = controls.hpf_hz.load().clamp(HPF_HZ.0, HPF_HZ.1);
            for filter in &mut self.filters {
                filter.high_pass(hz, self.rate);
            }
            self.seen = Some(generation);
        }
        let mix = controls.mix.load().clamp(0.0, 1.0);
        let whole = out.len() / channels * channels;
        for piece in out[..whole].chunks_mut(PIECE_FRAMES * channels) {
            let n = piece.len();
            self.dry[..n].copy_from_slice(piece);
            for (index, sample) in piece.iter().enumerate() {
                self.key[index] = self.filters[index % channels].next(*sample);
            }
            self.comp
                .process_keyed(&controls.comp, piece, &self.key[..n], channels);
            controls.reduction.record(self.comp.reduction_db());
            if mix < 1.0 {
                for (wet, dry) in piece.iter_mut().zip(&self.dry[..n]) {
                    *wet = *dry + (*wet - *dry) * mix;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dynamics::CompressorParams;

    fn sine(hz: f32, amplitude: f32, frames: usize) -> Vec<f32> {
        let step = std::f32::consts::TAU * hz / 48_000.0;
        #[allow(clippy::cast_precision_loss)]
        (0..frames)
            .flat_map(|n| {
                let s = amplitude * (step * n as f32).sin();
                [s, s]
            })
            .collect()
    }

    fn peak(out: &[f32]) -> f32 {
        out.iter().fold(0.0, |m: f32, s| m.max(s.abs()))
    }

    fn controls() -> BusCompControls {
        let c = BusCompControls::default();
        c.comp.set(CompressorParams {
            threshold_db: -20.0,
            ratio: 4.0,
            makeup_db: 0.0,
            ..CompressorParams::default()
        });
        c.comp.switch(true);
        c
    }

    /// Runs `signal` through in callbacks of 480 frames; the last second.
    fn run(c: &BusCompControls, signal: &[f32]) -> Vec<f32> {
        let mut comp = BusCompressor::new(48_000);
        let mut out = Vec::new();
        for callback in signal.chunks(480 * 2) {
            let mut buf = callback.to_vec();
            comp.process(c, &mut buf, 2);
            out.extend(buf);
        }
        out[out.len() - 48_000 * 2..].to_vec()
    }

    #[test]
    fn off_it_passes_untouched() {
        let c = BusCompControls::default();
        let input = sine(1000.0, 0.5, 48_000);
        assert_eq!(run(&c, &input), input[input.len() - 96_000..]);
    }

    #[test]
    fn it_compresses_and_the_mix_brings_the_dry_sound_back() {
        let c = controls();
        let input = sine(1000.0, 0.5, 96_000); // -6 dB, 14 dB over the threshold
        let wet = peak(&run(&c, &input));
        assert!(wet < 0.25, "compressed: {wet}");
        assert!(c.reduction.take() > 6.0);
        c.set_mix(0.5);
        let half = peak(&run(&c, &input));
        assert!(
            (half - f32::midpoint(0.5, wet)).abs() < 0.02,
            "half dry: {half}, wet {wet}"
        );
        c.set_mix(0.0);
        assert!((peak(&run(&c, &input)) - 0.5).abs() < 1e-3, "all dry");
    }

    #[test]
    fn the_sidechain_high_pass_keeps_the_bass_from_pumping() {
        let c = controls();
        let bass = sine(40.0, 0.5, 96_000);
        let full = peak(&run(&c, &bass));
        c.set_hpf(300.0);
        let filtered = peak(&run(&c, &bass));
        assert!(
            filtered > full * 1.5,
            "the bass compresses far less: {filtered} vs {full}"
        );
        // Clamped to its range.
        c.set_hpf(5000.0);
        assert_eq!(c.hpf_hz.load(), HPF_HZ.1);
    }

    #[test]
    fn it_does_not_allocate_and_leaves_too_many_channels_alone() {
        let c = controls();
        let mut comp = BusCompressor::new(48_000);
        let mut out = vec![0.4; 3000 * 2];
        assert_no_alloc::assert_no_alloc(|| {
            comp.process(&c, &mut out, 2);
            c.set_mix(0.3);
            comp.process(&c, &mut out, 2);
        });
        let mut wide = vec![0.9; 9 * 100];
        comp.process(&c, &mut wide, 9);
        assert!(wide.iter().all(|&s| s.to_bits() == 0.9_f32.to_bits()));
    }
}
