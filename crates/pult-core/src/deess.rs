//! The de-esser of an input: it turns the band of the sibilants down while they are loud, and
//! leaves everything below it alone.
//!
//! A compressor behind a sidechain high-pass ([`crate::sidechain`]) hears the sibilants only, but
//! turns the whole sound down at them. The de-esser splits the sound at one corner instead, with
//! a Linkwitz-Riley filter of the fourth order as in the multiband compressor
//! ([`crate::multiband`]): what is above the corner goes through a fast compressor of its own,
//! what is below passes, and both are added again. Below its threshold nothing but the phase
//! changes.
//!
//! It has two settings: the corner in Hz and the threshold in dB, the level of the upper band
//! above which it turns down. A threshold at the upper end of its range ([`THRESHOLD_DB`]) is
//! off, and off it does not run at all. Coming on or going off it blends over [`BLEND_MS`]
//! between the sound and the split sound, since the two differ in phase and a hard change
//! would click.
//!
//! It sits behind the compressor of the input (which makes sibilants louder) and before the
//! equalizer when that comes behind the dynamics; it adds no delay.

use std::fmt::Write as _;

use serde::{Deserialize, Serialize};

use crate::dynamics::{Compressor, CompressorControls, CompressorParams};
use crate::level::AtomicF32;
use crate::multiband::Pair;

/// The corner of a de-esser in Hz: above it are the sibilants.
pub const FREQ_HZ: (f32, f32) = (2000.0, 12_000.0);
/// The threshold of a de-esser in dB; its upper end is off.
pub const THRESHOLD_DB: (f32, f32) = (-60.0, 0.0);
/// Milliseconds it takes to come on or go off.
pub const BLEND_MS: f32 = 5.0;
/// How hard it turns the band down: this many dB above the threshold become one.
const RATIO: f32 = 6.0;
/// Its times: fast enough for a sibilant, slow enough not to follow the wave.
const ATTACK_MS: f32 = 0.5;
const RELEASE_MS: f32 = 60.0;

/// The settings of a de-esser, as a view, a profile and the protocol carry them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DeessSettings {
    /// The level of the upper band in dB above which it turns down; [`THRESHOLD_DB`]`.1` is off.
    pub threshold_db: f32,
    /// The corner in Hz.
    pub freq_hz: f32,
}

impl Default for DeessSettings {
    fn default() -> Self {
        Self {
            threshold_db: THRESHOLD_DB.1,
            freq_hz: 6000.0,
        }
    }
}

impl DeessSettings {
    /// Whether it does anything.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.threshold_db < THRESHOLD_DB.1
    }

    /// Makes one change, unchecked ([`DeessChange::checked`] checks).
    pub fn apply(&mut self, change: DeessChange) {
        match change {
            DeessChange::Threshold(db) => self.threshold_db = db,
            DeessChange::Freq(hz) => self.freq_hz = hz,
        }
    }

    /// Every setting, checked.
    ///
    /// # Errors
    ///
    /// The message of the first one outside its range.
    pub fn checked(self) -> Result<Self, String> {
        DeessChange::Threshold(self.threshold_db).checked()?;
        DeessChange::Freq(self.freq_hz).checked()?;
        Ok(self)
    }

    /// What `show` says of it behind the rest of an input: nothing while it is off, else
    /// `  deess -30 dB above 6000 Hz`.
    #[must_use]
    pub fn summary(&self) -> String {
        let mut said = String::new();
        if self.is_on() {
            let _ = write!(
                said,
                "  deess {:.0} dB above {:.0} Hz",
                self.threshold_db, self.freq_hz
            );
        }
        said
    }
}

/// Which setting of a de-esser an address or a control names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DeessParam {
    /// Its threshold in dB: `input/<n>/deess/threshold`.
    Threshold,
    /// Its corner in Hz: `input/<n>/deess/freq`.
    Freq,
}

impl DeessParam {
    /// All of them, in the order of the addresses.
    pub const ALL: [Self; 2] = [Self::Threshold, Self::Freq];

    /// The words of its address behind `input/<n>/`.
    #[must_use]
    pub fn address(self) -> &'static str {
        match self {
            Self::Threshold => "deess/threshold",
            Self::Freq => "deess/freq",
        }
    }

    /// The setting those words name.
    #[must_use]
    pub fn from_address(words: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|param| param.address() == words)
    }

    /// The range of its numbers.
    #[must_use]
    pub fn range(self) -> (f32, f32) {
        match self {
            Self::Threshold => THRESHOLD_DB,
            Self::Freq => FREQ_HZ,
        }
    }

    /// What it is called for assistive technology.
    #[must_use]
    pub fn title(self) -> &'static str {
        match self {
            Self::Threshold => "de-esser threshold",
            Self::Freq => "de-esser frequency",
        }
    }

    /// The change that sets it to `number`.
    #[must_use]
    pub fn change(self, number: f32) -> DeessChange {
        match self {
            Self::Threshold => DeessChange::Threshold(number),
            Self::Freq => DeessChange::Freq(number),
        }
    }

    /// What `settings` has for it, as the change that would set it.
    #[must_use]
    pub fn read(self, settings: &DeessSettings) -> DeessChange {
        match self {
            Self::Threshold => DeessChange::Threshold(settings.threshold_db),
            Self::Freq => DeessChange::Freq(settings.freq_hz),
        }
    }
}

/// One change of a de-esser.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DeessChange {
    /// The threshold in dB; the upper end of [`THRESHOLD_DB`] switches it off.
    Threshold(f32),
    /// The corner in Hz.
    Freq(f32),
}

impl DeessChange {
    /// Which setting it changes.
    #[must_use]
    pub fn param(self) -> DeessParam {
        match self {
            Self::Threshold(_) => DeessParam::Threshold,
            Self::Freq(_) => DeessParam::Freq,
        }
    }

    /// Its value as a number.
    #[must_use]
    pub fn number(self) -> f32 {
        match self {
            Self::Threshold(n) | Self::Freq(n) => n,
        }
    }

    /// What it sets, as a reply says it: `deess -30 dB`, `deess off`, `deess above 6000 Hz`.
    #[must_use]
    pub fn said(self) -> String {
        match self {
            Self::Threshold(db) if db >= THRESHOLD_DB.1 => "deess off".to_owned(),
            Self::Threshold(db) => format!("deess {db:.0} dB"),
            Self::Freq(hz) => format!("deess above {hz:.0} Hz"),
        }
    }

    /// The change when its number is inside its range: nothing is clamped, as with a gain.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise.
    pub fn checked(self) -> Result<Self, String> {
        let (what, unit) = match self {
            Self::Threshold(_) => ("threshold", "dB"),
            Self::Freq(_) => ("frequency", "Hz"),
        };
        let (low, high) = self.param().range();
        let value = self.number();
        if (low..=high).contains(&value) {
            Ok(self)
        } else {
            Err(format!(
                "the {what} of a de-esser must be {low} to {high} {unit}, not {value}"
            ))
        }
    }
}

/// The de-esser of one input as the control thread sets it; off at first.
#[derive(Debug)]
pub struct DeessControls {
    threshold_db: AtomicF32,
    freq_hz: AtomicF32,
}

impl Default for DeessControls {
    fn default() -> Self {
        let settings = DeessSettings::default();
        Self {
            threshold_db: AtomicF32::new(settings.threshold_db),
            freq_hz: AtomicF32::new(settings.freq_hz),
        }
    }
}

impl DeessControls {
    /// Sets everything at once; a number outside its range is clamped (check before with
    /// [`DeessSettings::checked`]), one that is no number leaves its setting as it is.
    pub fn set_all(&self, settings: DeessSettings) {
        if settings.threshold_db.is_finite() {
            self.threshold_db
                .store(settings.threshold_db.clamp(THRESHOLD_DB.0, THRESHOLD_DB.1));
        }
        if settings.freq_hz.is_finite() {
            self.freq_hz
                .store(settings.freq_hz.clamp(FREQ_HZ.0, FREQ_HZ.1));
        }
    }

    /// What it is set to now.
    #[must_use]
    pub fn settings(&self) -> DeessSettings {
        DeessSettings {
            threshold_db: self.threshold_db.load(),
            freq_hz: self.freq_hz.load(),
        }
    }
}

/// The two halves of one channel: below the corner and above it.
#[derive(Debug, Clone, Copy, Default)]
struct Split {
    low: Pair,
    high: Pair,
}

/// The de-esser inside the stage of one input.
#[derive(Debug)]
pub struct Deesser {
    rate: f32,
    channels: usize,
    settings: DeessSettings,
    splits: Vec<Split>,
    comp: Compressor,
    /// The compressor of the upper band is set by this de-esser alone.
    comp_controls: CompressorControls,
    /// How much of the split sound is in what goes out: 0 while off, 1 while on.
    blend: f32,
    /// What the blend moves per frame.
    pace: f32,
    /// Whether the filters ran in the last block: after a rest they start from rest.
    ran: bool,
    low: Vec<f32>,
    high: Vec<f32>,
}

impl Deesser {
    /// The de-esser of an input of `channels` at `rate`, for blocks of at most `frames` frames;
    /// off.
    #[must_use]
    pub fn new(channels: usize, rate: u32, frames: usize) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate_hz = rate.max(1) as f32;
        let mut deesser = Self {
            rate: rate_hz,
            channels,
            settings: DeessSettings::default(),
            splits: vec![Split::default(); channels],
            comp: Compressor::new(rate),
            comp_controls: CompressorControls::default(),
            blend: 0.0,
            pace: 1000.0 / (BLEND_MS * rate_hz),
            ran: false,
            low: vec![0.0; frames * channels],
            high: vec![0.0; frames * channels],
        };
        deesser.configure(DeessSettings::default());
        deesser
    }

    fn configure(&mut self, settings: DeessSettings) {
        self.settings = settings;
        for split in &mut self.splits {
            split.low.low_pass(settings.freq_hz, self.rate);
            split.high.high_pass(settings.freq_hz, self.rate);
        }
        self.comp_controls.set(CompressorParams {
            threshold_db: settings.threshold_db,
            ratio: RATIO,
            attack_ms: ATTACK_MS,
            release_ms: RELEASE_MS,
            makeup_db: 0.0,
        });
        self.comp_controls.switch(settings.is_on());
    }

    /// Forgets what it heard, as after a new stream; what is on is on at once.
    pub fn start_over(&mut self) {
        self.ran = false;
        self.blend = if self.settings.is_on() { 1.0 } else { 0.0 };
        self.comp.start_over();
    }

    /// How many dB it takes from the upper band now; 0 while it does not run.
    #[must_use]
    pub fn reduction_db(&self) -> f32 {
        if self.ran {
            self.comp.reduction_db()
        } else {
            0.0
        }
    }

    /// De-esses `block` (interleaved, whole frames) in place as `controls` say. A block longer
    /// than it was made for passes untouched.
    pub fn process(&mut self, controls: &DeessControls, block: &mut [f32]) {
        let settings = controls.settings();
        if settings != self.settings {
            self.configure(settings);
        }
        let goal = if settings.is_on() { 1.0 } else { 0.0 };
        let n = block.len();
        #[allow(clippy::float_cmp)] // Exactly: the blend is set to its goal when it arrives.
        let resting = goal == 0.0 && self.blend == 0.0;
        if resting || self.channels == 0 || n > self.low.len() {
            self.ran = false;
            return;
        }
        if !self.ran {
            for split in &mut self.splits {
                split.low.start_over();
                split.high.start_over();
            }
            self.comp.start_over();
            self.ran = true;
        }
        let (low, high) = (&mut self.low[..n], &mut self.high[..n]);
        for (index, sample) in block.iter().enumerate() {
            let split = &mut self.splits[index % self.channels];
            low[index] = split.low.next(*sample);
            high[index] = split.high.next(*sample);
        }
        self.comp.process(&self.comp_controls, high, self.channels);
        for ((frame, low), high) in block
            .chunks_exact_mut(self.channels)
            .zip(low.chunks_exact(self.channels))
            .zip(high.chunks_exact(self.channels))
        {
            if self.blend < goal {
                self.blend = (self.blend + self.pace).min(goal);
            } else if self.blend > goal {
                self.blend = (self.blend - self.pace).max(goal);
            }
            for ((sample, low), high) in frame.iter_mut().zip(low).zip(high) {
                *sample += self.blend * (low + high - *sample);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const BLOCK: usize = 480;

    fn sine(hz: f32, amplitude: f32, frames: usize, start: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = (start + i) as f32 / RATE as f32;
                (std::f32::consts::TAU * hz * t).sin() * amplitude
            })
            .collect()
    }

    fn peak(data: &[f32]) -> f32 {
        data.iter().fold(0.0, |most, sample| most.max(sample.abs()))
    }

    fn on(threshold_db: f32, freq_hz: f32) -> DeessControls {
        let controls = DeessControls::default();
        controls.set_all(DeessSettings {
            threshold_db,
            freq_hz,
        });
        controls
    }

    /// `blocks` blocks of `signal(start)` through; the peak of the last one.
    fn settled(
        deesser: &mut Deesser,
        controls: &DeessControls,
        blocks: usize,
        signal: impl Fn(usize) -> Vec<f32>,
    ) -> f32 {
        let mut last = 0.0;
        for block in 0..blocks {
            let mut data = signal(block * BLOCK);
            deesser.process(controls, &mut data);
            last = peak(&data);
        }
        last
    }

    #[test]
    fn off_it_changes_nothing_and_does_not_run() {
        let controls = DeessControls::default();
        let mut deesser = Deesser::new(1, RATE, BLOCK);
        let before = sine(8000.0, 0.9, BLOCK, 0);
        let mut data = before.clone();
        deesser.process(&controls, &mut data);
        assert_eq!(data, before);
        assert!(deesser.reduction_db().abs() < f32::EPSILON);
        assert!(!controls.settings().is_on());
    }

    #[test]
    fn a_loud_sibilant_is_turned_down_and_a_voice_below_the_corner_is_not() {
        let controls = on(-30.0, 5000.0);
        let mut deesser = Deesser::new(1, RATE, BLOCK);
        let hiss = settled(&mut deesser, &controls, 40, |at| {
            sine(8000.0, 0.5, BLOCK, at)
        });
        // -6 dBFS is 24 dB above the threshold, of which 20 dB would go at 6:1 for a steady
        // level; of a wave the detector sees every sample, so it settles some dB short of that.
        assert!(hiss < 0.5 * 0.25, "the sibilant is turned down: {hiss}");
        assert!(deesser.reduction_db() > 12.0, "{}", deesser.reduction_db());
        let mut deesser = Deesser::new(1, RATE, BLOCK);
        let voice = settled(&mut deesser, &controls, 40, |at| {
            sine(300.0, 0.5, BLOCK, at)
        });
        assert!((voice - 0.5).abs() < 0.01, "the voice stays: {voice}");
        assert!(deesser.reduction_db() < 0.1, "{}", deesser.reduction_db());
    }

    #[test]
    fn only_the_upper_band_goes_down_when_both_sound() {
        let controls = on(-30.0, 5000.0);
        let mut deesser = Deesser::new(1, RATE, BLOCK);
        let both = |at: usize| -> Vec<f32> {
            sine(300.0, 0.4, BLOCK, at)
                .iter()
                .zip(sine(8000.0, 0.4, BLOCK, at))
                .map(|(low, high)| low + high)
                .collect()
        };
        let together = settled(&mut deesser, &controls, 40, both);
        // The voice alone would peak at 0.4; a compressor on all of it would take it far below.
        assert!(together > 0.38, "the voice is still there: {together}");
        assert!(together < 0.5, "the sibilant is not: {together}");
    }

    #[test]
    fn below_its_threshold_the_level_stays_as_it_is() {
        let controls = on(-10.0, 5000.0);
        for hz in [300.0, 5000.0, 9000.0] {
            let mut deesser = Deesser::new(1, RATE, BLOCK);
            // -26 dBFS, well below the threshold: the split alone must be flat.
            let out = settled(&mut deesser, &controls, 40, |at| sine(hz, 0.05, BLOCK, at));
            assert!((out - 0.05).abs() < 0.001, "{hz} Hz: {out}");
        }
    }

    #[test]
    fn coming_on_and_going_off_it_makes_no_step() {
        let controls = on(-10.0, 5000.0);
        let off = DeessControls::default();
        let mut deesser = Deesser::new(1, RATE, BLOCK);
        // A tone at the corner, where the split turns the phase most.
        let steepest = |data: &[f32], last: &mut f32| {
            let mut most = 0.0_f32;
            for sample in data {
                most = most.max((sample - *last).abs());
                *last = *sample;
            }
            most
        };
        let mut last = 0.0;
        let mut most = 0.0_f32;
        for block in 0..30 {
            let mut data = sine(5000.0, 0.05, BLOCK, block * BLOCK);
            let controls = if (10..20).contains(&block) {
                &controls
            } else {
                &off
            };
            deesser.process(controls, &mut data);
            most = most.max(steepest(&data, &mut last));
        }
        // The steepest step of the tone itself is 0.05 * 2 pi * 5000 / 48000.
        let own = 0.05 * std::f32::consts::TAU * 5000.0 / 48_000.0;
        assert!(most < own * 1.3, "a step of {most}, the tone has {own}");
        // And it rests again once it is off and blended out.
        assert!(deesser.reduction_db().abs() < f32::EPSILON);
    }

    #[test]
    fn two_channels_keep_their_own_sound_and_go_down_together() {
        let controls = on(-30.0, 5000.0);
        let mut deesser = Deesser::new(2, RATE, BLOCK);
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;
        for block in 0..40 {
            let hiss = sine(8000.0, 0.5, BLOCK, block * BLOCK);
            let voice = sine(300.0, 0.3, BLOCK, block * BLOCK);
            let mut data: Vec<f32> = hiss
                .iter()
                .zip(&voice)
                .flat_map(|(h, v)| [*h, *v])
                .collect();
            deesser.process(&controls, &mut data);
            left = data.iter().step_by(2).fold(0.0, |m, s| m.max(s.abs()));
            right = data
                .iter()
                .skip(1)
                .step_by(2)
                .fold(0.0, |m, s| m.max(s.abs()));
        }
        assert!(left < 0.125, "the sibilant on the left: {left}");
        assert!(
            (right - 0.3).abs() < 0.01,
            "the voice on the right: {right}"
        );
    }

    #[test]
    fn nothing_allocates_and_a_longer_block_passes() {
        let controls = on(-30.0, 5000.0);
        let mut deesser = Deesser::new(2, RATE, BLOCK);
        let mut data = vec![0.25; BLOCK * 2];
        assert_no_alloc::assert_no_alloc(|| {
            deesser.process(&controls, &mut data);
            controls.set_all(DeessSettings {
                threshold_db: -20.0,
                freq_hz: 7000.0,
            });
            deesser.process(&controls, &mut data);
        });
        let before = vec![0.25; (BLOCK + 1) * 2];
        let mut long = before.clone();
        deesser.process(&controls, &mut long);
        assert_eq!(long, before);
    }

    #[test]
    fn settings_changes_and_addresses() {
        let mut settings = DeessSettings::default();
        assert_eq!(settings.summary(), "");
        settings.apply(DeessChange::Threshold(-30.0));
        settings.apply(DeessChange::Freq(7000.0));
        assert_eq!(settings.summary(), "  deess -30 dB above 7000 Hz");
        assert!(settings.checked().is_ok());
        assert!(DeessChange::Threshold(1.0).checked().is_err());
        assert!(DeessChange::Threshold(f32::NAN).checked().is_err());
        assert!(
            DeessChange::Freq(100.0)
                .checked()
                .unwrap_err()
                .contains("2000 to 12000 Hz")
        );
        for param in DeessParam::ALL {
            assert_eq!(DeessParam::from_address(param.address()), Some(param));
            let change = param.read(&settings);
            assert_eq!(change.param(), param);
            assert_eq!(param.change(change.number()), change);
        }
        assert_eq!(DeessChange::Threshold(0.0).said(), "deess off");
        assert_eq!(DeessChange::Threshold(-24.0).said(), "deess -24 dB");
        assert_eq!(DeessChange::Freq(6000.0).said(), "deess above 6000 Hz");
        // No number leaves a setting as it is; one outside is clamped.
        let controls = DeessControls::default();
        controls.set_all(DeessSettings {
            threshold_db: f32::NAN,
            freq_hz: 50_000.0,
        });
        let now = controls.settings();
        assert!(!now.is_on());
        assert!((now.freq_hz - FREQ_HZ.1).abs() < f32::EPSILON);
    }
}
