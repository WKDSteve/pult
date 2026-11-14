//! The multiband compressor of a bus: the sum is split into three bands, each has a compressor
//! of its own with one knob, and the bands are added again. A loud bass then no longer turns the
//! voices down, as it does in a compressor that sees the whole sum.
//!
//! The bands are split by Linkwitz-Riley filters of the fourth order (two Butterworth filters
//! in a row): low-pass and high-pass of one corner add up to a flat level, so with every knob at
//! 0 nothing but the phase would change, and with every knob at 0 it does not even run. The low
//! band passes both filters of the upper corner too, which keeps its phase with the two others.
//!
//! It sits behind the bus compressor and before the fader, works in pieces of
//! [`PIECE_FRAMES`] frames through buffers made in [`Multiband::new`] for up to
//! [`MAX_CHANNELS`] channels, and allocates nothing in a callback. An output with more channels
//! passes untouched.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::buscomp::{MAX_CHANNELS, PIECE_FRAMES};
use crate::dynamics::{Compressor, CompressorControls};
use crate::level::{AtomicF32, PeakMeter};
use crate::strip::Biquad;
use crate::view::check_amount;

/// The bands, from the bottom.
pub const BANDS: usize = 3;
/// What the bands are called, in commands, addresses and the profile.
pub const BAND_WORDS: [&str; BANDS] = ["low", "mid", "high"];
/// Range of the corner between the low and the middle band in Hz.
pub const LOW_SPLIT_HZ: (f32, f32) = (60.0, 1000.0);
/// Range of the corner between the middle and the high band in Hz.
pub const HIGH_SPLIT_HZ: (f32, f32) = (1000.0, 10_000.0);

/// The settings of a multiband compressor, as a view, a profile and the protocol carry them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct MultibandSettings {
    /// The knob of each band, 0 (off) to 10, from the bottom.
    pub bands: [f32; BANDS],
    /// The corner between the low and the middle band in Hz.
    pub low_hz: f32,
    /// The corner between the middle and the high band in Hz.
    pub high_hz: f32,
}

impl Default for MultibandSettings {
    fn default() -> Self {
        Self {
            bands: [0.0; BANDS],
            low_hz: 200.0,
            high_hz: 2000.0,
        }
    }
}

impl MultibandSettings {
    /// Makes one change, unchecked ([`MultibandChange::checked`] checks).
    pub fn apply(&mut self, change: MultibandChange) {
        match change {
            MultibandChange::Band(band, amount) => {
                if let Some(knob) = self.bands.get_mut(band) {
                    *knob = amount;
                }
            }
            MultibandChange::LowSplit(hz) => self.low_hz = hz,
            MultibandChange::HighSplit(hz) => self.high_hz = hz,
        }
    }

    /// Every setting, checked.
    ///
    /// # Errors
    ///
    /// The message of the first one outside its range.
    pub fn checked(self) -> Result<Self, String> {
        for (band, amount) in self.bands.into_iter().enumerate() {
            MultibandChange::Band(band, amount).checked()?;
        }
        MultibandChange::LowSplit(self.low_hz).checked()?;
        MultibandChange::HighSplit(self.high_hz).checked()?;
        Ok(self)
    }

    /// Whether any band compresses.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.bands.iter().any(|&amount| amount > 0.0)
    }
}

/// One change of a multiband compressor.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MultibandChange {
    /// The knob of a band (from 0, the lowest), 0 (off) to 10.
    Band(usize, f32),
    /// The corner between the low and the middle band in Hz.
    LowSplit(f32),
    /// The corner between the middle and the high band in Hz.
    HighSplit(f32),
}

/// One setting of a multiband compressor, as an address names it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MultibandParam {
    /// The knob of a band, from 0 (the lowest).
    Band(usize),
    /// The corner between the low and the middle band.
    LowSplit,
    /// The corner between the middle and the high band.
    HighSplit,
}

impl MultibandParam {
    /// The setting a word of an address names: `low`, `mid`, `high`, `xlow`, `xhigh`.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        match word {
            "xlow" => Some(Self::LowSplit),
            "xhigh" => Some(Self::HighSplit),
            band => BAND_WORDS.iter().position(|&w| w == band).map(Self::Band),
        }
    }

    /// Its word in an address.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::Band(band) => BAND_WORDS.get(band).copied().unwrap_or("low"),
            Self::LowSplit => "xlow",
            Self::HighSplit => "xhigh",
        }
    }

    /// What it is called for assistive technology.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Band(0) => "low band",
            Self::Band(1) => "middle band",
            Self::Band(_) => "high band",
            Self::LowSplit => "low split",
            Self::HighSplit => "high split",
        }
    }

    /// Its value in `settings`.
    #[must_use]
    pub fn read(self, settings: &MultibandSettings) -> f32 {
        match self {
            Self::Band(band) => settings.bands.get(band).copied().unwrap_or(0.0),
            Self::LowSplit => settings.low_hz,
            Self::HighSplit => settings.high_hz,
        }
    }

    /// The change that sets it to `number`.
    #[must_use]
    pub fn change(self, number: f32) -> MultibandChange {
        match self {
            Self::Band(band) => MultibandChange::Band(band, number),
            Self::LowSplit => MultibandChange::LowSplit(number),
            Self::HighSplit => MultibandChange::HighSplit(number),
        }
    }
}

impl MultibandChange {
    /// The setting it changes.
    #[must_use]
    pub fn param(self) -> MultibandParam {
        match self {
            Self::Band(band, _) => MultibandParam::Band(band),
            Self::LowSplit(_) => MultibandParam::LowSplit,
            Self::HighSplit(_) => MultibandParam::HighSplit,
        }
    }

    /// The number it sets.
    #[must_use]
    pub fn number(self) -> f32 {
        match self {
            Self::Band(_, number) | Self::LowSplit(number) | Self::HighSplit(number) => number,
        }
    }

    /// The change when its value is inside its range: nothing is clamped, as with a gain.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise, or that there is no such band.
    pub fn checked(self) -> Result<Self, String> {
        let (what, hz, (low, high)) = match self {
            Self::Band(band, amount) => {
                if band >= BANDS {
                    return Err(format!("a multiband compressor has {BANDS} bands"));
                }
                return check_amount("multiband", amount).map(|amount| Self::Band(band, amount));
            }
            Self::LowSplit(hz) => ("low", hz, LOW_SPLIT_HZ),
            Self::HighSplit(hz) => ("high", hz, HIGH_SPLIT_HZ),
        };
        if (low..=high).contains(&hz) {
            Ok(self)
        } else {
            Err(format!(
                "the {what} split of a multiband compressor must be {low} to {high} Hz, not {hz}"
            ))
        }
    }

    /// What it sets, as a reply says it: `multiband low 3.0`, `multiband mid off`,
    /// `multiband split low 200 Hz`.
    #[must_use]
    pub fn said(self) -> String {
        match self {
            Self::Band(band, amount) => {
                let word = BAND_WORDS.get(band).copied().unwrap_or("?");
                if amount > 0.0 {
                    format!("multiband {word} {amount:.1}")
                } else {
                    format!("multiband {word} off")
                }
            }
            Self::LowSplit(hz) => format!("multiband split low {hz:.0} Hz"),
            Self::HighSplit(hz) => format!("multiband split high {hz:.0} Hz"),
        }
    }
}

/// The multiband compressor of one bus as the control thread sets it.
#[derive(Debug)]
pub struct MultibandControls {
    bands: [CompressorControls; BANDS],
    low_hz: AtomicF32,
    high_hz: AtomicF32,
    /// The most each band took away since the desk last read it, in dB.
    pub reduction: [PeakMeter; BANDS],
    /// Changes of the corners, so the filters are computed again only when one moved.
    generation: AtomicU32,
}

impl Default for MultibandControls {
    fn default() -> Self {
        let settings = MultibandSettings::default();
        Self {
            bands: std::array::from_fn(|_| CompressorControls::default()),
            low_hz: AtomicF32::new(settings.low_hz),
            high_hz: AtomicF32::new(settings.high_hz),
            reduction: std::array::from_fn(|_| PeakMeter::default()),
            generation: AtomicU32::new(0),
        }
    }
}

impl MultibandControls {
    /// Makes one change; a corner outside its range is clamped (check before with
    /// [`MultibandChange::checked`]), one that is no number leaves the setting as it is.
    pub fn set(&self, change: MultibandChange) {
        let corner = |cell: &AtomicF32, hz: f32, (low, high): (f32, f32)| {
            if hz.is_finite() {
                cell.store(hz.clamp(low, high));
                self.generation.fetch_add(1, Ordering::Release);
            }
        };
        match change {
            MultibandChange::Band(band, amount) => {
                if let Some(controls) = self.bands.get(band) {
                    controls.set_simple(amount);
                }
            }
            MultibandChange::LowSplit(hz) => corner(&self.low_hz, hz, LOW_SPLIT_HZ),
            MultibandChange::HighSplit(hz) => corner(&self.high_hz, hz, HIGH_SPLIT_HZ),
        }
    }

    /// Sets everything at once.
    pub fn set_all(&self, settings: MultibandSettings) {
        for (band, amount) in settings.bands.into_iter().enumerate() {
            self.set(MultibandChange::Band(band, amount));
        }
        self.set(MultibandChange::LowSplit(settings.low_hz));
        self.set(MultibandChange::HighSplit(settings.high_hz));
    }

    fn is_on(&self) -> bool {
        self.bands.iter().any(CompressorControls::is_on)
    }
}

/// A Linkwitz-Riley filter of the fourth order for one channel: two Butterworth filters in a row.
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct Pair([Biquad; 2]);

impl Pair {
    pub(crate) fn low_pass(&mut self, hz: f32, rate: f32) {
        for filter in &mut self.0 {
            filter.low_pass(hz, rate);
        }
    }

    pub(crate) fn high_pass(&mut self, hz: f32, rate: f32) {
        for filter in &mut self.0 {
            filter.high_pass(hz, rate);
        }
    }

    pub(crate) fn next(&mut self, x: f32) -> f32 {
        let once = self.0[0].next(x);
        self.0[1].next(once)
    }

    pub(crate) fn start_over(&mut self) {
        self.0.iter_mut().for_each(Biquad::start_over);
    }
}

/// The filters that split one channel into its three bands.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    /// Below the lower corner, and what is above it.
    low: Pair,
    rest: Pair,
    /// The low band through both filters of the upper corner: together they only turn its phase,
    /// as the split of the two other bands turns theirs.
    low_below: Pair,
    low_above: Pair,
    /// The rest below the upper corner, and above it.
    mid: Pair,
    high: Pair,
}

impl Split {
    fn set(&mut self, low_hz: f32, high_hz: f32, rate: f32) {
        self.low.low_pass(low_hz, rate);
        self.rest.high_pass(low_hz, rate);
        self.low_below.low_pass(high_hz, rate);
        self.low_above.high_pass(high_hz, rate);
        self.mid.low_pass(high_hz, rate);
        self.high.high_pass(high_hz, rate);
    }

    fn next(&mut self, x: f32) -> [f32; BANDS] {
        let low = self.low.next(x);
        let rest = self.rest.next(x);
        [
            self.low_below.next(low) + self.low_above.next(low),
            self.mid.next(rest),
            self.high.next(rest),
        ]
    }

    fn start_over(&mut self) {
        for pair in [
            &mut self.low,
            &mut self.rest,
            &mut self.low_below,
            &mut self.low_above,
            &mut self.mid,
            &mut self.high,
        ] {
            pair.start_over();
        }
    }
}

/// The multiband compressor of one bus, in its output callback.
#[derive(Debug)]
pub struct Multiband {
    rate: f32,
    splits: [Split; MAX_CHANNELS],
    comps: [Compressor; BANDS],
    seen: Option<u32>,
    /// It ran in the last callback: filters that rested start from rest.
    ran: bool,
    bands: [Vec<f32>; BANDS],
}

impl Multiband {
    /// A multiband compressor at `rate` frames per second.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate_f = rate.max(1) as f32;
        Self {
            rate: rate_f,
            splits: [Split::default(); MAX_CHANNELS],
            comps: std::array::from_fn(|_| Compressor::new(rate)),
            seen: None,
            ran: false,
            bands: std::array::from_fn(|_| vec![0.0; PIECE_FRAMES * MAX_CHANNELS]),
        }
    }

    /// Compresses `out` (interleaved, `channels` per frame) in place as `controls` say.
    pub fn process(&mut self, controls: &MultibandControls, out: &mut [f32], channels: usize) {
        if !controls.is_on() || channels == 0 || channels > MAX_CHANNELS {
            self.ran = false;
            return;
        }
        // The generation first, the corners after it (see the channel strip).
        let generation = controls.generation.load(Ordering::Acquire);
        if self.seen != Some(generation) {
            let low_hz = controls.low_hz.load();
            let high_hz = controls.high_hz.load();
            for split in &mut self.splits {
                split.set(low_hz, high_hz, self.rate);
            }
            self.seen = Some(generation);
        }
        if !self.ran {
            self.splits.iter_mut().for_each(Split::start_over);
            self.ran = true;
        }
        let whole = out.len() / channels * channels;
        for piece in out[..whole].chunks_mut(PIECE_FRAMES * channels) {
            let n = piece.len();
            for (index, sample) in piece.iter().enumerate() {
                let parts = self.splits[index % channels].next(*sample);
                for (band, part) in self.bands.iter_mut().zip(parts) {
                    band[index] = part;
                }
            }
            piece.fill(0.0);
            for (index, band) in self.bands.iter_mut().enumerate() {
                let band = &mut band[..n];
                self.comps[index].process(&controls.bands[index], band, channels);
                controls.reduction[index].record(self.comps[index].reduction_db());
                for (sum, part) in piece.iter_mut().zip(band.iter()) {
                    *sum += *part;
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

    /// `signal` through in callbacks of 480 frames; the last half second.
    fn run(c: &MultibandControls, signal: &[f32]) -> Vec<f32> {
        let mut multiband = Multiband::new(48_000);
        let mut out = Vec::new();
        for callback in signal.chunks(480 * 2) {
            let mut buf = callback.to_vec();
            multiband.process(c, &mut buf, 2);
            out.extend(buf);
        }
        out[out.len() - 48_000..].to_vec()
    }

    /// A band that compresses hard, but so little that the level stays: the split runs.
    fn splitting() -> MultibandControls {
        let c = MultibandControls::default();
        c.bands[0].set(CompressorParams {
            threshold_db: 0.0,
            ratio: 1.0,
            makeup_db: 0.0,
            ..CompressorParams::default()
        });
        c.bands[0].switch(true);
        c
    }

    fn squeezing(band: usize) -> MultibandControls {
        let c = MultibandControls::default();
        c.bands[band].set(CompressorParams {
            threshold_db: -30.0,
            ratio: 8.0,
            makeup_db: 0.0,
            ..CompressorParams::default()
        });
        c.bands[band].switch(true);
        c
    }

    #[test]
    fn off_it_passes_untouched() {
        let c = MultibandControls::default();
        let input = sine(1000.0, 0.5, 48_000);
        assert_eq!(run(&c, &input), input[input.len() - 48_000..]);
    }

    #[test]
    fn the_bands_add_up_to_a_flat_level() {
        let c = splitting();
        for hz in [40.0, 200.0, 630.0, 2000.0, 9000.0] {
            let level = peak(&run(&c, &sine(hz, 0.5, 96_000)));
            assert!((level - 0.5).abs() < 0.01, "{hz} Hz comes out at {level}");
        }
    }

    #[test]
    fn a_band_compresses_only_what_is_in_it() {
        let c = squeezing(0);
        let bass = peak(&run(&c, &sine(60.0, 0.5, 96_000)));
        assert!(bass < 0.2, "the bass is squeezed: {bass}");
        assert!(c.reduction[0].take() > 6.0);
        let voice = peak(&run(&c, &sine(3000.0, 0.5, 96_000)));
        assert!(voice > 0.48, "the voice is left alone: {voice}");
        // The corner moves: 300 Hz is in the middle band, until the low band reaches up to it.
        let before = peak(&run(&c, &sine(300.0, 0.5, 96_000)));
        c.set(MultibandChange::LowSplit(900.0));
        let after = peak(&run(&c, &sine(300.0, 0.5, 96_000)));
        assert!(after < before * 0.6, "{after} against {before}");
    }

    #[test]
    fn changes_are_checked_said_and_applied() {
        assert!(MultibandChange::Band(3, 1.0).checked().is_err());
        assert!(MultibandChange::Band(0, 11.0).checked().is_err());
        assert!(MultibandChange::LowSplit(20.0).checked().is_err());
        assert!(MultibandChange::HighSplit(500.0).checked().is_err());
        assert_eq!(
            MultibandChange::Band(1, 3.0).said(),
            "multiband mid 3.0".to_owned()
        );
        assert_eq!(
            MultibandChange::Band(2, 0.0).said(),
            "multiband high off".to_owned()
        );
        let mut settings = MultibandSettings::default();
        assert!(!settings.is_on());
        settings.apply(MultibandChange::Band(1, 3.0));
        settings.apply(MultibandChange::HighSplit(4000.0));
        assert!(settings.is_on());
        assert_eq!(settings.checked(), Ok(settings));
        // Clamped where it is set without a check.
        let c = MultibandControls::default();
        c.set(MultibandChange::LowSplit(5.0));
        assert!((c.low_hz.load() - LOW_SPLIT_HZ.0).abs() < f32::EPSILON);
    }

    #[test]
    fn it_does_not_allocate_and_leaves_too_many_channels_alone() {
        let c = squeezing(1);
        let mut multiband = Multiband::new(48_000);
        let mut out = vec![0.4; 3000 * 2];
        assert_no_alloc::assert_no_alloc(|| {
            multiband.process(&c, &mut out, 2);
            c.set(MultibandChange::HighSplit(3000.0));
            multiband.process(&c, &mut out, 2);
        });
        let mut wide = vec![0.9; 9 * 100];
        multiband.process(&c, &mut wide, 9);
        assert!(wide.iter().all(|&s| s.to_bits() == 0.9_f32.to_bits()));
    }
}
