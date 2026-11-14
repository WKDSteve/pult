//! The sidechain of an input: what its gate and its compressor listen to.
//!
//! Without one they listen to the sound they work on. A sidechain changes that in two ways:
//!
//! * **Filters.** A high-pass and a low-pass in front of the detectors only: the sound itself
//!   stays as it is, but a compressor behind a high-pass no longer pumps with the bass, and
//!   one that hears only the top of a voice turns down at its sibilants.
//! * **A key.** Another input of the route: the gate opens and the compressor turns down by its
//!   level instead (a bass that only sounds with the kick drum, music that gives way to a
//!   voice with the curve of a compressor).
//!
//! # How the key travels
//!
//! As for ducking ([`crate::duck`]): the stage of every input stores its level once per block on
//! the [`KeyBoard`] of the route, and the stage of the keyed input reads it there, in its own
//! thread. Neither waits for the other; what the detectors see of the key is at most one
//! callback old, and one level per block (a gate keyed that way opens up to a block late). The
//! level is taken after the channel strip of the key and before its gate and compressor, so two
//! inputs can key each other without locking each other out. A muted key keys nothing, and one
//! whose thread stopped counts as silent after [`crate::duck::STALE_MS`].
//!
//! The filters work on the own sound only: of a key there is a level, no sound to filter.

use std::fmt::Write as _;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::duck::{KeyBoard, KeyWatch};
use crate::level::AtomicF32;
use crate::strip::Biquad;

/// The corner of the high-pass of a sidechain in Hz; the lower end is off.
pub const HPF_HZ: (f32, f32) = (20.0, 10_000.0);
/// The corner of the low-pass of a sidechain in Hz; the upper end is off.
pub const LPF_HZ: (f32, f32) = (100.0, 20_000.0);

/// The settings of a sidechain, as a view, a profile and the protocol carry them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SidechainSettings {
    /// The input gate and compressor listen to, by its number from 1 as in the addresses; 0 is
    /// the input itself.
    pub key: usize,
    /// The corner of the high-pass before the detectors in Hz; [`HPF_HZ`]`.0` is off.
    pub hpf_hz: f32,
    /// The corner of the low-pass before the detectors in Hz; [`LPF_HZ`]`.1` is off.
    pub lpf_hz: f32,
}

impl Default for SidechainSettings {
    fn default() -> Self {
        Self {
            key: 0,
            hpf_hz: HPF_HZ.0,
            lpf_hz: LPF_HZ.1,
        }
    }
}

impl SidechainSettings {
    /// Every setting as it starts: gate and compressor listen to what they work on.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Whether the high-pass does anything.
    #[must_use]
    pub fn high_on(&self) -> bool {
        self.hpf_hz > HPF_HZ.0
    }

    /// Whether the low-pass does anything.
    #[must_use]
    pub fn low_on(&self) -> bool {
        self.lpf_hz < LPF_HZ.1
    }

    /// Makes one change, unchecked ([`SidechainChange::checked`] checks).
    pub fn apply(&mut self, change: SidechainChange) {
        match change {
            SidechainChange::Key(key) => self.key = key,
            SidechainChange::Hpf(hz) => self.hpf_hz = hz,
            SidechainChange::Lpf(hz) => self.lpf_hz = hz,
        }
    }

    /// Every setting, checked; the key only as a number ([`check_key`] knows the route).
    ///
    /// # Errors
    ///
    /// The message of the first one outside its range.
    pub fn checked(self) -> Result<Self, String> {
        SidechainChange::Hpf(self.hpf_hz).checked()?;
        SidechainChange::Lpf(self.lpf_hz).checked()?;
        Ok(self)
    }

    /// What `show` says of it behind the rest of an input: nothing while it is neutral, else
    /// `  sidechain by [2]` or `  sidechain hpf 4000 Hz lpf 9000 Hz`.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.is_neutral() {
            return String::new();
        }
        let mut said = "  sidechain".to_owned();
        if self.key > 0 {
            let _ = write!(said, " by [{}]", self.key);
        }
        if self.high_on() {
            let _ = write!(said, " hpf {:.0} Hz", self.hpf_hz);
        }
        if self.low_on() {
            let _ = write!(said, " lpf {:.0} Hz", self.lpf_hz);
        }
        said
    }
}

/// Which setting of a sidechain an address or a control names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SidechainParam {
    /// Its key, the number of an input: `input/<n>/sidechain/key`.
    Key,
    /// The corner of its high-pass in Hz: `input/<n>/sidechain/hpf`.
    Hpf,
    /// The corner of its low-pass in Hz: `input/<n>/sidechain/lpf`.
    Lpf,
}

impl SidechainParam {
    /// All of them, in the order of the addresses.
    pub const ALL: [Self; 3] = [Self::Key, Self::Hpf, Self::Lpf];

    /// The words of its address behind `input/<n>/`.
    #[must_use]
    pub fn address(self) -> &'static str {
        match self {
            Self::Key => "sidechain/key",
            Self::Hpf => "sidechain/hpf",
            Self::Lpf => "sidechain/lpf",
        }
    }

    /// The setting those words name.
    #[must_use]
    pub fn from_address(words: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|param| param.address() == words)
    }

    /// The range of its numbers; `None` for the key, which the route limits.
    #[must_use]
    pub fn range(self) -> Option<(f32, f32)> {
        match self {
            Self::Key => None,
            Self::Hpf => Some(HPF_HZ),
            Self::Lpf => Some(LPF_HZ),
        }
    }

    /// The change that sets it to `number`; the key takes a whole number, anything else becomes
    /// a key no route has, which [`check_key`] refuses.
    #[must_use]
    pub fn change(self, number: f32) -> SidechainChange {
        match self {
            Self::Key => {
                let whole = number.is_finite() && number >= 0.0 && number.fract() == 0.0;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                SidechainChange::Key(if whole { number as usize } else { usize::MAX })
            }
            Self::Hpf => SidechainChange::Hpf(number),
            Self::Lpf => SidechainChange::Lpf(number),
        }
    }

    /// What `settings` has for it, as the change that would set it.
    #[must_use]
    pub fn read(self, settings: &SidechainSettings) -> SidechainChange {
        match self {
            Self::Key => SidechainChange::Key(settings.key),
            Self::Hpf => SidechainChange::Hpf(settings.hpf_hz),
            Self::Lpf => SidechainChange::Lpf(settings.lpf_hz),
        }
    }
}

/// One change of a sidechain.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum SidechainChange {
    /// The input gate and compressor listen to, by its number from 1; 0 is the input itself.
    Key(usize),
    /// The corner of the high-pass in Hz; the lower end of [`HPF_HZ`] switches it off.
    Hpf(f32),
    /// The corner of the low-pass in Hz; the upper end of [`LPF_HZ`] switches it off.
    Lpf(f32),
}

impl SidechainChange {
    /// Which setting it changes.
    #[must_use]
    pub fn param(self) -> SidechainParam {
        match self {
            Self::Key(_) => SidechainParam::Key,
            Self::Hpf(_) => SidechainParam::Hpf,
            Self::Lpf(_) => SidechainParam::Lpf,
        }
    }

    /// Its value as a number, the key as the number of its input.
    #[must_use]
    pub fn number(self) -> f32 {
        match self {
            #[allow(clippy::cast_precision_loss)]
            Self::Key(key) => key as f32,
            Self::Hpf(n) | Self::Lpf(n) => n,
        }
    }

    /// What it sets, as a reply says it: `sidechain by [2]`, `sidechain by itself`,
    /// `sidechain hpf 4000 Hz`, `sidechain lpf off`.
    #[must_use]
    pub fn said(self) -> String {
        match self {
            Self::Key(0) => "sidechain by itself".to_owned(),
            Self::Key(key) => format!("sidechain by [{key}]"),
            Self::Hpf(hz) if hz <= HPF_HZ.0 => "sidechain hpf off".to_owned(),
            Self::Hpf(hz) => format!("sidechain hpf {hz:.0} Hz"),
            Self::Lpf(hz) if hz >= LPF_HZ.1 => "sidechain lpf off".to_owned(),
            Self::Lpf(hz) => format!("sidechain lpf {hz:.0} Hz"),
        }
    }

    /// The change when its number is inside its range: nothing is clamped, as with a gain. A key
    /// passes here; whether the route has that input, [`check_key`] says.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise.
    pub fn checked(self) -> Result<Self, String> {
        let what = match self {
            Self::Key(_) => return Ok(self),
            Self::Hpf(_) => "high-pass",
            Self::Lpf(_) => "low-pass",
        };
        let value = self.number();
        match self.param().range() {
            Some((low, high)) if !(low..=high).contains(&value) => Err(format!(
                "the sidechain {what} must be {low} to {high} Hz, not {value}"
            )),
            _ => Ok(self),
        }
    }
}

/// `key` (a number from 1, 0 for the input itself) as the key of input `input` (an index from 0)
/// in a route of `inputs` inputs.
///
/// # Errors
///
/// A message when the route has no such input, or the input would be named as its own key
/// (which is what 0 says).
pub fn check_key(input: usize, key: usize, inputs: usize) -> Result<usize, String> {
    if key > inputs {
        Err(format!(
            "there is no input {key} to listen to (only {inputs})"
        ))
    } else if key == input + 1 {
        Err(format!(
            "[{key}] listens to itself without a key; sidechain off says so"
        ))
    } else {
        Ok(key)
    }
}

/// The sidechain of one input as the control thread sets it; neutral at first.
#[derive(Debug)]
pub struct SidechainControls {
    key: AtomicU32,
    hpf_hz: AtomicF32,
    lpf_hz: AtomicF32,
}

impl Default for SidechainControls {
    fn default() -> Self {
        let settings = SidechainSettings::default();
        Self {
            key: AtomicU32::new(0),
            hpf_hz: AtomicF32::new(settings.hpf_hz),
            lpf_hz: AtomicF32::new(settings.lpf_hz),
        }
    }
}

impl SidechainControls {
    /// Sets everything at once; a corner outside its range is clamped (check before with
    /// [`SidechainSettings::checked`]), one that is no number leaves its setting as it is.
    pub fn set_all(&self, settings: SidechainSettings) {
        self.key
            .store(u32::try_from(settings.key).unwrap_or(0), Ordering::Relaxed);
        if settings.hpf_hz.is_finite() {
            self.hpf_hz.store(settings.hpf_hz.clamp(HPF_HZ.0, HPF_HZ.1));
        }
        if settings.lpf_hz.is_finite() {
            self.lpf_hz.store(settings.lpf_hz.clamp(LPF_HZ.0, LPF_HZ.1));
        }
    }

    /// What it is set to now.
    #[must_use]
    pub fn settings(&self) -> SidechainSettings {
        SidechainSettings {
            key: self.key.load(Ordering::Relaxed) as usize,
            hpf_hz: self.hpf_hz.load(),
            lpf_hz: self.lpf_hz.load(),
        }
    }
}

/// The two filters in front of one detector, per channel.
#[derive(Debug)]
struct Filters {
    high: Vec<Biquad>,
    low: Vec<Biquad>,
}

impl Filters {
    fn new(channels: usize) -> Self {
        Self {
            high: vec![Biquad::default(); channels],
            low: vec![Biquad::default(); channels],
        }
    }

    fn configure(&mut self, settings: &SidechainSettings, rate: f32) {
        for filter in &mut self.high {
            filter.high_pass(settings.hpf_hz, rate);
        }
        for filter in &mut self.low {
            filter.low_pass(settings.lpf_hz, rate);
        }
    }

    fn start_over(&mut self) {
        for filter in self.high.iter_mut().chain(&mut self.low) {
            filter.start_over();
        }
    }

    /// `block` through the filters that are on, into `key`.
    fn run(&mut self, settings: &SidechainSettings, block: &[f32], key: &mut [f32]) {
        let channels = self.high.len();
        let (high_on, low_on) = (settings.high_on(), settings.low_on());
        for (from, to) in block
            .chunks_exact(channels)
            .zip(key.chunks_exact_mut(channels))
        {
            for (channel, (sample, out)) in from.iter().zip(to).enumerate() {
                let mut value = *sample;
                if high_on {
                    value = self.high[channel].next(value);
                }
                if low_on {
                    value = self.low[channel].next(value);
                }
                *out = value;
            }
        }
    }
}

/// Which detector asks for its key: each has filters of its own, since the compressor hears what
/// the gate let through.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detector {
    /// The gate of the input.
    Gate,
    /// Its compressor, behind the gate.
    Compressor,
}

/// The sidechain inside the stage of one input.
#[derive(Debug)]
pub struct Sidechain {
    rate: f32,
    settings: SidechainSettings,
    watch: KeyWatch,
    /// The level of the key for this block; `None` while the input listens to itself.
    level: Option<f32>,
    gate: Filters,
    compressor: Filters,
    /// What a detector listens to instead of the sound, for one block.
    key: Vec<f32>,
}

impl Sidechain {
    /// The sidechain of an input of `channels` at `rate`, for blocks of at most `frames` frames;
    /// neutral.
    #[must_use]
    pub fn new(channels: usize, rate: u32, frames: usize) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate_hz = rate.max(1) as f32;
        let settings = SidechainSettings::default();
        let mut sidechain = Self {
            rate: rate_hz,
            settings,
            watch: KeyWatch::new(rate),
            level: None,
            gate: Filters::new(channels),
            compressor: Filters::new(channels),
            key: vec![0.0; frames * channels],
        };
        sidechain.gate.configure(&settings, rate_hz);
        sidechain.compressor.configure(&settings, rate_hz);
        sidechain
    }

    /// Forgets what it heard, as after a new stream.
    pub fn start_over(&mut self) {
        self.watch.start_over();
        self.level = None;
        self.gate.start_over();
        self.compressor.start_over();
    }

    /// Starts a block of `frames` frames: takes the settings from `controls` and reads the level
    /// of the key from `board`. Without a board the input listens to itself.
    pub fn begin(&mut self, controls: &SidechainControls, board: Option<&KeyBoard>, frames: usize) {
        let settings = controls.settings();
        if settings != self.settings {
            if settings.key != self.settings.key {
                // Another key: what was seen of the old one says nothing about it.
                self.watch.start_over();
            }
            // A filter that comes on starts from rest, not from what it heard when it last ran.
            if settings.high_on() != self.settings.high_on()
                || settings.low_on() != self.settings.low_on()
            {
                self.gate.start_over();
                self.compressor.start_over();
            }
            self.settings = settings;
            self.gate.configure(&settings, self.rate);
            self.compressor.configure(&settings, self.rate);
        }
        self.level = match (settings.key, board) {
            (0, _) | (_, None) => None,
            (key, Some(board)) => Some(self.watch.level(board, key, frames, true)),
        };
    }

    /// What `detector` listens to instead of `block`: the level of the key through all of it, or
    /// `block` through the filters. `None` when it listens to `block` itself, and for a block
    /// longer than the sidechain was made for.
    pub fn key(&mut self, detector: Detector, block: &[f32]) -> Option<&[f32]> {
        let key = self.key.get_mut(..block.len())?;
        if let Some(level) = self.level {
            key.fill(level);
            return Some(key);
        }
        if !self.settings.high_on() && !self.settings.low_on() {
            return None;
        }
        let filters = match detector {
            Detector::Gate => &mut self.gate,
            Detector::Compressor => &mut self.compressor,
        };
        filters.run(&self.settings, block, key);
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const BLOCK: usize = 480;

    fn sine(hz: f32, frames: usize, start: usize) -> Vec<f32> {
        (0..frames)
            .map(|i| {
                #[allow(clippy::cast_precision_loss)]
                let t = (start + i) as f32 / RATE as f32;
                (std::f32::consts::TAU * hz * t).sin() * 0.5
            })
            .collect()
    }

    fn peak(data: &[f32]) -> f32 {
        data.iter().fold(0.0, |most, sample| most.max(sample.abs()))
    }

    /// The peak of the key of the last of `blocks` blocks of a sine of `hz`.
    fn heard(sidechain: &mut Sidechain, controls: &SidechainControls, hz: f32) -> Option<f32> {
        let mut last = None;
        for block in 0..40 {
            let data = sine(hz, BLOCK, block * BLOCK);
            sidechain.begin(controls, None, BLOCK);
            last = sidechain.key(Detector::Compressor, &data).map(peak);
        }
        last
    }

    #[test]
    fn neutral_it_hands_out_no_key() {
        let controls = SidechainControls::default();
        let mut sidechain = Sidechain::new(1, RATE, BLOCK);
        assert_eq!(heard(&mut sidechain, &controls, 100.0), None);
        assert!(controls.settings().is_neutral());
    }

    #[test]
    fn the_high_pass_takes_the_bass_out_of_what_the_detector_hears() {
        let controls = SidechainControls::default();
        controls.set_all(SidechainSettings {
            hpf_hz: 2000.0,
            ..SidechainSettings::default()
        });
        let mut sidechain = Sidechain::new(1, RATE, BLOCK);
        let bass = heard(&mut sidechain, &controls, 100.0).expect("a key");
        let top = heard(&mut sidechain, &controls, 6000.0).expect("a key");
        assert!(bass < 0.01, "the bass is gone: {bass}");
        assert!((top - 0.5).abs() < 0.03, "the top passes: {top}");
    }

    #[test]
    fn the_low_pass_takes_the_top_out_of_what_the_detector_hears() {
        let controls = SidechainControls::default();
        controls.set_all(SidechainSettings {
            lpf_hz: 300.0,
            ..SidechainSettings::default()
        });
        let mut sidechain = Sidechain::new(1, RATE, BLOCK);
        let bass = heard(&mut sidechain, &controls, 60.0).expect("a key");
        let top = heard(&mut sidechain, &controls, 6000.0).expect("a key");
        assert!((bass - 0.5).abs() < 0.03, "the bass passes: {bass}");
        assert!(top < 0.01, "the top is gone: {top}");
    }

    #[test]
    fn a_key_is_the_level_of_another_input_and_silent_once_it_stops() {
        let board = KeyBoard::new(2);
        let controls = SidechainControls::default();
        controls.set_all(SidechainSettings {
            key: 2,
            ..SidechainSettings::default()
        });
        let mut sidechain = Sidechain::new(2, RATE, BLOCK);
        let own = vec![0.9; BLOCK * 2];
        board.store_raw(1, 0.25);
        board.store(1, 0.0);
        sidechain.begin(&controls, Some(&board), BLOCK);
        let key = sidechain.key(Detector::Gate, &own).expect("a key");
        assert!(key.iter().all(|sample| (*sample - 0.25).abs() < 1e-6));
        // The key no longer beats: after a quarter of a second it counts as silent.
        let mut last = 1.0;
        for _ in 0..30 {
            sidechain.begin(&controls, Some(&board), BLOCK);
            last = sidechain.key(Detector::Gate, &own).expect("a key")[0];
        }
        assert_eq!(last, 0.0);
        // Without a board it listens to itself.
        sidechain.begin(&controls, None, BLOCK);
        assert!(sidechain.key(Detector::Gate, &own).is_none());
    }

    #[test]
    fn nothing_allocates_and_a_longer_block_keys_itself() {
        let board = KeyBoard::new(2);
        let controls = SidechainControls::default();
        controls.set_all(SidechainSettings {
            key: 2,
            hpf_hz: 500.0,
            lpf_hz: 5000.0,
        });
        let mut sidechain = Sidechain::new(1, RATE, BLOCK);
        let data = sine(1000.0, BLOCK, 0);
        assert_no_alloc::assert_no_alloc(|| {
            sidechain.begin(&controls, Some(&board), BLOCK);
            assert!(sidechain.key(Detector::Gate, &data).is_some());
            controls.set_all(SidechainSettings {
                key: 0,
                hpf_hz: 500.0,
                lpf_hz: 5000.0,
            });
            sidechain.begin(&controls, Some(&board), BLOCK);
            assert!(sidechain.key(Detector::Compressor, &data).is_some());
        });
        let long = vec![0.0; BLOCK + 1];
        assert!(sidechain.key(Detector::Gate, &long).is_none());
    }

    #[test]
    fn settings_changes_and_addresses() {
        let mut settings = SidechainSettings::default();
        settings.apply(SidechainChange::Key(2));
        settings.apply(SidechainChange::Hpf(4000.0));
        assert_eq!(settings.summary(), "  sidechain by [2] hpf 4000 Hz");
        assert!(SidechainChange::Hpf(5.0).checked().is_err());
        assert!(SidechainChange::Lpf(30_000.0).checked().is_err());
        assert!(SidechainChange::Key(99).checked().is_ok());
        assert!(check_key(0, 3, 2).unwrap_err().contains("no input 3"));
        assert!(check_key(1, 2, 2).unwrap_err().contains("itself"));
        assert_eq!(check_key(1, 1, 2), Ok(1));
        assert_eq!(check_key(1, 0, 2), Ok(0));
        for param in SidechainParam::ALL {
            assert_eq!(SidechainParam::from_address(param.address()), Some(param));
            let change = param.read(&settings);
            assert_eq!(change.param(), param);
            assert_eq!(param.change(change.number()), change);
        }
        assert_eq!(
            SidechainParam::Key.change(1.5),
            SidechainChange::Key(usize::MAX)
        );
        assert_eq!(SidechainChange::Key(0).said(), "sidechain by itself");
        assert_eq!(SidechainChange::Lpf(20_000.0).said(), "sidechain lpf off");
    }
}
