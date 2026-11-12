//! The channel strip of an input, as on an SSL 4000 G or any digital console: polarity, a
//! high-pass and a low-pass filter, a delay and the width of a stereo input, before its gate and
//! compressor ([`crate::stage::InputStage`]).
//!
//! It runs at the rate of the input, in its capture callback; the delay line is made in
//! [`ChannelStrip::new`], nothing is allocated while it works. Each part is off at first and
//! costs nothing while it is.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::level::AtomicF32;

/// Range of the high-pass in Hz; at its lower end it is off.
pub const HPF_RANGE_HZ: (f32, f32) = (20.0, 1000.0);
/// Range of the low-pass in Hz; at its upper end it is off.
pub const LPF_RANGE_HZ: (f32, f32) = (1000.0, 20_000.0);
/// The longest delay in milliseconds: enough to line up a microphone 80 m away.
pub const MAX_DELAY_MS: f32 = 250.0;
/// Range of the width of a stereo input in percent: 0 is mono, 100 as it comes, 200 twice
/// as wide.
pub const WIDTH_RANGE: (f32, f32) = (0.0, 200.0);

/// The settings of a channel strip, as a view, a profile and the protocol carry them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct StripSettings {
    /// The polarity is turned over (the Ø button).
    pub polarity: bool,
    /// The corner of the high-pass in Hz; [`HPF_RANGE_HZ`]`.0` is off.
    pub hpf_hz: f32,
    /// The corner of the low-pass in Hz; [`LPF_RANGE_HZ`]`.1` is off.
    pub lpf_hz: f32,
    /// The delay in milliseconds.
    pub delay_ms: f32,
    /// The width of a stereo input in percent; 100 as it comes. A mono input ignores it.
    pub width: f32,
}

impl Default for StripSettings {
    fn default() -> Self {
        Self {
            polarity: false,
            hpf_hz: HPF_RANGE_HZ.0,
            lpf_hz: LPF_RANGE_HZ.1,
            delay_ms: 0.0,
            width: 100.0,
        }
    }
}

impl StripSettings {
    /// Every setting as it starts: the strip does nothing.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Makes one change, unchecked ([`StripChange::checked`] checks).
    pub fn apply(&mut self, change: StripChange) {
        match change {
            StripChange::Polarity(on) => self.polarity = on,
            StripChange::Hpf(hz) => self.hpf_hz = hz,
            StripChange::Lpf(hz) => self.lpf_hz = hz,
            StripChange::Delay(ms) => self.delay_ms = ms,
            StripChange::Width(percent) => self.width = percent,
        }
    }

    /// Every setting, checked.
    ///
    /// # Errors
    ///
    /// The message of the first one outside its range.
    pub fn checked(self) -> Result<Self, String> {
        StripChange::Hpf(self.hpf_hz).checked()?;
        StripChange::Lpf(self.lpf_hz).checked()?;
        StripChange::Delay(self.delay_ms).checked()?;
        StripChange::Width(self.width).checked()?;
        Ok(self)
    }
}

/// One change of a channel strip.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum StripChange {
    /// Turns the polarity over (true) or back.
    Polarity(bool),
    /// The corner of the high-pass in Hz.
    Hpf(f32),
    /// The corner of the low-pass in Hz.
    Lpf(f32),
    /// The delay in milliseconds.
    Delay(f32),
    /// The width in percent.
    Width(f32),
}

impl StripChange {
    /// What it sets, as a reply says it: `polarity turned over`, `high-pass 80 Hz`,
    /// `low-pass off`, `delay 12.5 ms`, `width 0 % (mono)`.
    #[must_use]
    pub fn said(self) -> String {
        match self {
            Self::Polarity(true) => "polarity turned over".to_owned(),
            Self::Polarity(false) => "polarity as it comes".to_owned(),
            Self::Hpf(hz) if hz <= HPF_RANGE_HZ.0 => "high-pass off".to_owned(),
            Self::Hpf(hz) => format!("high-pass {hz:.0} Hz"),
            Self::Lpf(hz) if hz >= LPF_RANGE_HZ.1 => "low-pass off".to_owned(),
            Self::Lpf(hz) => format!("low-pass {hz:.0} Hz"),
            Self::Delay(ms) => format!("delay {ms:.1} ms"),
            Self::Width(percent) if percent <= 0.0 => "width 0 % (mono)".to_owned(),
            Self::Width(percent) => format!("width {percent:.0} %"),
        }
    }

    /// The change when its value is inside its range: nothing is clamped, as with a gain.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise.
    pub fn checked(self) -> Result<Self, String> {
        let (what, value, (low, high), unit) = match self {
            Self::Polarity(_) => return Ok(self),
            Self::Hpf(hz) => ("high-pass", hz, HPF_RANGE_HZ, " Hz"),
            Self::Lpf(hz) => ("low-pass", hz, LPF_RANGE_HZ, " Hz"),
            Self::Delay(ms) => ("delay", ms, (0.0, MAX_DELAY_MS), " ms"),
            Self::Width(percent) => ("width", percent, WIDTH_RANGE, " %"),
        };
        if (low..=high).contains(&value) {
            Ok(self)
        } else {
            Err(format!(
                "the {what} must be {low} to {high}{unit}, not {value}"
            ))
        }
    }
}

/// The channel strip of one input as the control thread sets it.
#[derive(Debug)]
pub struct StripControls {
    polarity: AtomicBool,
    hpf_hz: AtomicF32,
    lpf_hz: AtomicF32,
    delay_ms: AtomicF32,
    width: AtomicF32,
    /// Changes of the filters, so they are computed again only when a corner moved.
    generation: AtomicU32,
}

impl Default for StripControls {
    fn default() -> Self {
        let settings = StripSettings::default();
        Self {
            polarity: AtomicBool::new(settings.polarity),
            hpf_hz: AtomicF32::new(settings.hpf_hz),
            lpf_hz: AtomicF32::new(settings.lpf_hz),
            delay_ms: AtomicF32::new(settings.delay_ms),
            width: AtomicF32::new(settings.width),
            generation: AtomicU32::new(0),
        }
    }
}

impl StripControls {
    /// Makes one change; a value outside its range is clamped (check before with
    /// [`StripChange::checked`]), one that is no number leaves the setting as it is.
    pub fn set(&self, change: StripChange) {
        let clamped =
            |value: f32, (low, high): (f32, f32)| value.is_finite().then(|| value.clamp(low, high));
        match change {
            StripChange::Polarity(on) => self.polarity.store(on, Ordering::Relaxed),
            StripChange::Hpf(hz) => {
                if let Some(hz) = clamped(hz, HPF_RANGE_HZ) {
                    self.hpf_hz.store(hz);
                    self.generation.fetch_add(1, Ordering::Release);
                }
            }
            StripChange::Lpf(hz) => {
                if let Some(hz) = clamped(hz, LPF_RANGE_HZ) {
                    self.lpf_hz.store(hz);
                    self.generation.fetch_add(1, Ordering::Release);
                }
            }
            StripChange::Delay(ms) => {
                if let Some(ms) = clamped(ms, (0.0, MAX_DELAY_MS)) {
                    self.delay_ms.store(ms);
                }
            }
            StripChange::Width(percent) => {
                if let Some(percent) = clamped(percent, WIDTH_RANGE) {
                    self.width.store(percent);
                }
            }
        }
    }

    /// Sets everything at once.
    pub fn set_all(&self, settings: StripSettings) {
        self.set(StripChange::Polarity(settings.polarity));
        self.set(StripChange::Hpf(settings.hpf_hz));
        self.set(StripChange::Lpf(settings.lpf_hz));
        self.set(StripChange::Delay(settings.delay_ms));
        self.set(StripChange::Width(settings.width));
    }

    /// What it is set to now.
    #[must_use]
    pub fn settings(&self) -> StripSettings {
        StripSettings {
            polarity: self.polarity.load(Ordering::Relaxed),
            hpf_hz: self.hpf_hz.load(),
            lpf_hz: self.lpf_hz.load(),
            delay_ms: self.delay_ms.load(),
            width: self.width.load(),
        }
    }
}

/// A second-order filter (RBJ cookbook, Q 0.707) for one channel.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    /// Makes it a high-pass at `hz`, keeping its state.
    pub(crate) fn high_pass(&mut self, hz: f32, rate: f32) {
        let (cos, alpha) = Self::angle(hz, rate);
        let a0 = 1.0 + alpha;
        self.b0 = f32::midpoint(1.0, cos) / a0;
        self.b1 = -(1.0 + cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// Makes it a low-pass at `hz`, keeping its state.
    pub(crate) fn low_pass(&mut self, hz: f32, rate: f32) {
        let (cos, alpha) = Self::angle(hz, rate);
        let a0 = 1.0 + alpha;
        self.b0 = (1.0 - cos) / 2.0 / a0;
        self.b1 = (1.0 - cos) / a0;
        self.b2 = self.b0;
        self.a1 = -2.0 * cos / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    /// The cosine of the corner and alpha at Q 0.707; a corner above Nyquist is held below it.
    fn angle(hz: f32, rate: f32) -> (f32, f32) {
        let hz = hz.min(rate * 0.49);
        let w = std::f32::consts::TAU * hz / rate;
        (w.cos(), w.sin() / (2.0 * std::f32::consts::FRAC_1_SQRT_2))
    }

    /// Forgets what came before.
    pub(crate) fn start_over(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }

    pub(crate) fn next(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = if y.is_finite() { y } else { 0.0 };
        self.y1
    }
}

/// The channel strip of one input, in its capture callback.
#[derive(Debug)]
pub struct ChannelStrip {
    rate: f32,
    channels: usize,
    high: Vec<Biquad>,
    low: Vec<Biquad>,
    seen: Option<u32>,
    high_on: bool,
    low_on: bool,
    /// The last [`MAX_DELAY_MS`] of the input, always written, so a delay that is switched on
    /// plays what really came before.
    line: Vec<f32>,
    /// The frame `line` is written at next.
    at: usize,
}

impl ChannelStrip {
    /// The strip of an input of `channels` at `rate`.
    ///
    /// # Panics
    ///
    /// When `channels` or `rate` is zero.
    #[must_use]
    pub fn new(channels: usize, rate: u32) -> Self {
        assert!(
            channels > 0 && rate > 0,
            "a strip needs channels and a rate"
        );
        #[allow(clippy::cast_precision_loss)]
        let rate_f = rate as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let frames = (MAX_DELAY_MS / 1000.0 * rate_f).ceil() as usize + 1;
        Self {
            rate: rate_f,
            channels,
            high: vec![Biquad::default(); channels],
            low: vec![Biquad::default(); channels],
            seen: None,
            high_on: false,
            low_on: false,
            line: vec![0.0; frames * channels],
            at: 0,
        }
    }

    /// Lets the filters start from rest and forgets the delay line, as after a new stream.
    pub fn start_over(&mut self) {
        self.high.iter_mut().for_each(Biquad::start_over);
        self.low.iter_mut().for_each(Biquad::start_over);
        self.line.fill(0.0);
        self.at = 0;
    }

    /// Works `block` (interleaved, whole frames of the channels it was made for) in place as
    /// `controls` say.
    pub fn process(&mut self, controls: &StripControls, block: &mut [f32]) {
        let ch = self.channels;
        let settings = controls.settings();
        if settings.polarity {
            for sample in block.iter_mut() {
                *sample = -*sample;
            }
        }
        self.filter(controls, block);
        self.delay(&settings, block);
        if ch == 2 && (settings.width - 100.0).abs() > 1e-3 {
            let side = settings.width / 100.0;
            for frame in block.as_chunks_mut::<2>().0 {
                let mid = f32::midpoint(frame[0], frame[1]);
                let half = (frame[0] - frame[1]) / 2.0 * side;
                frame[0] = mid + half;
                frame[1] = mid - half;
            }
        }
    }

    fn filter(&mut self, controls: &StripControls, block: &mut [f32]) {
        let ch = self.channels;
        // The generation first, the corners after it: a corner read before would be one older
        // than the generation it is remembered with, and its change lost until the next one.
        let generation = controls.generation.load(Ordering::Acquire);
        if self.seen != Some(generation) {
            let (hpf_hz, lpf_hz) = (controls.hpf_hz.load(), controls.lpf_hz.load());
            let high_on = hpf_hz > HPF_RANGE_HZ.0;
            let low_on = lpf_hz < LPF_RANGE_HZ.1;
            for filter in &mut self.high {
                filter.high_pass(hpf_hz, self.rate);
                if !self.high_on {
                    filter.start_over();
                }
            }
            for filter in &mut self.low {
                filter.low_pass(lpf_hz, self.rate);
                if !self.low_on {
                    filter.start_over();
                }
            }
            (self.high_on, self.low_on) = (high_on, low_on);
            self.seen = Some(generation);
        }
        if self.high_on {
            for (index, sample) in block.iter_mut().enumerate() {
                *sample = self.high[index % ch].next(*sample);
            }
        }
        if self.low_on {
            for (index, sample) in block.iter_mut().enumerate() {
                *sample = self.low[index % ch].next(*sample);
            }
        }
    }

    fn delay(&mut self, settings: &StripSettings, block: &mut [f32]) {
        let ch = self.channels;
        let frames = self.line.len() / ch;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let behind = ((settings.delay_ms / 1000.0 * self.rate).round() as usize).min(frames - 1);
        for frame in block.chunks_exact_mut(ch) {
            let write = self.at * ch;
            self.line[write..write + ch].copy_from_slice(frame);
            if behind > 0 {
                let read = (self.at + frames - behind) % frames * ch;
                frame.copy_from_slice(&self.line[read..read + ch]);
            }
            self.at = (self.at + 1) % frames;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, frames: usize, channels: usize) -> Vec<f32> {
        let step = std::f32::consts::TAU * hz / 48_000.0;
        #[allow(clippy::cast_precision_loss)]
        (0..frames)
            .flat_map(|n| std::iter::repeat_n(0.5 * (step * n as f32).sin(), channels))
            .collect()
    }

    fn peak(out: &[f32]) -> f32 {
        out.iter().fold(0.0, |m: f32, s| m.max(s.abs()))
    }

    /// `input` through a strip in callbacks of 480 frames; the last half.
    fn run(c: &StripControls, input: &[f32], channels: usize) -> Vec<f32> {
        let mut strip = ChannelStrip::new(channels, 48_000);
        let mut out = Vec::new();
        for callback in input.chunks(480 * channels) {
            let mut buf = callback.to_vec();
            strip.process(c, &mut buf);
            out.extend(buf);
        }
        out[out.len() / 2..].to_vec()
    }

    #[test]
    fn neutral_it_passes_untouched() {
        let c = StripControls::default();
        let input = sine(1000.0, 9600, 2);
        assert_eq!(run(&c, &input, 2), input[input.len() / 2..]);
        assert!(c.settings().is_neutral());
    }

    #[test]
    fn polarity_turns_the_signal_over() {
        let c = StripControls::default();
        c.set(StripChange::Polarity(true));
        let input = sine(1000.0, 9600, 1);
        let out = run(&c, &input, 1);
        for (o, i) in out.iter().zip(&input[input.len() / 2..]) {
            assert!((o + i).abs() < 1e-6);
        }
    }

    #[test]
    fn the_filters_take_away_what_is_outside_their_corners() {
        let c = StripControls::default();
        c.set(StripChange::Hpf(200.0));
        assert!(
            peak(&run(&c, &sine(40.0, 48_000, 1), 1)) < 0.05,
            "bass gone"
        );
        assert!(
            peak(&run(&c, &sine(2000.0, 48_000, 1), 1)) > 0.48,
            "mids stay"
        );
        c.set(StripChange::Hpf(HPF_RANGE_HZ.0));
        c.set(StripChange::Lpf(2000.0));
        assert!(
            peak(&run(&c, &sine(12_000.0, 48_000, 1), 1)) < 0.05,
            "highs gone"
        );
        assert!(
            peak(&run(&c, &sine(200.0, 48_000, 1), 1)) > 0.48,
            "lows stay"
        );
    }

    #[test]
    fn the_delay_holds_the_sound_back() {
        let c = StripControls::default();
        c.set(StripChange::Delay(10.0)); // 480 frames
        let mut strip = ChannelStrip::new(1, 48_000);
        let mut click = vec![0.0; 960];
        click[3] = 1.0;
        strip.process(&c, &mut click);
        assert_eq!(click.iter().position(|&s| s > 0.5), Some(483));
        // The longest it can is held.
        c.set(StripChange::Delay(10_000.0));
        assert_eq!(c.settings().delay_ms, MAX_DELAY_MS);
    }

    #[test]
    fn width_zero_is_mono_and_a_mono_input_ignores_it() {
        let c = StripControls::default();
        c.set(StripChange::Width(0.0));
        let mut stereo = vec![1.0, 0.0, 0.2, 0.6];
        ChannelStrip::new(2, 48_000).process(&c, &mut stereo);
        assert_eq!(stereo, [0.5, 0.5, 0.4, 0.4]);
        let mut mono = vec![1.0, 0.0];
        ChannelStrip::new(1, 48_000).process(&c, &mut mono);
        assert_eq!(mono, [1.0, 0.0]);
    }

    #[test]
    fn changes_are_checked_and_it_does_not_allocate() {
        assert!(StripChange::Hpf(10.0).checked().is_err());
        assert!(StripChange::Lpf(500.0).checked().is_err());
        assert!(StripChange::Delay(-1.0).checked().is_err());
        assert!(StripChange::Width(250.0).checked().is_err());
        assert!(StripChange::Width(150.0).checked().is_ok());
        let c = StripControls::default();
        let mut strip = ChannelStrip::new(2, 48_000);
        let mut out = vec![0.3; 960];
        assert_no_alloc::assert_no_alloc(|| {
            c.set(StripChange::Hpf(100.0));
            c.set(StripChange::Delay(5.0));
            c.set(StripChange::Width(50.0));
            strip.process(&c, &mut out);
        });
    }
}
