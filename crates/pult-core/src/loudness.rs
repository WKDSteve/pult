//! Loudness of a bus after ITU-R BS.1770 / EBU R128: momentary, short-term and integrated
//! loudness in LUFS, the loudness range in LU, and the true peak (four times oversampled) in
//! dBTP, as a broadcast console (Lawo mc²) shows it; what a streaming platform normalises to.
//!
//! The measurement is too costly for an audio callback (filters, blocks, oversampling), so the
//! callback only hands its output to a [`LoudnessTap`]: one copy into a lock-free ring, nothing
//! else. A [`LoudnessMeter`] on its own thread empties the ring, measures with `ebur128` and
//! puts the numbers into [`LoudnessStats`] for the desk. A full ring drops what does not fit and
//! counts it; the numbers then lag, the sound does not.

use std::sync::Arc;

use ebur128::{EbuR128, Mode};
use rtrb::{Consumer, Producer, RingBuffer};

use crate::level::{AtomicF32, Counter};

/// The numbers of one bus, written by its meter, read by the desk. `None` until there is one
/// (momentary loudness needs 400 ms, short-term 3 s).
#[derive(Debug)]
pub struct LoudnessStats {
    momentary: AtomicF32,
    short_term: AtomicF32,
    integrated: AtomicF32,
    range: AtomicF32,
    true_peak: AtomicF32,
    /// Samples the tap could not hand over because the ring was full.
    pub dropped: Counter,
}

impl Default for LoudnessStats {
    fn default() -> Self {
        Self {
            momentary: AtomicF32::new(f32::NAN),
            short_term: AtomicF32::new(f32::NAN),
            integrated: AtomicF32::new(f32::NAN),
            range: AtomicF32::new(f32::NAN),
            true_peak: AtomicF32::new(f32::NAN),
            dropped: Counter::default(),
        }
    }
}

fn known(value: f32) -> Option<f32> {
    value.is_finite().then_some(value)
}

impl LoudnessStats {
    /// Momentary loudness (400 ms) in LUFS.
    #[must_use]
    pub fn momentary(&self) -> Option<f32> {
        known(self.momentary.load())
    }

    /// Short-term loudness (3 s) in LUFS.
    #[must_use]
    pub fn short_term(&self) -> Option<f32> {
        known(self.short_term.load())
    }

    /// Integrated loudness since the start, gated as BS.1770 says, in LUFS.
    #[must_use]
    pub fn integrated(&self) -> Option<f32> {
        known(self.integrated.load())
    }

    /// Loudness range in LU.
    #[must_use]
    pub fn range(&self) -> Option<f32> {
        known(self.range.load())
    }

    /// The highest true peak since the start, over all channels, in dBTP.
    #[must_use]
    pub fn true_peak(&self) -> Option<f32> {
        known(self.true_peak.load())
    }
}

/// The callback's end: hands the output of a bus to its meter.
#[derive(Debug)]
pub struct LoudnessTap {
    producer: Producer<f32>,
    channels: usize,
    stats: Arc<LoudnessStats>,
}

impl LoudnessTap {
    /// Hands over `out` (interleaved, `channels` per frame). Does not allocate, lock or wait; an
    /// output with another channel count than the meter was made for is left out.
    pub fn feed(&mut self, out: &[f32], channels: usize) {
        if channels != self.channels {
            return;
        }
        let fit = self.producer.slots() / channels * channels;
        let samples = (out.len() / channels * channels).min(fit);
        if samples < out.len() {
            self.stats.dropped.add((out.len() - samples) as u64);
        }
        if let Ok(chunk) = self.producer.write_chunk_uninit(samples) {
            chunk.fill_from_iter(out[..samples].iter().copied());
        }
    }
}

/// The measuring end, on a thread of its own.
pub struct LoudnessMeter {
    consumer: Consumer<f32>,
    ebur: EbuR128,
    channels: usize,
    stats: Arc<LoudnessStats>,
    block: Vec<f32>,
}

impl std::fmt::Debug for LoudnessMeter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LoudnessMeter")
            .field("channels", &self.channels)
            .finish_non_exhaustive()
    }
}

/// A tap and its meter for an output of `channels` at `rate`, with a ring for `seconds` of sound
/// (what the meter may fall behind before samples are dropped).
///
/// # Errors
///
/// When `ebur128` takes neither the channels nor the rate (0 of either, or more than it knows).
pub fn loudness(
    channels: usize,
    rate: u32,
    seconds: f32,
) -> Result<(LoudnessTap, LoudnessMeter, Arc<LoudnessStats>), String> {
    let count = u32::try_from(channels).map_err(|_| "too many channels".to_owned())?;
    let ebur = EbuR128::new(
        count,
        rate,
        Mode::M | Mode::S | Mode::I | Mode::LRA | Mode::TRUE_PEAK,
    )
    .map_err(|e| format!("loudness meter: {e:?}"))?;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let capacity = ((rate as f32 * seconds.max(0.1)) as usize).max(1024) * channels;
    let (producer, consumer) = RingBuffer::new(capacity);
    let stats = Arc::new(LoudnessStats::default());
    Ok((
        LoudnessTap {
            producer,
            channels,
            stats: Arc::clone(&stats),
        },
        LoudnessMeter {
            consumer,
            ebur,
            channels,
            stats: Arc::clone(&stats),
            block: Vec::with_capacity(capacity),
        },
        stats,
    ))
}

#[allow(clippy::cast_possible_truncation)]
fn as_f32(value: Result<f64, ebur128::Error>) -> f32 {
    value.map_or(f32::NAN, |v| v as f32)
}

impl LoudnessMeter {
    /// Measures what the tap handed over since the last call and publishes the numbers. Off the
    /// audio threads.
    pub fn drain(&mut self) {
        let whole = self.consumer.slots() / self.channels * self.channels;
        if whole == 0 {
            return;
        }
        self.block.clear();
        if let Ok(chunk) = self.consumer.read_chunk(whole) {
            let (first, second) = chunk.as_slices();
            self.block.extend_from_slice(first);
            self.block.extend_from_slice(second);
            chunk.commit_all();
        }
        if self.ebur.add_frames_f32(&self.block).is_err() {
            return;
        }
        let stats = &self.stats;
        stats
            .momentary
            .store(as_f32(self.ebur.loudness_momentary()));
        stats
            .short_term
            .store(as_f32(self.ebur.loudness_shortterm()));
        stats.integrated.store(as_f32(self.ebur.loudness_global()));
        stats.range.store(as_f32(self.ebur.loudness_range()));
        let mut peak = 0.0_f64;
        for channel in 0..self.channels {
            if let Ok(value) = self.ebur.true_peak(u32::try_from(channel).unwrap_or(0)) {
                peak = peak.max(value);
            }
        }
        #[allow(clippy::cast_possible_truncation)]
        let db = if peak > 0.0 {
            (20.0 * peak.log10()) as f32
        } else {
            f32::NAN
        };
        stats.true_peak.store(db);
    }

    /// Starts the integrated loudness, the range and the true peak afresh.
    pub fn reset(&mut self) {
        self.ebur.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Feeds `seconds` of a 1 kHz sine at `dbfs` (peak) on both channels through tap and meter,
    /// in callbacks of 480 frames, the meter draining every ten.
    fn sine(dbfs: f32, seconds: usize) -> Arc<LoudnessStats> {
        let (mut tap, mut meter, stats) = loudness(2, 48_000, 2.0).unwrap();
        let amplitude = 10_f32.powf(dbfs / 20.0);
        let step = std::f32::consts::TAU * 1000.0 / 48_000.0;
        let mut out = vec![0.0; 480 * 2];
        let mut n = 0_u32;
        for callback in 0..seconds * 100 {
            for frame in out.as_chunks_mut::<2>().0 {
                #[allow(clippy::cast_precision_loss)]
                let sample = amplitude * (step * n as f32).sin();
                frame.fill(sample);
                n = (n + 1) % 48_000;
            }
            tap.feed(&out, 2);
            if callback % 10 == 9 {
                meter.drain();
            }
        }
        meter.drain();
        stats
    }

    #[test]
    fn a_stereo_sine_at_minus_23_dbfs_is_minus_23_lufs() {
        // EBU Tech 3341, case 1: stereo sine 1 kHz at -23 dBFS, expected -23.0 LUFS +-0.1.
        let stats = sine(-23.0, 20);
        for (what, value) in [
            ("momentary", stats.momentary()),
            ("short-term", stats.short_term()),
            ("integrated", stats.integrated()),
        ] {
            let lufs = value.expect(what);
            assert!((lufs + 23.0).abs() < 0.1, "{what}: {lufs} LUFS");
        }
        // A steady tone has no range; its true peak is its peak.
        assert!(stats.range().expect("range") < 0.5);
        let peak = stats.true_peak().expect("true peak");
        assert!((peak + 23.0).abs() < 0.2, "true peak {peak} dBTP");
        assert_eq!(stats.dropped.get(), 0);
    }

    #[test]
    fn nothing_measured_is_no_number() {
        let (_tap, _meter, stats) = loudness(2, 48_000, 1.0).unwrap();
        assert_eq!(stats.integrated(), None);
        assert_eq!(stats.true_peak(), None);
    }

    #[test]
    fn a_meter_that_falls_behind_drops_and_counts_and_another_layout_is_left_out() {
        let (mut tap, _meter, stats) = loudness(2, 48_000, 0.1).unwrap();
        let out = vec![0.1; 48_000 * 2];
        tap.feed(&out, 2);
        assert!(stats.dropped.get() > 0);
        let before = stats.dropped.get();
        tap.feed(&[0.1; 6], 3);
        assert_eq!(
            stats.dropped.get(),
            before,
            "a 3-channel output is not this meter's"
        );
    }

    #[test]
    fn the_tap_does_not_allocate() {
        let (mut tap, _meter, _stats) = loudness(2, 48_000, 1.0).unwrap();
        let out = vec![0.1; 960];
        assert_no_alloc::assert_no_alloc(|| {
            for _ in 0..20 {
                tap.feed(&out, 2);
            }
        });
    }
}
