//! Inputs, their rings and the mixer that sums them into one output.
//!
//! Each input device gets a [`Capture`] (its callback pushes into a ring) and a
//! [`MixerInput`] (the output callback reads from that ring through a resampler that
//! converts the input's rate to the output's and follows the drift between their clocks). A [`Mixer`] owns the inputs of
//! one bus and fills that bus's output buffer. An input that feeds several buses has one
//! ring per bus, each with its own drift control (every output runs on its own clock), and
//! shares one set of [`InputControls`] across them. Control and statistics go through an
//! [`InputHandle`] and a [`BusHandle`] the control thread keeps.
//!
//! While a bus's output device is gone, [`Mixer::suspend`] closes its rings: the captures
//! discard into them instead of counting a full ring as lost frames. [`Mixer::resume`]
//! empties and reopens them for the device's return.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rtrb::{Consumer, Producer, RingBuffer};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, Resizable, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
// `Adjustable` brings `set_resample_ratio_relative` into scope.
use rubato::Adjustable as _;

use crate::drift::{DriftControl, Event};
use crate::level::{AtomicF32, Counter, PeakMeter};
use crate::rate::RateControl;
use crate::remix::remix_add;

/// Output frames one resampler call produces at most; longer callbacks take several.
const MAX_CHUNK: usize = 1024;

/// Seconds in which the rate control's proportional term would remove a level error; the
/// integral time is four times that (critically damped, see [`crate::rate`]). Slow on
/// purpose: an error up to the skip threshold (twice the band, 20 ms at 40 ms latency) asks
/// for at most about 700 ppm, about one cent of pitch; real drift is tens of ppm.
const STEER_SECONDS: f64 = 30.0;

/// The largest correction against clock drift: 0.5 %, far beyond any real clock (they differ
/// by tens of ppm) and still far below an audible pitch change.
const MAX_CORRECTION: f64 = 0.005;

/// Gain, mute and pan of one input, set by the control thread.
#[derive(Debug)]
pub struct InputControls {
    /// Linear gain factor.
    pub gain: AtomicF32,
    /// Muted inputs keep draining their ring but add nothing.
    pub muted: AtomicBool,
    /// -1 (left) to 1 (right); see [`remix_add`].
    pub pan: AtomicF32,
}

impl Default for InputControls {
    fn default() -> Self {
        Self {
            gain: AtomicF32::new(1.0),
            muted: AtomicBool::new(false),
            pan: AtomicF32::new(0.0),
        }
    }
}

/// What happened to one ring of an input, counted by the audio threads.
#[derive(Debug, Default)]
pub struct InputStats {
    /// Peak after gain, as fed into the mix.
    pub peak: PeakMeter,
    /// Frames the capture could not store because the ring was full.
    pub dropped_at_capture: Counter,
    /// Times the ring ran dry during playback.
    pub underruns: Counter,
    /// Hard skips back to the target level.
    pub resyncs: Counter,
    /// The resampler's latest correction against clock drift, in parts per million: how
    /// many more input frames it takes per output frame than the nominal rates say.
    pub correction_ppm: AtomicF32,
    /// Frames skipped once at the end of a settling phase.
    pub aligned: Counter,
}

/// Gain and mute of one bus, set by the control thread.
#[derive(Debug)]
pub struct BusControls {
    /// Linear gain factor applied to the sum.
    pub gain: AtomicF32,
    /// A muted bus plays silence; its inputs keep draining.
    pub muted: AtomicBool,
}

impl Default for BusControls {
    fn default() -> Self {
        Self {
            gain: AtomicF32::new(1.0),
            muted: AtomicBool::new(false),
        }
    }
}

/// Statistics of one bus's output.
#[derive(Debug, Default)]
pub struct BusStats {
    /// Peak of the output after the bus gain.
    pub peak: PeakMeter,
    /// Output samples beyond full scale (clipped to it) or invalid (replaced by silence).
    pub clipped: Counter,
}

/// The control thread's side of one input ring.
#[derive(Debug, Clone)]
pub struct InputHandle {
    /// Gain, mute and pan, shared by all rings of the input.
    pub controls: Arc<InputControls>,
    /// Peaks and counters of this ring.
    pub stats: Arc<InputStats>,
}

/// The control thread's side of one bus.
#[derive(Debug, Clone)]
pub struct BusHandle {
    /// Gain and mute.
    pub controls: Arc<BusControls>,
    /// Peak and clip counter.
    pub stats: Arc<BusStats>,
}

/// Layout, rates and buffering of one input; frames count at the input's rate.
#[derive(Debug, Clone, Copy)]
pub struct InputConfig {
    /// Channels per frame of the input device.
    pub channels: usize,
    /// Sample rate of the input device, in Hz.
    pub input_rate: u32,
    /// Sample rate of the bus's output device, in Hz.
    pub output_rate: u32,
    /// Ring size.
    pub capacity: usize,
    /// Frames the control keeps buffered: the added latency.
    pub target: usize,
    /// Tolerance around the target before a correction.
    pub band: usize,
    /// Weight of each callback in the level average of the drift control (1.0: none).
    pub smoothing: f64,
    /// Frames after each priming in which the drift control measures the start-up offset
    /// instead of correcting (0: none).
    pub settle: usize,
}

/// Creates the ring of one input with its own controls and both of its ends.
///
/// # Panics
///
/// When `channels` is zero.
#[must_use]
pub fn connect(config: InputConfig) -> (Capture, MixerInput, InputHandle) {
    connect_with(config, Arc::new(InputControls::default()))
}

/// Creates one more ring of an input whose `controls` other rings already share.
///
/// # Panics
///
/// When `channels` or a rate is zero.
#[must_use]
pub fn connect_with(
    config: InputConfig,
    controls: Arc<InputControls>,
) -> (Capture, MixerInput, InputHandle) {
    assert!(config.channels > 0, "an input needs at least one channel");
    assert!(
        config.input_rate > 0 && config.output_rate > 0,
        "an input needs sample rates"
    );
    let samples = config.capacity * config.channels;
    let (producer, consumer) = RingBuffer::new(samples);
    let resampler = resampler(config);
    let scratch = vec![0.0; resampler.input_frames_max() * config.channels];
    let chunk_out = vec![0.0; MAX_CHUNK * config.channels];
    let open = Arc::new(AtomicBool::new(true));
    let handle = InputHandle {
        controls,
        stats: Arc::new(InputStats::default()),
    };
    let capture = Capture {
        producer,
        channels: config.channels,
        open: Arc::clone(&open),
        stats: Arc::clone(&handle.stats),
    };
    let input = MixerInput {
        consumer,
        channels: config.channels,
        open,
        drift: DriftControl::new(config.target, config.band)
            .smoothed(config.smoothing)
            .settling(config.settle),
        rate: RateControl::new(STEER_SECONDS, 4.0 * STEER_SECONDS, MAX_CORRECTION)
            // Up to where the drift control skips a surplus; beyond it is a backlog.
            .integrating_within(2.0 * config.band as f64 / f64::from(config.input_rate)),
        resampler,
        input_rate: f64::from(config.input_rate),
        output_rate: f64::from(config.output_rate),
        scratch,
        resampled: chunk_out,
        controls: Arc::clone(&handle.controls),
        stats: Arc::clone(&handle.stats),
    };
    (capture, input, handle)
}

/// The resampler of one ring: band-limited (sinc) so that rates far apart convert cleanly,
/// with a short filter and linear interpolation between its points to stay cheap with many
/// rings, and room for [`MAX_CORRECTION`] around the nominal ratio.
fn resampler(config: InputConfig) -> Async<f32> {
    let parameters = SincInterpolationParameters {
        sinc_len: 128,
        f_cutoff: None,
        oversampling_factor: 256,
        interpolation: SincInterpolationType::Linear,
        window: WindowFunction::BlackmanHarris2,
    };
    let ratio = f64::from(config.output_rate) / f64::from(config.input_rate);
    Async::new_sinc(
        ratio,
        1.0 + 2.0 * MAX_CORRECTION,
        &parameters,
        MAX_CHUNK,
        config.channels,
        FixedAsync::Output,
    )
    .expect("valid resampler parameters: positive rates, chunk and channels")
}

/// The capture end of an input ring: its device callback pushes here.
#[derive(Debug)]
pub struct Capture {
    producer: Producer<f32>,
    channels: usize,
    /// Closed while the ring's bus has no device; then `push` discards.
    open: Arc<AtomicBool>,
    stats: Arc<InputStats>,
}

impl Capture {
    /// Channels per frame this ring was made for.
    #[must_use]
    pub fn channels(&self) -> usize {
        self.channels
    }

    /// Stores the whole frames of `data` that fit; the rest is counted as dropped. A closed
    /// ring discards everything without counting.
    pub fn push(&mut self, data: &[f32]) {
        if !self.open.load(Ordering::Acquire) {
            return;
        }
        let frames = data.len() / self.channels;
        let stored = frames.min(self.producer.slots() / self.channels);
        if stored < frames {
            self.stats.dropped_at_capture.add((frames - stored) as u64);
        }
        let samples = stored * self.channels;
        if let Ok(chunk) = self.producer.write_chunk_uninit(samples) {
            chunk.fill_from_iter(data[..samples].iter().copied());
        }
    }
}

/// The playback end of an input ring, owned by the mixer of one bus.
#[derive(Debug)]
pub struct MixerInput {
    consumer: Consumer<f32>,
    channels: usize,
    open: Arc<AtomicBool>,
    drift: DriftControl,
    rate: RateControl,
    resampler: Async<f32>,
    input_rate: f64,
    output_rate: f64,
    /// Frames read from the ring for one chunk, at the input rate.
    scratch: Vec<f32>,
    /// One chunk resampled to the output rate.
    resampled: Vec<f32>,
    controls: Arc<InputControls>,
    stats: Arc<InputStats>,
}

impl MixerInput {
    /// Empties the ring and lets the drift control prime again, with a fresh resampler and
    /// rate control: the device on the other side may run on another clock now.
    fn restart(&mut self) {
        let stale = self.consumer.slots();
        if let Ok(chunk) = self.consumer.read_chunk(stale) {
            chunk.commit_all();
        }
        self.drift.reset();
        self.resampler.reset();
        self.rate.reset();
        self.stats.correction_ppm.store(0.0);
    }

    /// Resamples `frames` output frames from the ring, chunk by chunk, and adds them into
    /// `out`.
    fn mix_into(&mut self, out: &mut [f32], out_channels: usize, frames: usize) {
        let gain = if self.controls.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.controls.gain.load()
        };
        let position = self.controls.pan.load();
        let ch = self.channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_CHUNK);
            self.next_chunk(n);
            let block = &self.resampled[..n * ch];
            self.stats.peak.record_scaled(block, gain);
            let rest = &mut out[done * out_channels..];
            remix_add(block, ch, rest, out_channels, gain, position);
            done += n;
        }
    }

    /// Fills `resampled` with `frames` output frames (at most [`MAX_CHUNK`]): reads what the
    /// resampler needs per the drift plan, resamples it and steers the ratio by the level.
    fn next_chunk(&mut self, frames: usize) {
        let ch = self.channels;
        let end = frames * ch;
        // Fails only for zero or more than MAX_CHUNK frames, which never reach here.
        let _ = self.resampler.set_chunk_size(frames);
        let needed = self.resampler.input_frames_next();
        let available = self.consumer.slots() / ch;
        let (plan, event) = self.drift.plan(available, needed);
        match event {
            Event::Underrun => self.stats.underruns.add(1),
            Event::Resync => self.stats.resyncs.add(1),
            Event::Aligned => self.stats.aligned.add(plan.skip as u64),
            Event::None | Event::Priming => {}
        }
        if event == Event::Priming {
            self.resampled[..end].fill(0.0);
            return;
        }
        if let Ok(chunk) = self.consumer.read_chunk(plan.skip * ch) {
            chunk.commit_all();
        }
        let read = plan.read * ch;
        if let Ok(chunk) = self.consumer.read_chunk(read) {
            let (first, second) = chunk.as_slices();
            self.scratch[..first.len()].copy_from_slice(first);
            self.scratch[first.len()..read].copy_from_slice(second);
            chunk.commit_all();
        }
        self.scratch[read..needed * ch].fill(0.0);

        let input = InterleavedSlice::new(&self.scratch[..needed * ch], ch, needed);
        let output = InterleavedSlice::new_mut(&mut self.resampled[..end], ch, frames);
        let processed = match (input, output) {
            (Ok(input), Ok(mut output)) => self
                .resampler
                .process_into_buffer(&input, &mut output, None)
                .is_ok(),
            _ => false,
        };
        if !processed {
            self.resampled[..end].fill(0.0);
        }
        if event == Event::Underrun {
            // The silence after an underrun starts the next priming from a clean filter.
            self.resampler.reset();
            return;
        }
        if let Some(error) = self.drift.steady_error() {
            let correction = self
                .rate
                .update(error / self.input_rate, frames as f64 / self.output_rate);
            // Fails only outside the range the resampler was built for, which the rate
            // control's limit keeps clear of.
            let _ = self
                .resampler
                .set_resample_ratio_relative(1.0 / (1.0 + correction), true);
            #[allow(clippy::cast_possible_truncation)]
            self.stats.correction_ppm.store((correction * 1e6) as f32);
        }
    }
}

/// Sums the inputs of one bus into its output buffer; runs inside the output device's
/// callback.
#[derive(Debug)]
pub struct Mixer {
    inputs: Vec<MixerInput>,
    out_channels: usize,
    controls: Arc<BusControls>,
    stats: Arc<BusStats>,
}

impl Mixer {
    /// A mixer for an output with `out_channels` channels.
    #[must_use]
    pub fn new(inputs: Vec<MixerInput>, out_channels: usize) -> (Self, BusHandle) {
        let handle = BusHandle {
            controls: Arc::new(BusControls::default()),
            stats: Arc::new(BusStats::default()),
        };
        let mixer = Self {
            inputs,
            out_channels,
            controls: Arc::clone(&handle.controls),
            stats: Arc::clone(&handle.stats),
        };
        (mixer, handle)
    }

    /// Closes the rings of all inputs: their captures discard until [`Self::resume`]. For
    /// a bus whose output device is gone and whose mixer therefore no longer drains them.
    pub fn suspend(&mut self) {
        for input in &self.inputs {
            input.open.store(false, Ordering::Release);
        }
    }

    /// Empties the rings, lets every input prime again and reopens them, for an output with
    /// `out_channels` channels (a device may come back with another layout).
    pub fn resume(&mut self, out_channels: usize) {
        self.out_channels = out_channels;
        for input in &mut self.inputs {
            input.restart();
            input.open.store(true, Ordering::Release);
        }
    }

    /// Overwrites `out` (interleaved) with the mix of all inputs times the bus gain, clipped
    /// to full scale. A NaN (a broken input) becomes silence: a device may turn it into
    /// full-scale noise.
    pub fn process(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if self.out_channels == 0 {
            return;
        }
        let frames = out.len() / self.out_channels;
        for input in &mut self.inputs {
            input.mix_into(out, self.out_channels, frames);
        }
        let gain = if self.controls.muted.load(Ordering::Relaxed) {
            0.0
        } else {
            self.controls.gain.load()
        };
        let mut clipped = 0;
        for sample in out.iter_mut() {
            let value = *sample * gain;
            if value.is_nan() {
                clipped += 1;
                *sample = 0.0;
            } else if value.abs() > 1.0 {
                clipped += 1;
                *sample = value.clamp(-1.0, 1.0);
            } else {
                *sample = value;
            }
        }
        if clipped > 0 {
            self.stats.clipped.add(clipped);
        }
        self.stats.peak.record_scaled(out, 1.0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Output callbacks of 480 frames, as WASAPI asks at 48 kHz.
    const PERIOD: usize = 480;

    fn config(channels: usize, target: usize) -> InputConfig {
        InputConfig {
            channels,
            input_rate: 48_000,
            output_rate: 48_000,
            capacity: 16_384,
            target,
            band: 64,
            smoothing: 1.0,
            settle: 0,
        }
    }

    /// Pushes `frames` frames of the constant `value` on every channel.
    fn push_dc(capture: &mut Capture, frames: usize, value: f32) {
        capture.push(&vec![value; frames * capture.channels()]);
    }

    /// Runs `callbacks` output callbacks of `PERIOD` frames into `out_channels` channels,
    /// feeding every capture one period of `value` before each; returns the last output.
    fn run_dc(
        mixer: &mut Mixer,
        captures: &mut [&mut Capture],
        out_channels: usize,
        value: f32,
        callbacks: usize,
    ) -> Vec<f32> {
        let mut out = vec![0.0; PERIOD * out_channels];
        for _ in 0..callbacks {
            for capture in captures.iter_mut() {
                push_dc(capture, PERIOD, value);
            }
            mixer.process(&mut out);
        }
        out
    }

    fn all_near(out: &[f32], expected: &[f32]) -> bool {
        out.chunks_exact(expected.len()).all(|frame| {
            frame
                .iter()
                .zip(expected)
                .all(|(o, e)| (o - e).abs() < 2e-3)
        })
    }

    #[test]
    fn plays_silence_until_primed_then_the_captured_signal() {
        let (mut capture, input, _handle) = connect(config(2, 960));
        let (mut mixer, _) = Mixer::new(vec![input], 2);
        let mut out = vec![9.0; PERIOD * 2];
        mixer.process(&mut out);
        assert!(out.iter().all(|&o| o == 0.0));

        // Primed after the target plus one call's worth; past the filter delay the
        // constant comes through unchanged.
        let out = run_dc(&mut mixer, &mut [&mut capture], 2, 0.25, 10);
        assert!(all_near(&out, &[0.25, 0.25]), "{:?}", &out[..4]);
    }

    #[test]
    fn sums_inputs_with_gain_and_mute() {
        let (mut a, input_a, handle_a) = connect(config(1, 0));
        let (mut b, input_b, handle_b) = connect(config(2, 0));
        let (mut mixer, _) = Mixer::new(vec![input_a, input_b], 2);
        handle_a.controls.gain.store(0.5);
        run_dc(&mut mixer, &mut [&mut a, &mut b], 2, 0.2, 5);
        // The peaks so far include the filter's ringing at the start; measure afresh.
        handle_a.stats.peak.take();
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 2, 0.2, 1);
        assert!(all_near(&out, &[0.3, 0.3]), "{:?}", &out[..2]);
        assert!((handle_a.stats.peak.take() - 0.1).abs() < 2e-3);

        handle_b.controls.muted.store(true, Ordering::Relaxed);
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 2, 0.2, 1);
        assert!(all_near(&out, &[0.1, 0.1]), "{:?}", &out[..2]);
    }

    #[test]
    fn one_input_feeds_two_buses_with_shared_controls() {
        let controls = Arc::new(InputControls::default());
        let (mut to_a, ring_a, handle_a) = connect_with(config(1, 0), Arc::clone(&controls));
        let (mut to_b, ring_b, handle_b) = connect_with(config(1, 0), Arc::clone(&controls));
        let (mut bus_a, _) = Mixer::new(vec![ring_a], 2);
        let (mut bus_b, bus_b_handle) = Mixer::new(vec![ring_b], 2);
        controls.pan.store(1.0);
        bus_b_handle.controls.gain.store(0.5);
        let out_a = run_dc(&mut bus_a, &mut [&mut to_a], 2, 0.8, 5);
        run_dc(&mut bus_b, &mut [&mut to_b], 2, 0.8, 5);
        bus_b_handle.stats.peak.take();
        let out_b = run_dc(&mut bus_b, &mut [&mut to_b], 2, 0.8, 1);
        assert!(all_near(&out_a, &[0.0, 0.8]), "{:?}", &out_a[..2]);
        assert!(all_near(&out_b, &[0.0, 0.4]), "{:?}", &out_b[..2]);
        assert!(Arc::ptr_eq(&handle_a.controls, &handle_b.controls));
        assert!(!Arc::ptr_eq(&handle_a.stats, &handle_b.stats));
        assert!((bus_b_handle.stats.peak.take() - 0.4).abs() < 2e-3);
    }

    #[test]
    fn a_muted_bus_is_silent_but_drains_its_inputs() {
        let (mut capture, input, handle) = connect(config(1, 0));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        bus.controls.muted.store(true, Ordering::Relaxed);
        let out = run_dc(&mut mixer, &mut [&mut capture], 1, 0.5, 3);
        assert!(out.iter().all(|&o| o == 0.0));
        assert!(handle.stats.peak.take() > 0.49);
        bus.controls.muted.store(false, Ordering::Relaxed);
        let mut out = vec![0.0; PERIOD];
        mixer.process(&mut out);
        assert_eq!(handle.stats.underruns.get(), 1);
    }

    #[test]
    fn a_suspended_bus_discards_without_counting_and_resumes_fresh() {
        let (mut capture, input, handle) = connect(config(1, 960));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        push_dc(&mut capture, 2000, 0.3);
        mixer.suspend();
        push_dc(&mut capture, 20_000, 0.9);
        assert_eq!(handle.stats.dropped_at_capture.get(), 0);

        mixer.resume(2);
        // The stale frames are gone: priming needs the target plus a callback again.
        let mut out = vec![9.0; PERIOD * 2];
        mixer.process(&mut out);
        assert!(out.iter().all(|&o| o == 0.0));
        let out = run_dc(&mut mixer, &mut [&mut capture], 2, 0.5, 10);
        assert!(all_near(&out, &[0.5, 0.5]), "{:?}", &out[..2]);
        assert_eq!(capture.channels(), 1);
    }

    #[test]
    fn counts_full_rings() {
        let (mut capture, _, handle) = connect(InputConfig {
            capacity: 4,
            ..config(1, 0)
        });
        capture.push(&[0.1; 6]);
        assert_eq!(handle.stats.dropped_at_capture.get(), 2);
    }

    #[test]
    fn counts_an_underrun_once_and_primes_again() {
        let (mut capture, input, handle) = connect(config(1, 0));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        push_dc(&mut capture, 3 * PERIOD, 0.5);
        let mut out = vec![0.0; PERIOD];
        for _ in 0..6 {
            mixer.process(&mut out);
        }
        // One underrun, then the input waits (primes) instead of running dry again.
        assert_eq!(handle.stats.underruns.get(), 1);
        assert!(out.iter().all(|&o| o == 0.0));
    }

    #[test]
    fn follows_a_fast_input_clock_without_dropouts() {
        // The input clock runs 200 ppm fast: 480.096 frames arrive per output callback.
        // The rate control has to learn that and keep the level on the target.
        let target = 1920;
        let (mut capture, input, handle) = connect(InputConfig {
            smoothing: 1.0 / 128.0,
            settle: 96_000,
            band: target / 4,
            ..config(2, target)
        });
        let (mut mixer, _) = Mixer::new(vec![input], 2);
        let mut out = vec![0.0; PERIOD * 2];
        let mut owed = 0.0;
        let mut frame = vec![0.1; (PERIOD + 1) * 2];
        // Ten simulated minutes.
        for _ in 0..60_000 {
            owed += PERIOD as f64 * (1.0 + 200e-6);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let frames = owed as usize;
            owed -= frames as f64;
            frame.resize(frames * 2, 0.1);
            capture.push(&frame);
            mixer.process(&mut out);
        }
        assert_eq!(handle.stats.underruns.get(), 0);
        assert_eq!(handle.stats.resyncs.get(), 0);
        assert_eq!(handle.stats.dropped_at_capture.get(), 0);
        let ppm = handle.stats.correction_ppm.load();
        assert!((ppm - 200.0).abs() < 20.0, "correction {ppm} ppm");
        assert!(all_near(&out, &[0.1, 0.1]), "{:?}", &out[..2]);
    }

    #[test]
    fn converts_between_rates_and_keeps_the_pitch() {
        for (input_rate, output_rate) in [(44_100, 48_000), (96_000, 48_000), (48_000, 44_100)] {
            // 40 ms of buffering, as the engine sets it, and 10 ms callbacks on both sides.
            let target = input_rate as usize / 25;
            let (mut capture, input, handle) = connect(InputConfig {
                input_rate,
                output_rate,
                smoothing: 1.0 / 128.0,
                settle: 2 * input_rate as usize,
                ..config(1, target)
            });
            let (mut mixer, _) = Mixer::new(vec![input], 1);
            let mut out = vec![0.0; output_rate as usize / 100];
            let block = input_rate as usize / 100;
            let step = std::f32::consts::TAU * 1000.0 / input_rate as f32;
            let mut phase = 0.0_f32;
            let mut data = vec![0.0; block];
            let mut crossings = 0;
            // One simulated minute; the zero crossings of the last ten seconds give the pitch.
            for callback in 0..6000 {
                for sample in &mut data {
                    *sample = 0.5 * phase.sin();
                    phase = (phase + step) % std::f32::consts::TAU;
                }
                capture.push(&data);
                mixer.process(&mut out);
                if callback >= 5000 {
                    crossings += out
                        .windows(2)
                        .filter(|pair| pair[0] < 0.0 && pair[1] >= 0.0)
                        .count();
                }
            }
            let rates = format!("{input_rate} -> {output_rate}");
            assert_eq!(handle.stats.underruns.get(), 0, "{rates}");
            assert_eq!(handle.stats.resyncs.get(), 0, "{rates}");
            // 1 kHz for ten seconds, give or take the crossings at the callback edges.
            assert!(
                (9_000..=10_001).contains(&crossings),
                "{rates}: {crossings}"
            );
            let ppm = handle.stats.correction_ppm.load();
            assert!(ppm.abs() < 50.0, "{rates}: correction {ppm} ppm");
        }
    }

    #[test]
    fn serves_callbacks_longer_than_one_chunk() {
        // The target has to cover a whole callback, as with any buffering.
        let (mut capture, input, handle) = connect(config(1, 4000));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        let mut out = vec![0.0; 3000];
        for _ in 0..4 {
            push_dc(&mut capture, 3000, 0.4);
            mixer.process(&mut out);
        }
        assert!(all_near(&out, &[0.4]), "{:?}", &out[..2]);
        assert_eq!(handle.stats.underruns.get(), 0);
    }

    #[test]
    fn mixes_without_allocating_on_every_path() {
        let (mut capture, input, handle) = connect(InputConfig {
            settle: 4800,
            ..config(2, 480)
        });
        let (mut mixer, _) = Mixer::new(vec![input], 2);
        let data = vec![0.2; PERIOD * 2];
        let flood = vec![0.2; 8000 * 2];
        let mut out = vec![0.0; PERIOD * 2];
        let mut long = vec![0.0; 3000 * 2];
        assert_no_alloc::assert_no_alloc(|| {
            // Priming, then steady playing with the rate control steering.
            for _ in 0..50 {
                capture.push(&data);
                mixer.process(&mut out);
            }
            // A flood beyond the hard limit: one resync.
            capture.push(&flood);
            mixer.process(&mut out);
            // Running dry: an underrun with its resampler reset, then priming again.
            for _ in 0..30 {
                mixer.process(&mut out);
            }
            // A callback longer than one chunk, and a suspended and resumed bus.
            capture.push(&flood);
            mixer.process(&mut long);
            mixer.suspend();
            mixer.resume(2);
            capture.push(&flood);
            mixer.process(&mut out);
        });
        // Every path was taken (the long callback may resync or run dry once more).
        assert!(handle.stats.resyncs.get() >= 1);
        assert!(handle.stats.underruns.get() >= 1);
    }

    #[test]
    fn never_passes_nan_or_infinity_to_the_device() {
        let (mut capture, input, _) = connect(config(1, 0));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        capture.push(&[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.5]);
        let out = run_dc(&mut mixer, &mut [&mut capture], 1, 0.5, 3);
        assert!(out.iter().all(|o| o.is_finite() && o.abs() <= 1.0));
        assert!(bus.stats.clipped.get() > 0);
    }

    #[test]
    fn clips_the_mix_to_full_scale() {
        let (mut capture, input, _) = connect(config(1, 0));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        let out = run_dc(&mut mixer, &mut [&mut capture], 1, 1.5, 3);
        assert!(
            out.iter().all(|&o| (o - 1.0).abs() < f32::EPSILON),
            "{:?}",
            &out[..2]
        );
        assert!(bus.stats.clipped.get() >= PERIOD as u64);
        assert_eq!(bus.stats.peak.take(), 1.0);
    }
}
