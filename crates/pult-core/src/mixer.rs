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
//!
//! The routing matrix can change while the route runs without building anything in a
//! callback: a route has a ring for every input and bus, and [`InputHandle::routed`] switches
//! one on or off. An unrouted ring is closed and skipped (no resampling); when it is routed
//! again, the mixer empties it, primes it afresh and opens it, all inside its own callback.
//!
//! Nothing that changes the sound jumps: gain, pan, mute, solo and the matrix all glide (see
//! [`crate::ramp`]). A ring that is switched off fades out before it closes, one that is
//! switched on fades in once it is primed.
//!
//! An input passes its gate and its compressor ([`crate::dynamics`]) once, in its
//! [`crate::stage::InputStage`] before its rings, at its own rate; a bus passes its equalizer
//! ([`crate::eq`]) after the sum and before its fader, and its limiter ([`crate::limiter`])
//! behind it. An input has two meters: [`InputControls::pre_peak`] in the stage, what arrives
//! before all of that, and the meter of each ring ([`InputStats::peak`]), what goes into that
//! bus's mix.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};

use rtrb::{Consumer, Producer, RingBuffer};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{
    Async, FixedAsync, Resampler, Resizable, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};
// `Adjustable` brings `set_resample_ratio_relative` into scope.
use rubato::Adjustable as _;

use crate::automix::{AutomixGroup, glide_gain};
use crate::drift::{DriftControl, Event};
use crate::dynamics::{CompressorControls, GateControls};
use crate::eq::{EqControls, Equalizer};
use crate::level::{AtomicF32, Counter, PeakMeter};
use crate::limiter::{Limiter, LimiterControls};
use crate::ramp::{Aim, Follower, Glide, MIN_RAMP_MS};
use crate::rate::RateControl;
use crate::remix::{remix_add, remix_add_ramped};

/// Output frames one resampler call produces at most; longer callbacks take several.
const MAX_CHUNK: usize = 1024;

/// Seconds in which the rate control's proportional term would remove a level error; the
/// integral time is four times that (critically damped, see [`crate::rate`]). A compromise
/// two long runs settled: fast enough that a clock jumping by 1500 ppm settles at about
/// 22 ms, below the skip threshold (three times the band, 30 ms at 40 ms latency), so it is
/// steered and learned rather than skipped; slow enough that an error up to that threshold
/// asks for at most 2000 ppm, about three cents, and only for seconds.
const STEER_SECONDS: f64 = 15.0;

/// The largest correction against clock drift: 0.5 %, far beyond any real clock (they differ
/// by tens of ppm) and still far below an audible pitch change.
const MAX_CORRECTION: f64 = 0.005;

/// Gain, mute and pan of one input, set by the control thread.
#[derive(Debug)]
pub struct InputControls {
    /// Linear gain factor.
    pub gain: Glide,
    /// Muted inputs keep draining their ring but add nothing.
    pub muted: AtomicBool,
    /// Milliseconds a change of `muted`, `soloed_out` or a ring's `routed` takes. Set it before
    /// the switch, and switch with `Release`.
    pub fade_ms: AtomicU32,
    /// Silent because other inputs are soloed; the control thread sets it for every input
    /// that is not soloed while any is. Acts like `muted` without touching it.
    pub soloed_out: AtomicBool,
    /// Which meter the desk shows for this input: [`Self::pre_peak`], what arrives before gate,
    /// compressor, gain and mute (true), or the rings' meters, what goes into the mix (false).
    /// Both always measure.
    pub meter_pre_fader: AtomicBool,
    /// The PRE meter of the input, once for all its rings: written by its
    /// [`crate::stage::InputStage`], at the samples of the device, routed or not.
    pub pre_peak: PeakMeter,
    /// The most the gate took away since the desk last read it, in dB (positive).
    pub gate_reduction: PeakMeter,
    /// The most the compressor took away since the desk last read it, in dB (positive).
    pub comp_reduction: PeakMeter,
    /// Whether the gate let the sound through at the end of the last block.
    pub gate_open: AtomicBool,
    /// The automix group of the input and its place in it ([`crate::automix`]), set once when
    /// the route is built; an input without one keeps its gain.
    pub automix: OnceLock<(Arc<AutomixGroup>, usize)>,
    /// -1 (left) to 1 (right); see [`remix_add`].
    pub pan: Glide,
    /// The gate, before the compressor; off at first.
    pub gate: GateControls,
    /// The compressor, before the fader; off at first.
    pub compressor: CompressorControls,
    /// The channel strip before the gate: polarity, filters, delay, width; neutral at first.
    pub strip: crate::strip::StripControls,
}

impl Default for InputControls {
    fn default() -> Self {
        Self {
            gain: Glide::new(1.0),
            muted: AtomicBool::new(false),
            fade_ms: AtomicU32::new(0),
            soloed_out: AtomicBool::new(false),
            meter_pre_fader: AtomicBool::new(false),
            pre_peak: PeakMeter::default(),
            gate_reduction: PeakMeter::default(),
            comp_reduction: PeakMeter::default(),
            gate_open: AtomicBool::new(true),
            automix: OnceLock::new(),
            pan: Glide::new(0.0),
            gate: GateControls::default(),
            compressor: CompressorControls::default(),
            strip: crate::strip::StripControls::default(),
        }
    }
}

/// What happened to one ring of an input, counted by the audio threads.
#[derive(Debug, Default)]
pub struct InputStats {
    /// Peak of what this ring feeds into its bus's mix, after gain, pan and mute.
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
    pub gain: Glide,
    /// A muted bus plays silence; its inputs keep draining.
    pub muted: AtomicBool,
    /// Milliseconds a change of `muted` takes. Set it before the switch, and switch with
    /// `Release`.
    pub fade_ms: AtomicU32,
    /// The equalizer, before the gain; flat at first.
    pub eq: EqControls,
    /// The limiter, behind the gain; off until a route switches it on.
    pub limiter: LimiterControls,
    /// The bus compressor, after the equalizer and before the gain; off at first.
    pub comp: crate::buscomp::BusCompControls,
    /// The delay, behind the limiter; none at first.
    pub delay: crate::busdelay::BusDelayControls,
    /// Whether solo acts on this bus: on every bus while the route has no monitor bus, else
    /// only on the monitor buses, so a talk bus keeps hearing every source. True at first.
    pub solo_here: AtomicBool,
}

impl Default for BusControls {
    fn default() -> Self {
        Self {
            gain: Glide::new(1.0),
            muted: AtomicBool::new(false),
            fade_ms: AtomicU32::new(0),
            eq: EqControls::default(),
            limiter: LimiterControls::default(),
            delay: crate::busdelay::BusDelayControls::default(),
            comp: crate::buscomp::BusCompControls::default(),
            solo_here: AtomicBool::new(true),
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
    /// How far the limiter turned down at most since the last look, in dB.
    pub limited_db: PeakMeter,
    /// The same over the whole run; nobody takes it.
    pub most_limited_db: PeakMeter,
}

/// The control thread's side of one input ring.
#[derive(Debug, Clone)]
pub struct InputHandle {
    /// Gain, mute and pan, shared by all rings of the input.
    pub controls: Arc<InputControls>,
    /// Peaks and counters of this ring.
    pub stats: Arc<InputStats>,
    /// Whether the input feeds this ring's bus: the matrix button. The mixer follows a
    /// change in its next callback.
    pub routed: Arc<AtomicBool>,
    /// The level of this crossing, linear, gliding like a fader: what of the input this bus
    /// gets (an aux send). 1 at first.
    pub send: Arc<Glide>,
    /// Whether the send is taken before the input's fader (pre) instead of after it (post, at
    /// first): a pre send keeps its level when the fader moves, a headphone mix stays when the
    /// stream mix is turned down. Mute and solo act on both.
    pub send_pre: Arc<AtomicBool>,
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
    let drift = DriftControl::new(config.target, config.band)
        .smoothed(config.smoothing)
        .settling(config.settle);
    // The integral learns up to where the drift control skips a surplus; beyond is a backlog.
    let window = drift.skip_threshold() as f64 / f64::from(config.input_rate);
    let scratch = vec![0.0; resampler.input_frames_max() * config.channels];
    let chunk_out = vec![0.0; MAX_CHUNK * config.channels];
    let open = Arc::new(AtomicBool::new(true));
    let handle = InputHandle {
        controls,
        stats: Arc::new(InputStats::default()),
        routed: Arc::new(AtomicBool::new(true)),
        send: Arc::new(Glide::new(1.0)),
        send_pre: Arc::new(AtomicBool::new(false)),
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
        drift,
        rate: RateControl::new(STEER_SECONDS, 4.0 * STEER_SECONDS, MAX_CORRECTION)
            .integrating_within(window),
        resampler,
        input_rate: f64::from(config.input_rate),
        output_rate: f64::from(config.output_rate),
        scratch,
        resampled: chunk_out,
        controls: Arc::clone(&handle.controls),
        stats: Arc::clone(&handle.stats),
        routed: Arc::clone(&handle.routed),
        send: Arc::clone(&handle.send),
        send_pre: Arc::clone(&handle.send_pre),
        seen_fader: handle.controls.gain.load(),
        seen_send: handle.send.load(),
        seen_pre: false,
        aim: handle.controls.gain.load(),
        playing: true,
        gain: Follower::new(config.output_rate),
        pan: Follower::new(config.output_rate),
        automix_gain: 1.0,
        onset_frames: (u64::from(config.input_rate) * u64::from(MIN_RAMP_MS) / 1000).max(1)
            as usize,
        onset: 0,
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
    /// Closed while the ring's bus has no device or its stream has not called back yet; then
    /// `push` discards.
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
    routed: Arc<AtomicBool>,
    /// Whether the ring plays: routed, or fading out after it no longer is.
    playing: bool,
    /// The level of this crossing ([`InputHandle::send`]): the send of the input to this bus.
    send: Arc<Glide>,
    /// Whether the send is taken before the input's fader ([`InputHandle::send_pre`]).
    send_pre: Arc<AtomicBool>,
    /// Fader, send and pre as last seen, and the aim made of them for [`Self::gain`]: a
    /// new aim only when one of them moved, with the time of the one that moved.
    seen_fader: Aim,
    seen_send: Aim,
    seen_pre: bool,
    aim: Aim,
    /// Where the gain stands, silence and the fades of the matrix included.
    gain: Follower,
    /// Where the pan stands.
    pan: Follower,
    /// The automix gain at the end of the last block; the next block glides from it.
    automix_gain: f32,
    /// Input frames over which the sound fades in where it starts after a priming: what the ring
    /// gives then begins anywhere in its waveform.
    onset_frames: usize,
    /// Input frames of that fade-in done so far.
    onset: usize,
}

impl MixerInput {
    /// The same ring, not routed yet: closed, so its capture discards, until
    /// [`InputHandle::routed`] is set.
    #[must_use]
    pub fn unrouted(mut self) -> Self {
        self.routed.store(false, Ordering::Release);
        self.playing = false;
        self.open.store(false, Ordering::Release);
        self
    }

    /// Follows the matrix button: closes the ring when it was switched off and has faded out,
    /// empties, primes and opens it when it was switched on. Returns whether it is routed, or
    /// `None` when it does not play.
    fn follow_routing(&mut self) -> Option<bool> {
        let routed = self.routed.load(Ordering::Acquire);
        if routed && !self.playing {
            self.restart();
            self.gain.silence(self.controls.gain.load());
            self.playing = true;
            self.open.store(true, Ordering::Release);
        } else if !routed && self.playing && self.gain.is_silent() {
            self.playing = false;
            self.stats.correction_ppm.store(0.0);
            self.open.store(false, Ordering::Release);
        }
        self.playing.then_some(routed)
    }

    /// Empties the ring and lets the drift control prime again, with a fresh resampler and
    /// rate control: the device on the other side may run on another clock now.
    fn restart(&mut self) {
        let stale = self.consumer.slots();
        if let Ok(chunk) = self.consumer.read_chunk(stale) {
            chunk.commit_all();
        }
        self.drift.reset();
        self.resampler.reset();
        self.onset = 0;
        self.rate.reset();
        self.stats.correction_ppm.store(0.0);
    }

    /// Resamples `frames` output frames from the ring, chunk by chunk, and adds them into
    /// `out`.
    fn mix_into(&mut self, out: &mut [f32], out_channels: usize, frames: usize, solo_here: bool) {
        let Some(routed) = self.follow_routing() else {
            return;
        };
        // Soloed out only where solo acts: a talk bus beside a monitor bus hears every source.
        let silent = self.controls.muted.load(Ordering::Acquire)
            || (solo_here && self.controls.soloed_out.load(Ordering::Acquire));
        let fade_ms = self.controls.fade_ms.load(Ordering::Relaxed);
        let aim = self.crossing_aim(fade_ms);
        self.gain.follow(aim, routed && !silent, fade_ms);
        self.pan.follow(self.controls.pan.load(), true, 0);
        let ch = self.channels;
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(MAX_CHUNK);
            // A priming ring is silent, and the ramps wait: a fade-in starts with the sound.
            if self.next_chunk(n) {
                // Gate and compressor ran in the input's stage, before the ring; the automix
                // share glides over the block.
                let share = self
                    .controls
                    .automix
                    .get()
                    .map_or(1.0, |(group, index)| group.gain(*index));
                glide_gain(&mut self.resampled[..n * ch], ch, self.automix_gain, share);
                self.automix_gain = share;
                let block = &self.resampled[..n * ch];
                let rest = &mut out[done * out_channels..];
                let before = self.gain.ramp.value();
                if self.gain.ramp.is_steady() && self.pan.ramp.is_steady() {
                    let position = self.pan.ramp.value();
                    remix_add(block, ch, rest, out_channels, before, position);
                } else {
                    let (gain, pan) = (&mut self.gain.ramp, &mut self.pan.ramp);
                    remix_add_ramped(block, ch, rest, out_channels, gain, pan);
                }
                let metered = before.max(self.gain.ramp.value());
                self.stats.peak.record_scaled(block, metered);
            } else {
                self.gain.hold();
            }
            done += n;
        }
    }

    /// The gain this ring aims at: the input's fader times the send of this crossing, or the send
    /// alone when it is taken before the fader. A new aim only when fader, send or pre moved,
    /// reached in the time of what moved (a switch of pre in the fade time, at least the
    /// shortest ramp, since the level jumps).
    fn crossing_aim(&mut self, fade_ms: u32) -> Aim {
        let fader = self.controls.gain.load();
        let send = self.send.load();
        let pre = self.send_pre.load(Ordering::Relaxed);
        if fader == self.seen_fader && send == self.seen_send && pre == self.seen_pre {
            return self.aim;
        }
        let mut ms = 0;
        if fader != self.seen_fader && !pre {
            ms = ms.max(fader.ms());
        }
        if send != self.seen_send {
            ms = ms.max(send.ms());
        }
        if pre != self.seen_pre {
            ms = ms.max(fade_ms.max(MIN_RAMP_MS));
        }
        let base = if pre { 1.0 } else { fader.value() };
        self.seen_fader = fader;
        self.seen_send = send;
        self.seen_pre = pre;
        self.aim = Aim::new(base * send.value(), ms);
        self.aim
    }

    /// Fades in the first `frames` frames of `scratch` while the fade-in after a priming lasts.
    /// It works on the input, before the resampler: a ramp of the gain behind it would start
    /// before the sound, which the filter of the resampler delays.
    fn fade_in(&mut self, frames: usize) {
        if crate::ramp::unsmoothed() {
            self.onset = self.onset_frames;
        }
        if self.onset >= self.onset_frames {
            return;
        }
        let ch = self.channels;
        for frame in self.scratch[..frames * ch].chunks_exact_mut(ch) {
            if self.onset >= self.onset_frames {
                break;
            }
            let gain = self.onset as f32 / self.onset_frames as f32;
            for sample in frame {
                *sample *= gain;
            }
            self.onset += 1;
        }
    }

    /// Fills `resampled` with `frames` output frames (at most [`MAX_CHUNK`]): reads what the
    /// resampler needs per the drift plan, resamples it and steers the ratio by the level.
    /// False, with nothing in `resampled`, while the ring primes.
    fn next_chunk(&mut self, frames: usize) -> bool {
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
            return false;
        }
        let skip = plan.skip * ch;
        let read = plan.read * ch;
        // The plan never reads past what is buffered, so one chunk holds the skip and the read.
        if let Ok(chunk) = self.consumer.read_chunk(skip + read) {
            let (first, second) = chunk.as_slices();
            let at = |i: usize| {
                first
                    .get(i)
                    .copied()
                    .unwrap_or_else(|| second[i - first.len()])
            };
            for (i, sample) in self.scratch[..read].iter_mut().enumerate() {
                *sample = at(skip + i);
            }
            if skip > 0 && !crate::ramp::unsmoothed() {
                // A skip (aligned after settling, or a resync) would jump to another point of
                // the waveform: cross over from the sound that would have come next instead.
                let fade = self.onset_frames.min(plan.read);
                for (frame, out) in self.scratch[..fade * ch].chunks_exact_mut(ch).enumerate() {
                    let new = (frame + 1) as f32 / (fade + 1) as f32;
                    for (c, sample) in out.iter_mut().enumerate() {
                        *sample = at(frame * ch + c) * (1.0 - new) + *sample * new;
                    }
                }
            }
            chunk.commit_all();
        }
        self.fade_in(plan.read);
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
            // The silence after an underrun starts the next priming from a clean filter, and
            // the sound after it fades in again.
            self.resampler.reset();
            self.onset = 0;
            return true;
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
        true
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
    /// Where the bus gain stands, silence included.
    gain: Follower,
    /// The equalizer of the bus, at its output rate. Boxed: what its filters remember is
    /// large, and a mixer is moved around before it plays.
    eq: Box<Equalizer>,
    /// The limiter of the bus, at its output rate.
    limiter: Box<Limiter>,
    /// The delay of the bus, behind the limiter. Boxed: its line is large.
    delay: Box<crate::busdelay::BusDelay>,
    /// The bus compressor, at its output rate.
    comp: Box<crate::buscomp::BusCompressor>,
    /// Resumed, and no callback yet: the next one opens the rings.
    opening: bool,
    /// Where the output goes to be measured for loudness, off this thread.
    loudness: Option<crate::loudness::LoudnessTap>,
}

impl Mixer {
    /// A mixer for an output with `out_channels` channels, at the output rate of its inputs.
    #[must_use]
    pub fn new(inputs: Vec<MixerInput>, out_channels: usize) -> (Self, BusHandle) {
        let handle = BusHandle {
            controls: Arc::new(BusControls::default()),
            stats: Arc::new(BusStats::default()),
        };
        // Only how long a ramp of the bus takes hangs on it; without inputs nothing sounds.
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let rate = inputs
            .first()
            .map_or(48_000, |input| input.output_rate as u32);
        let mixer = Self {
            gain: Follower::new(rate),
            eq: Box::new(Equalizer::new(rate)),
            limiter: Box::new(Limiter::new(rate, out_channels)),
            delay: Box::new(crate::busdelay::BusDelay::new(rate, out_channels)),
            comp: Box::new(crate::buscomp::BusCompressor::new(rate)),
            inputs,
            out_channels,
            controls: Arc::clone(&handle.controls),
            stats: Arc::clone(&handle.stats),
            opening: false,
            loudness: None,
        };
        (mixer, handle)
    }

    /// Closes the rings of all inputs: their captures discard until [`Self::resume`]. For
    /// a bus whose output device is gone and whose mixer therefore no longer drains them.
    pub fn suspend(&mut self) {
        self.opening = false;
        for input in &self.inputs {
            input.open.store(false, Ordering::Release);
        }
    }

    /// Gets ready for an output with `out_channels` channels (a device may come back with
    /// another layout). The rings stay closed until its first callback, which empties the routed
    /// ones, lets them prime again and opens them: an output can take seconds to call back on a
    /// busy machine, and what piles up until then could only be thrown away with a jump.
    pub fn resume(&mut self, out_channels: usize) {
        self.out_channels = out_channels;
        self.gain.start_over();
        self.eq.start_over();
        self.limiter.prepare(out_channels);
        self.delay.prepare(out_channels);
        self.opening = true;
        for input in &mut self.inputs {
            input.open.store(false, Ordering::Release);
            input.gain.start_over();
            input.pan.start_over();
            input.playing = input.routed.load(Ordering::Acquire);
        }
    }

    /// Overwrites `out` (interleaved) with the mix of all inputs times the bus gain, limited,
    /// then clipped to full scale. A NaN (a broken input) becomes silence: a device may turn it into
    /// full-scale noise.
    pub fn process(&mut self, out: &mut [f32]) {
        out.fill(0.0);
        if self.out_channels == 0 {
            return;
        }
        if std::mem::take(&mut self.opening) {
            for input in self.inputs.iter_mut().filter(|input| input.playing) {
                input.restart();
                input.open.store(true, Ordering::Release);
            }
        }
        let frames = out.len() / self.out_channels;
        let solo_here = self.controls.solo_here.load(Ordering::Acquire);
        for input in &mut self.inputs {
            input.mix_into(out, self.out_channels, frames, solo_here);
        }
        self.eq.process(&self.controls.eq, out, self.out_channels);
        self.comp
            .process(&self.controls.comp, out, self.out_channels);
        let sounding = !self.controls.muted.load(Ordering::Acquire);
        let fade_ms = self.controls.fade_ms.load(Ordering::Relaxed);
        self.gain
            .follow(self.controls.gain.load(), sounding, fade_ms);
        if self.gain.ramp.is_steady() {
            let gain = self.gain.ramp.value();
            for sample in out.iter_mut() {
                *sample *= gain;
            }
        } else {
            for frame in out.chunks_exact_mut(self.out_channels) {
                let gain = self.gain.ramp.next_value();
                for sample in frame {
                    *sample *= gain;
                }
            }
        }
        let reduced = self
            .limiter
            .process(&self.controls.limiter, out, self.out_channels);
        self.stats.limited_db.record(reduced);
        self.stats.most_limited_db.record(reduced);
        self.delay
            .process(&self.controls.delay, out, self.out_channels);
        let clipped: u64 = out.iter_mut().map(limited).sum();
        if clipped > 0 {
            self.stats.clipped.add(clipped);
        }
        self.stats.peak.record_scaled(out, 1.0);
        if let Some(tap) = &mut self.loudness {
            tap.feed(out, self.out_channels);
        }
    }

    /// Hands every output from now on to a loudness meter ([`crate::loudness`]).
    pub fn set_loudness(&mut self, tap: crate::loudness::LoudnessTap) {
        self.loudness = Some(tap);
    }
}

/// `sample` limited to full scale; a NaN becomes silence. Returns how many samples that had to
/// change: 1 or 0.
#[inline]
fn limited(sample: &mut f32) -> u64 {
    if sample.is_nan() {
        *sample = 0.0;
        1
    } else if sample.abs() > 1.0 {
        *sample = sample.clamp(-1.0, 1.0);
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::eq::{Band, BandKind};
    use crate::level::{db_to_linear, linear_to_db};

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

        // One callback for the mute's short fade, then it is gone.
        handle_b.controls.muted.store(true, Ordering::Release);
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 2, 0.2, 2);
        assert!(all_near(&out, &[0.1, 0.1]), "{:?}", &out[..2]);
    }

    #[test]
    fn a_matrix_button_switches_a_ring_while_running() {
        let (mut a, input_a, handle_a) = connect(config(1, 480));
        let (mut b, input_b, _) = connect(config(1, 480));
        let (mut mixer, _) = Mixer::new(vec![input_a, input_b.unrouted()], 1);
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 1, 0.2, 6);
        assert!(all_near(&out, &[0.2]), "only a: {:?}", &out[..1]);

        // Off: a adds nothing, its capture discards without counting lost frames.
        handle_a.routed.store(false, Ordering::Release);
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 1, 0.2, 40);
        assert!(out.iter().all(|&o| o == 0.0), "{:?}", &out[..1]);
        assert_eq!(handle_a.stats.dropped_at_capture.get(), 0);
        assert_eq!(handle_a.stats.underruns.get(), 0);

        // On again: a primes afresh from an empty ring and plays.
        handle_a.routed.store(true, Ordering::Release);
        let mut out = vec![9.0; PERIOD];
        mixer.process(&mut out);
        assert!(out.iter().all(|&o| o == 0.0), "priming: {:?}", &out[..1]);
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 1, 0.2, 6);
        assert!(all_near(&out, &[0.2]), "{:?}", &out[..1]);
    }

    /// Runs `callbacks` callbacks of a 103 Hz tone at half scale through `capture` and returns all
    /// that `mixer` plays (mono).
    fn record_tone(
        mixer: &mut Mixer,
        capture: &mut Capture,
        tone: &mut crate::tone::Sine,
        callbacks: usize,
    ) -> Vec<f32> {
        let mut all = Vec::with_capacity(callbacks * PERIOD);
        let mut feed = vec![0.0; PERIOD];
        let mut out = vec![0.0; PERIOD];
        for _ in 0..callbacks {
            tone.fill(&mut feed, 1);
            capture.push(&feed);
            mixer.process(&mut out);
            all.extend_from_slice(&out);
        }
        all
    }

    #[test]
    fn a_ring_starts_to_play_without_a_click() {
        // The sound a ring gives after priming begins anywhere in its waveform; it fades in on
        // the input side, since a ramp of the gain behind the resampler would be partly over
        // before the filter lets the sound through.
        let detector = crate::clicks::Clicks::for_tone(103.0, 48_000);
        let mut tone = crate::tone::Sine::new(103.0, 48_000, 0.5);
        let (mut capture, input, handle) = connect(config(1, 480));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        mixer.resume(1);
        let started = record_tone(&mut mixer, &mut capture, &mut tone, 20);
        assert!(started.iter().any(|s| s.abs() > 0.4), "it plays");
        assert_eq!(detector.find(&started, 1), Vec::new(), "at the start");

        // Off and on again with the matrix button: primes afresh and fades in.
        handle.routed.store(false, Ordering::Release);
        record_tone(&mut mixer, &mut capture, &mut tone, 5);
        handle.routed.store(true, Ordering::Release);
        let again = record_tone(&mut mixer, &mut capture, &mut tone, 20);
        assert!(again.iter().any(|s| s.abs() > 0.4), "it plays again");
        assert_eq!(detector.find(&again, 1), Vec::new(), "routed again");
    }

    /// Runs `callbacks` callbacks of `value` on one mono ring into a mono bus and returns all
    /// of the output.
    fn record(mixer: &mut Mixer, capture: &mut Capture, value: f32, callbacks: usize) -> Vec<f32> {
        let mut all = Vec::with_capacity(callbacks * PERIOD);
        let mut out = vec![0.0; PERIOD];
        for _ in 0..callbacks {
            push_dc(capture, PERIOD, value);
            mixer.process(&mut out);
            all.extend_from_slice(&out);
        }
        all
    }

    /// The largest step between neighbouring frames.
    fn largest_step(out: &[f32]) -> f32 {
        out.windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    /// A playing ring of `value` on a mono bus, past the ringing of the filter at its start.
    fn playing(value: f32) -> (Mixer, Capture, InputHandle, BusHandle) {
        let (mut capture, input, handle) = connect(config(1, 480));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        record(&mut mixer, &mut capture, value, 10);
        (mixer, capture, handle, bus)
    }

    #[test]
    fn the_limiter_sits_behind_the_bus_fader_and_nothing_clips() {
        let (mut mixer, mut capture, _, bus) = playing(0.5);
        bus.controls.limiter.set_all(crate::limiter::LimitSettings {
            on: true,
            ceiling_db: -6.0,
            lookahead_ms: 0.0,
        });
        // Switched on, it slides down from out of the way.
        record(&mut mixer, &mut capture, 0.5, 10);
        bus.controls.gain.store(8.0);
        let out = record(&mut mixer, &mut capture, 0.5, 200);
        let peak = out.iter().fold(0.0_f32, |max, s| max.max(s.abs()));
        // The fader alone would take the 0.5 to 4; the slide of the fader and the ceiling are done.
        assert!((out[out.len() - 1] - db_to_linear(-6.0)).abs() < 1e-3);
        assert!(peak <= 1.0);
        assert_eq!(bus.stats.clipped.get(), 0);
        assert!(bus.stats.limited_db.take() > 6.0);
        // The meter behind it shows what leaves the bus.
        assert!(bus.stats.peak.take() <= 1.0);
    }

    #[test]
    fn with_the_limiter_off_the_clamp_still_counts() {
        let (mut mixer, mut capture, _, bus) = playing(0.5);
        bus.controls.gain.store(8.0);
        let out = record(&mut mixer, &mut capture, 0.5, 10);
        assert!(out.iter().all(|s| s.abs() <= 1.0));
        assert!(bus.stats.clipped.get() > 0);
        assert_eq!(bus.stats.limited_db.take(), 0.0);
    }

    #[test]
    fn a_broken_sum_stays_silence_with_the_limiter_on() {
        let (capture, input, _handle) = connect(config(1, 0));
        drop(capture);
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        bus.controls.limiter.set_all(crate::limiter::LimitSettings {
            on: true,
            ceiling_db: -6.0,
            lookahead_ms: 1.0,
        });
        bus.controls.gain.store(f32::NAN);
        let mut out = vec![0.0; PERIOD];
        for _ in 0..50 {
            mixer.process(&mut out);
        }
        assert!(out.iter().all(|s| s.is_finite()));
    }

    #[test]
    fn a_device_back_with_more_channels_is_limited_too() {
        let (mut capture, input, _handle) = connect(config(1, 480));
        let (mut mixer, bus) = Mixer::new(vec![input], 1);
        bus.controls.limiter.set_all(crate::limiter::LimitSettings {
            on: true,
            ceiling_db: -6.0,
            lookahead_ms: 0.0,
        });
        bus.controls.gain.store(8.0);
        record(&mut mixer, &mut capture, 0.5, 10);
        mixer.suspend();
        mixer.resume(2);
        let mut out = vec![0.0; PERIOD * 2];
        for _ in 0..200 {
            push_dc(&mut capture, PERIOD, 0.5);
            mixer.process(&mut out);
        }
        assert!(out.iter().all(|s| s.abs() <= db_to_linear(-6.0) * 1.0001));
        assert!(out[0].abs() > 0.1);
    }

    #[test]
    fn a_gain_ramp_takes_its_time_and_has_no_step() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        // 100 ms at 48 kHz: 4800 frames, ten callbacks.
        handle.controls.gain.ramp_to(0.0, 100);
        let out = record(&mut mixer, &mut capture, 0.5, 12);
        assert!(
            largest_step(&out) < 0.5 / 4800.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert!(
            (out[0] - 0.5).abs() < 2e-3,
            "it starts where it stood: {}",
            out[0]
        );
        assert!((out[2400] - 0.25).abs() < 2e-3, "halfway: {}", out[2400]);
        assert!(out[4800..].iter().all(|&o| o == 0.0));
    }

    #[test]
    fn a_send_scales_its_crossing_after_the_fader() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.controls.gain.store(0.5);
        handle.send.ramp_to(0.5, 50);
        let out = record(&mut mixer, &mut capture, 0.5, 20);
        assert!(
            (out[out.len() - 1] - 0.125).abs() < 2e-3,
            "{}",
            out[out.len() - 1]
        );
        assert!(largest_step(&out) < 0.01, "no step: {}", largest_step(&out));
    }

    #[test]
    fn a_pre_send_keeps_its_level_when_the_fader_moves() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.send.store(0.5);
        handle.send_pre.store(true, Ordering::Relaxed);
        record(&mut mixer, &mut capture, 0.5, 10);
        handle.controls.gain.ramp_to(0.1, 20);
        let out = record(&mut mixer, &mut capture, 0.5, 10);
        assert!(
            (out[out.len() - 1] - 0.25).abs() < 2e-3,
            "pre: {}",
            out[out.len() - 1]
        );
        // Back to post: the fader counts again, gliding, not in a step.
        handle.send_pre.store(false, Ordering::Relaxed);
        let out = record(&mut mixer, &mut capture, 0.5, 20);
        assert!(
            (out[out.len() - 1] - 0.025).abs() < 2e-3,
            "post: {}",
            out[out.len() - 1]
        );
        assert!(largest_step(&out) < 0.01, "no step: {}", largest_step(&out));
    }

    #[test]
    fn a_muted_input_is_silent_on_a_pre_send_too() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.send_pre.store(true, Ordering::Relaxed);
        handle.controls.muted.store(true, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 0.5, 20);
        assert_eq!(out[out.len() - 1], 0.0);
    }

    #[test]
    fn a_change_without_a_time_is_still_free_of_steps() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.controls.gain.store(0.25);
        let out = record(&mut mixer, &mut capture, 0.5, 2);
        // 5 ms: 240 frames from 0.5 to 0.125.
        assert!(
            largest_step(&out) < 0.375 / 240.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert!((out[out.len() - 1] - 0.125).abs() < 2e-3);
    }

    #[test]
    fn a_mute_fades_for_as_long_as_it_is_told_and_back() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.controls.muted.store(true, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 0.5, 2);
        assert!(
            largest_step(&out) < 0.5 / 240.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert_eq!(out[out.len() - 1], 0.0);

        handle.controls.fade_ms.store(50, Ordering::Relaxed);
        handle.controls.muted.store(false, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 0.5, 6);
        assert!(
            largest_step(&out) < 0.5 / 2400.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert!((out[1200] - 0.25).abs() < 2e-3, "halfway: {}", out[1200]);
        assert!((out[out.len() - 1] - 0.5).abs() < 2e-3);
    }

    #[test]
    fn a_pan_ramp_moves_the_sides_without_a_step() {
        let (mut capture, input, handle) = connect(config(1, 480));
        let (mut mixer, _) = Mixer::new(vec![input], 2);
        let mut out = vec![0.0; PERIOD * 2];
        let mut left = Vec::new();
        for callback in 0..16 {
            if callback == 10 {
                handle.controls.pan.ramp_to(1.0, 40);
            }
            push_dc(&mut capture, PERIOD, 0.5);
            mixer.process(&mut out);
            if callback >= 10 {
                left.extend(out.as_chunks::<2>().0.iter().map(|frame| frame[0]));
            }
        }
        // 40 ms: 1920 frames from the left at full level to silence.
        assert!(
            largest_step(&left) < 0.5 / 1920.0 * 1.5,
            "{}",
            largest_step(&left)
        );
        assert!((left[0] - 0.5).abs() < 2e-3);
        assert_eq!(left[left.len() - 1], 0.0);
    }

    #[test]
    fn a_bus_gain_ramps_and_a_bus_mute_fades() {
        let (mut mixer, mut capture, _, bus) = playing(0.5);
        bus.controls.gain.ramp_to(0.5, 50);
        let out = record(&mut mixer, &mut capture, 0.5, 6);
        // 50 ms: 2400 frames for a change of 0.25.
        assert!(
            largest_step(&out) < 0.25 / 2400.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert!((out[out.len() - 1] - 0.25).abs() < 2e-3);

        bus.controls.muted.store(true, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 0.5, 2);
        assert!(
            largest_step(&out) < 0.25 / 240.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert_eq!(out[out.len() - 1], 0.0);
    }

    #[test]
    fn a_matrix_button_off_fades_out_before_the_ring_closes() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.controls.fade_ms.store(50, Ordering::Relaxed);
        handle.routed.store(false, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 0.5, 8);
        assert!(
            largest_step(&out) < 0.5 / 2400.0 * 1.5,
            "{}",
            largest_step(&out)
        );
        assert!((out[1200] - 0.25).abs() < 2e-3, "halfway: {}", out[1200]);
        assert_eq!(out[out.len() - 1], 0.0);
        // Closed now: a full ring would count as lost frames if it were still open.
        let before = handle.stats.dropped_at_capture.get();
        push_dc(&mut capture, 100_000, 0.5);
        assert_eq!(handle.stats.dropped_at_capture.get(), before);
    }

    #[test]
    fn a_matrix_button_back_on_during_the_fade_turns_it_around() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.controls.fade_ms.store(50, Ordering::Relaxed);
        handle.routed.store(false, Ordering::Release);
        let down = record(&mut mixer, &mut capture, 0.5, 2);
        handle.routed.store(true, Ordering::Release);
        let up = record(&mut mixer, &mut capture, 0.5, 8);
        let all: Vec<f32> = down.into_iter().chain(up).collect();
        // No priming on the way: the ring never closed, so no gap and no step.
        assert!(
            largest_step(&all) < 0.5 / 2400.0 * 1.5,
            "{}",
            largest_step(&all)
        );
        assert!((all[all.len() - 1] - 0.5).abs() < 2e-3);
        assert_eq!(handle.stats.underruns.get(), 0);
    }

    #[test]
    fn a_matrix_button_on_fades_in_after_priming() {
        let (mut capture, input, handle) = connect(config(1, 480));
        let (mut mixer, _) = Mixer::new(vec![input.unrouted()], 1);
        record(&mut mixer, &mut capture, 0.5, 2);
        handle.controls.fade_ms.store(50, Ordering::Relaxed);
        handle.routed.store(true, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 0.5, 12);
        let first = out.iter().position(|&o| o != 0.0).expect("it plays");
        assert!(first > 0, "it primes first");
        // Constant level starting from nothing rings through the filter (a real signal rings
        // far less); near the start of the fade that is what the fade is there to quieten. The
        // sound itself fades in over its first 5 ms too (before the resampler, so 64 frames late).
        let (ringing, rest) = out[first..].split_at(512);
        assert!(largest_step(ringing) < 0.02, "{}", largest_step(ringing));
        assert!(
            largest_step(rest) < 0.5 / 2400.0 * 1.5,
            "{}",
            largest_step(rest)
        );
        assert!((out[out.len() - 1] - 0.5).abs() < 2e-3);
    }

    #[test]
    fn a_ring_without_sound_closes_at_once_when_switched_off() {
        // Nothing arrives: it stays priming, where a ramp would wait for ever.
        let (mut capture, input, handle) = connect(config(1, 480));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        let mut out = vec![0.0; PERIOD];
        mixer.process(&mut out);
        handle.controls.fade_ms.store(1000, Ordering::Relaxed);
        handle.routed.store(false, Ordering::Release);
        mixer.process(&mut out);
        mixer.process(&mut out);
        // Closed: a flood counts no lost frames.
        push_dc(&mut capture, 100_000, 0.5);
        assert_eq!(handle.stats.dropped_at_capture.get(), 0);
    }

    #[test]
    fn a_ring_that_was_closed_fades_in_for_the_time_of_the_switch() {
        let (mut capture, input, handle) = connect(config(1, 480));
        let (mut mixer, _) = Mixer::new(vec![input.unrouted()], 1);
        // A long ramp long ago, while the ring was closed.
        handle.controls.gain.ramp_to(0.5, 60_000);
        record(&mut mixer, &mut capture, 1.0, 2);
        handle.routed.store(true, Ordering::Release);
        let out = record(&mut mixer, &mut capture, 1.0, 8);
        assert!(
            (out[out.len() - 1] - 0.5).abs() < 2e-3,
            "{}",
            out[out.len() - 1]
        );
    }

    #[test]
    fn a_meter_reads_the_ramp_and_the_level_before_it() {
        let (mut mixer, mut capture, handle, _) = playing(0.5);
        handle.stats.peak.take();
        handle.controls.gain.ramp_to(0.0, 100);
        record(&mut mixer, &mut capture, 0.5, 1);
        assert!((handle.stats.peak.take() - 0.5).abs() < 2e-3);
        record(&mut mixer, &mut capture, 0.5, 11);
        handle.stats.peak.take();
        record(&mut mixer, &mut capture, 0.5, 1);
        assert_eq!(handle.stats.peak.take(), 0.0);
    }

    #[test]
    fn a_resumed_bus_keeps_unrouted_rings_closed() {
        let (mut a, input_a, handle_a) = connect(config(1, 0));
        let (mut mixer, _) = Mixer::new(vec![input_a.unrouted()], 1);
        mixer.suspend();
        mixer.resume(1);
        a.push(&[0.5; 2000]);
        assert_eq!(handle_a.stats.dropped_at_capture.get(), 0);
        let mut out = vec![9.0; PERIOD];
        mixer.process(&mut out);
        assert!(out.iter().all(|&o| o == 0.0));
        assert_eq!(handle_a.stats.underruns.get(), 0);
    }

    #[test]
    fn solo_silences_like_mute_and_the_ring_meter_reads_after_the_fader() {
        let (mut a, input_a, handle_a) = connect(config(1, 0));
        let (mut b, input_b, _) = connect(config(1, 0));
        let (mut mixer, _) = Mixer::new(vec![input_a, input_b], 1);
        handle_a.controls.soloed_out.store(true, Ordering::Relaxed);
        handle_a.controls.gain.store(0.5);
        run_dc(&mut mixer, &mut [&mut a, &mut b], 1, 0.2, 5);
        handle_a.stats.peak.take();
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 1, 0.2, 1);
        assert!(all_near(&out, &[0.2]), "{:?}", &out[..1]);
        assert_eq!(handle_a.stats.peak.take(), 0.0);

        // What arrives before the fader is the stage's meter (crate::stage); the ring's meter
        // reads what goes into the mix: half of 0.2 once the solo ends.
        handle_a.controls.soloed_out.store(false, Ordering::Release);
        let out = run_dc(&mut mixer, &mut [&mut a, &mut b], 1, 0.2, 2);
        assert!(all_near(&out, &[0.3]), "{:?}", &out[..1]);
        assert!((handle_a.stats.peak.take() - 0.1).abs() < 2e-3);
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
    fn a_ring_stays_closed_until_the_first_callback_of_its_output() {
        // An output stream can take seconds to call back on a busy machine, while its input
        // already delivers: more than the ring holds, before anything could have played. The
        // band is wider than a callback, as the engine's are, so priming alone is no surplus.
        let (mut capture, input, handle) = connect(InputConfig {
            band: 480,
            ..config(1, 960)
        });
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        mixer.suspend();
        push_dc(&mut capture, 40_000, 0.9);
        mixer.resume(1);
        push_dc(&mut capture, 40_000, 0.9);

        // The first callback finds an empty ring and primes it, without a jump.
        let mut out = vec![9.0; PERIOD];
        mixer.process(&mut out);
        assert!(out.iter().all(|&o| o == 0.0));
        let out = run_dc(&mut mixer, &mut [&mut capture], 1, 0.5, 10);
        assert!(all_near(&out, &[0.5]), "{:?}", &out[..1]);
        assert_eq!(handle.stats.dropped_at_capture.get(), 0);
        assert_eq!(handle.stats.resyncs.get(), 0);
        assert_eq!(handle.stats.underruns.get(), 0);
        assert_eq!(handle.stats.aligned.get(), 0);
    }

    #[test]
    fn a_bus_resumed_and_suspended_before_it_called_back_stays_closed() {
        // A stream that failed to start, or stopped before its first callback.
        let (mut capture, input, handle) = connect(config(1, 0));
        let (mut mixer, _) = Mixer::new(vec![input], 1);
        mixer.resume(1);
        mixer.suspend();
        let mut out = vec![0.0; PERIOD];
        mixer.process(&mut out);
        push_dc(&mut capture, 40_000, 0.9);
        assert_eq!(handle.stats.dropped_at_capture.get(), 0);
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
    fn learns_a_clock_that_jumps_without_skipping() {
        // A steady input that suddenly runs 1500 ppm fast (a busy machine, a device that
        // changed its clock): the resampler has to follow without a single resync.
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
        let mut frame = vec![0.1; (PERIOD + 2) * 2];
        // Ten simulated minutes, the jump after one.
        for callback in 0..60_000 {
            let drift = if callback < 6000 { 0.0 } else { 1500e-6 };
            owed += PERIOD as f64 * (1.0 + drift);
            #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
            let frames = owed as usize;
            owed -= frames as f64;
            frame.resize(frames * 2, 0.1);
            capture.push(&frame);
            mixer.process(&mut out);
        }
        assert_eq!(handle.stats.resyncs.get(), 0);
        assert_eq!(handle.stats.underruns.get(), 0);
        let ppm = handle.stats.correction_ppm.load();
        assert!((ppm - 1500.0).abs() < 50.0, "correction {ppm} ppm");
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

    /// How long a second of sound takes through one bus with two stereo rings, in ms. A
    /// measurement, not a check. The allocation check of these tests needs debug assertions, so:
    /// `CARGO_PROFILE_RELEASE_DEBUG_ASSERTIONS=true cargo test -p pult-core --release
    /// what_the_dsp_costs -- --ignored --nocapture`.
    fn time_a_second(set: impl Fn(&InputHandle, &BusHandle)) -> f64 {
        let (mut a, ring_a, handle_a) = connect(config(2, 960));
        let (mut b, ring_b, handle_b) = connect(config(2, 960));
        let (mut mixer, bus) = Mixer::new(vec![ring_a, ring_b], 2);
        set(&handle_a, &bus);
        set(&handle_b, &bus);
        let step = std::f32::consts::TAU * 440.0 / 48_000.0;
        let mut phase = 0.0_f32;
        let mut data = vec![0.0; PERIOD * 2];
        let mut out = vec![0.0; PERIOD * 2];
        let mut spent = std::time::Duration::ZERO;
        for _ in 0..1000 {
            for frame in data.as_chunks_mut::<2>().0 {
                let sample = 0.5 * phase.sin();
                *frame = [sample, sample];
                phase = (phase + step) % std::f32::consts::TAU;
            }
            a.push(&data);
            b.push(&data);
            let begun = std::time::Instant::now();
            mixer.process(&mut out);
            spent += begun.elapsed();
        }
        // A thousand callbacks of 10 ms are ten seconds of sound.
        spent.as_secs_f64() * 100.0
    }

    #[test]
    #[ignore = "a measurement, not a check"]
    fn what_the_dsp_costs() {
        let plain = time_a_second(|_, _| {});
        // Gate and compressor run in the input stage now: crate::stage, what_the_stage_saves.
        let all = time_a_second(|_, bus| {
            for band in 0..crate::eq::EQ_BANDS {
                bus.controls.eq.set(band, Band::at(band).with_gain(3.0));
            }
        });
        println!("per second of sound, two stereo rings on one bus:");
        println!("  plain                {plain:.2} ms");
        println!("  with all four bands  {all:.2} ms");
    }

    #[test]
    fn a_bus_passes_its_equalizer_before_its_fader() {
        let (mut mixer, mut capture, _, bus) = playing(0.25);
        // A low shelf raises a constant level by its whole gain.
        bus.controls.eq.set(
            0,
            Band {
                kind: BandKind::LowShelf,
                freq_hz: 200.0,
                gain_db: linear_to_db(2.0),
                q: 0.7,
            },
        );
        let out = record(&mut mixer, &mut capture, 0.25, 20);
        assert!(
            largest_step(&out) < 0.01,
            "it slides: {}",
            largest_step(&out)
        );
        assert!((out[out.len() - 1] - 0.5).abs() < 2e-3);
        bus.controls.gain.store(0.5);
        let out = record(&mut mixer, &mut capture, 0.25, 4);
        assert!((out[out.len() - 1] - 0.25).abs() < 2e-3);
        // What the equalizer raises beyond full scale is clipped and counted like any sum.
        bus.controls.gain.store(1.0);
        record(&mut mixer, &mut capture, 0.9, 4);
        assert!(bus.stats.clipped.get() > 0);
    }

    #[test]
    fn a_resumed_bus_stands_at_its_equalizer_at_once() {
        let (mut mixer, mut capture, _, bus) = playing(0.25);
        mixer.suspend();
        bus.controls.eq.set(
            0,
            Band {
                kind: BandKind::LowShelf,
                freq_hz: 200.0,
                gain_db: linear_to_db(2.0),
                q: 0.7,
            },
        );
        mixer.resume(1);
        let out = record(&mut mixer, &mut capture, 0.25, 12);
        // Past the priming and the filters' start, with no slide of the gain on the way.
        assert!((out[out.len() - 1] - 0.5).abs() < 2e-3);
        let first = out.iter().position(|&o| o != 0.0).expect("it plays");
        assert!(out[first + 480..].iter().all(|&o| (o - 0.5).abs() < 0.01));
    }

    #[test]
    fn mixes_without_allocating_on_every_path() {
        let (mut capture, input, handle) = connect(InputConfig {
            settle: 4800,
            ..config(2, 480)
        });
        // In an automix group of two: the share glides on every block.
        let group = Arc::new(AutomixGroup::new(2));
        group.member(1).store_power(0.01);
        handle.controls.automix.set((group, 0)).unwrap();
        let (mut mixer, bus) = Mixer::new(vec![input], 2);
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
            // A matrix button off and on again.
            handle.routed.store(false, Ordering::Release);
            mixer.process(&mut out);
            mixer.process(&mut out);
            handle.routed.store(true, Ordering::Release);
            capture.push(&flood);
            mixer.process(&mut out);
            // Ramps of gain, pan and the bus, past their end.
            handle.controls.gain.ramp_to(0.5, 20);
            handle.controls.pan.ramp_to(-1.0, 20);
            bus.controls.gain.ramp_to(0.5, 20);
            for _ in 0..5 {
                capture.push(&data);
                mixer.process(&mut out);
            }
            // The equalizer: set, working, and switched off again (gate and compressor are the
            // stage's, tested there).
            bus.controls.eq.set(1, Band::at(1).with_gain(6.0));
            // The limiter at work, with another lookahead, and switched off.
            bus.controls.limiter.set_all(crate::limiter::LimitSettings {
                on: true,
                ceiling_db: -20.0,
                lookahead_ms: 5.0,
            });
            for round in 0..10 {
                if round == 3 {
                    bus.controls.limiter.set_lookahead(1.0);
                }
                if round == 5 {
                    bus.controls.eq.switch(false);
                    bus.controls.limiter.switch(false);
                }
                capture.push(&data);
                mixer.process(&mut out);
            }
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
