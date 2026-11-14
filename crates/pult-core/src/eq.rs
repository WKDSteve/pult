//! The parametric equalizer of a bus: a few bands, each one biquad filter.
//!
//! The control thread writes the bands into [`EqControls`] (atomics); the [`Equalizer`] in the
//! bus's callback follows them. Like every other change of the sound it does not jump: a gain
//! slides, a frequency and a width move by octaves, and a band changes its kind while it is
//! flat. A flat band costs nothing, it is skipped.
//!
//! All three kinds are flat at 0 dB, which is what makes switching a band or the whole
//! equalizer off a slide to 0 dB instead of a crossfade.
//!
//! # Dynamic bands
//!
//! A band with a threshold ([`Band::threshold_db`] below 0 dB) is dynamic: it listens to the
//! sound in its own range, before the equalizer (a bell through a band-pass of its frequency and
//! width, a shelf through a low- or high-pass at its frequency), and its gain goes from nothing
//! at the threshold to all of it [`DYNAMIC_REACH_DB`] above. A cut then only takes what is too
//! loud, and a boost only lifts what is there. The gain moves by the same slide as every other
//! change of a band, so nothing steps; it lets go with [`DYNAMIC_RELEASE_MS`].

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::level::{AtomicF32, within};

/// Bands of one equalizer.
pub const EQ_BANDS: usize = 4;

/// Channels an equalizer filters; further channels of a bus pass unchanged.
pub const EQ_CHANNELS: usize = 8;

/// Lowest and highest gain of a band in dB.
pub const EQ_GAIN_DB: (f32, f32) = (-24.0, 24.0);

/// Lowest and highest frequency of a band in Hz.
pub const EQ_FREQ_HZ: (f32, f32) = (20.0, 20_000.0);

/// Narrowest and widest setting of a band's width (Q).
pub const EQ_Q: (f32, f32) = (0.1, 10.0);

/// The threshold of a band in dB; at its upper end the band is not dynamic.
pub const EQ_THRESHOLD_DB: (f32, f32) = (-60.0, 0.0);

/// How many dB above its threshold a dynamic band stands at its whole gain.
pub const DYNAMIC_REACH_DB: f32 = 12.0;

/// Milliseconds a dynamic band takes to let go, as a time constant.
pub const DYNAMIC_RELEASE_MS: f32 = 150.0;

/// Frames between two steps of a moving band: short enough that a step is not heard.
const BLOCK: usize = 32;

/// How fast a gain slides, in dB per second: the whole range in 48 ms.
const GAIN_PACE: f32 = 1000.0;

/// How fast a frequency or a width moves, in octaves per second.
const OCTAVE_PACE: f32 = 100.0;

/// The highest frequency a band can stand at, as a part of the sample rate: below half of it,
/// where the filter is still well behaved.
const HIGHEST: f32 = 0.45;

/// What a band does to the sound. Profiles, commands and the control interface call the kinds
/// `low`, `bell` and `high`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum BandKind {
    /// Raises or lowers everything below its frequency.
    #[serde(rename = "low")]
    LowShelf,
    /// Raises or lowers a bell around its frequency.
    #[default]
    #[serde(rename = "bell")]
    Peak,
    /// Raises or lowers everything above its frequency.
    #[serde(rename = "high")]
    HighShelf,
}

impl BandKind {
    /// The word for it: `low`, `bell` or `high`.
    #[must_use]
    pub fn word(self) -> &'static str {
        match self {
            Self::LowShelf => "low",
            Self::Peak => "bell",
            Self::HighShelf => "high",
        }
    }

    /// The kind `word` names, ignoring case.
    #[must_use]
    pub fn from_word(word: &str) -> Option<Self> {
        [Self::LowShelf, Self::Peak, Self::HighShelf]
            .into_iter()
            .find(|kind| kind.word().eq_ignore_ascii_case(word))
    }

    fn from_code(code: u8) -> Self {
        match code {
            0 => Self::LowShelf,
            2 => Self::HighShelf,
            _ => Self::Peak,
        }
    }

    fn code(self) -> u8 {
        match self {
            Self::LowShelf => 0,
            Self::Peak => 1,
            Self::HighShelf => 2,
        }
    }
}

/// One band of an equalizer.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Band {
    /// What it does.
    pub kind: BandKind,
    /// Its frequency in Hz.
    pub freq_hz: f32,
    /// Its gain in dB; 0 is flat.
    pub gain_db: f32,
    /// Its width: a higher Q is a narrower bell or a steeper shelf.
    pub q: f32,
    /// The level in its own range, in dB, above which its gain comes in; at the upper end of
    /// [`EQ_THRESHOLD_DB`] the band is not dynamic and always has its gain.
    #[serde(default)]
    pub threshold_db: f32,
}

impl Band {
    /// Whether its gain depends on the level in its range.
    #[must_use]
    pub fn is_dynamic(&self) -> bool {
        self.threshold_db < EQ_THRESHOLD_DB.1
    }

    /// The flat band an equalizer starts with at place `index`: a low shelf, two bells and a
    /// high shelf.
    #[must_use]
    pub fn at(index: usize) -> Self {
        let (kind, freq_hz) = match index {
            0 => (BandKind::LowShelf, 100.0),
            1 => (BandKind::Peak, 500.0),
            2 => (BandKind::Peak, 2500.0),
            _ => (BandKind::HighShelf, 8000.0),
        };
        Self {
            kind,
            freq_hz,
            gain_db: 0.0,
            q: 0.7,
            threshold_db: EQ_THRESHOLD_DB.1,
        }
    }

    /// The bands an equalizer starts with, all flat.
    #[must_use]
    pub fn all_flat() -> [Self; EQ_BANDS] {
        std::array::from_fn(Self::at)
    }

    /// The same band with `change` made.
    ///
    /// # Errors
    ///
    /// A message naming the range when the new value is outside it.
    pub fn changed(self, change: BandChange) -> Result<Self, String> {
        Ok(match change.checked()? {
            BandChange::Kind(kind) => Self { kind, ..self },
            BandChange::Freq(freq_hz) => Self { freq_hz, ..self },
            BandChange::Gain(gain_db) => Self { gain_db, ..self },
            BandChange::Q(q) => Self { q, ..self },
            BandChange::Threshold(threshold_db) => Self {
                threshold_db,
                ..self
            },
        })
    }

    /// The band when all its values are inside their ranges.
    ///
    /// # Errors
    ///
    /// A message naming the first value that is not, and its range.
    pub fn checked(self) -> Result<Self, String> {
        BandChange::Freq(self.freq_hz).checked()?;
        BandChange::Gain(self.gain_db).checked()?;
        BandChange::Q(self.q).checked()?;
        BandChange::Threshold(self.threshold_db).checked()?;
        Ok(self)
    }

    /// The same band at `gain_db` dB.
    #[must_use]
    pub fn with_gain(self, gain_db: f32) -> Self {
        Self { gain_db, ..self }
    }

    /// The same band with every value inside its range; a NaN becomes the value of
    /// `Band::at(0)`, flat.
    #[must_use]
    pub fn clamped(self) -> Self {
        let start = Self::at(0);
        Self {
            kind: self.kind,
            freq_hz: within(self.freq_hz, EQ_FREQ_HZ, start.freq_hz),
            gain_db: within(self.gain_db, EQ_GAIN_DB, 0.0),
            q: within(self.q, EQ_Q, start.q),
            threshold_db: within(self.threshold_db, EQ_THRESHOLD_DB, start.threshold_db),
        }
    }
}

/// One setting of a band.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BandChange {
    /// What it does.
    Kind(BandKind),
    /// Its frequency in Hz.
    Freq(f32),
    /// Its gain in dB.
    Gain(f32),
    /// Its width (Q).
    Q(f32),
    /// Its threshold in dB; the upper end of [`EQ_THRESHOLD_DB`] makes it a plain band again.
    Threshold(f32),
}

impl BandChange {
    /// The change when its value is inside its range: nothing is clamped, as with a gain.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise.
    pub fn checked(self) -> Result<Self, String> {
        let (what, value, (low, high), unit) = match self {
            Self::Kind(_) => return Ok(self),
            Self::Freq(hz) => ("freq", hz, EQ_FREQ_HZ, " Hz"),
            Self::Gain(db) => ("gain", db, EQ_GAIN_DB, " dB"),
            Self::Q(q) => ("q", q, EQ_Q, ""),
            Self::Threshold(db) => ("threshold", db, EQ_THRESHOLD_DB, " dB"),
        };
        if (low..=high).contains(&value) {
            Ok(self)
        } else {
            Err(format!(
                "the {what} of a band must be {low} to {high}{unit}, not {value}"
            ))
        }
    }
}

#[derive(Debug)]
struct BandControls {
    kind: AtomicU8,
    freq_hz: AtomicF32,
    gain_db: AtomicF32,
    q: AtomicF32,
    threshold_db: AtomicF32,
}

/// The equalizer of one bus as the control thread sets it.
#[derive(Debug)]
pub struct EqControls {
    on: AtomicBool,
    bands: [BandControls; EQ_BANDS],
    /// Counts the changes: the callback reads the bands only when it moved.
    version: AtomicU32,
}

impl Default for EqControls {
    fn default() -> Self {
        Self {
            on: AtomicBool::new(true),
            bands: std::array::from_fn(|index| {
                let band = Band::at(index);
                BandControls {
                    kind: AtomicU8::new(band.kind.code()),
                    freq_hz: AtomicF32::new(band.freq_hz),
                    gain_db: AtomicF32::new(band.gain_db),
                    q: AtomicF32::new(band.q),
                    threshold_db: AtomicF32::new(band.threshold_db),
                }
            }),
            version: AtomicU32::new(0),
        }
    }
}

impl EqControls {
    /// Sets band `index` (from 0), clamped to its ranges; an index beyond [`EQ_BANDS`] is
    /// ignored.
    pub fn set(&self, index: usize, band: Band) {
        let Some(controls) = self.bands.get(index) else {
            return;
        };
        let band = band.clamped();
        controls.kind.store(band.kind.code(), Ordering::Relaxed);
        controls.freq_hz.store(band.freq_hz);
        controls.gain_db.store(band.gain_db);
        controls.q.store(band.q);
        controls.threshold_db.store(band.threshold_db);
        // After the values: a callback that saw half of them reads them again.
        self.version.fetch_add(1, Ordering::Release);
    }

    /// Sets all bands and the switch at once: what a route starts with.
    pub fn set_all(&self, on: bool, bands: &[Band; EQ_BANDS]) {
        for (index, band) in bands.iter().enumerate() {
            self.set(index, *band);
        }
        self.switch(on);
    }

    /// Band `index` (from 0) as it is set; `None` beyond [`EQ_BANDS`].
    #[must_use]
    pub fn band(&self, index: usize) -> Option<Band> {
        let controls = self.bands.get(index)?;
        Some(Band {
            kind: BandKind::from_code(controls.kind.load(Ordering::Relaxed)),
            freq_hz: controls.freq_hz.load(),
            gain_db: controls.gain_db.load(),
            q: controls.q.load(),
            threshold_db: controls.threshold_db.load(),
        })
    }

    /// Switches the whole equalizer on or off; off, every band slides to flat and keeps its
    /// setting.
    pub fn switch(&self, on: bool) {
        self.on.store(on, Ordering::Release);
    }

    /// Whether the equalizer is switched on.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Acquire)
    }
}

/// The filter of one band: `y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2` (direct form I).
#[derive(Clone, Copy, Debug)]
struct Filter {
    b0: f64,
    b1: f64,
    b2: f64,
    a1: f64,
    a2: f64,
}

impl Filter {
    /// The filter of `band` at `rate` frames per second, after the Audio EQ Cookbook
    /// (Robert Bristow-Johnson).
    fn of(band: Band, rate: f32) -> Self {
        let amount = 10f64.powf(f64::from(band.gain_db) / 40.0);
        let freq = band.freq_hz.min(rate * HIGHEST);
        let turn = TAU * f64::from(freq) / f64::from(rate);
        let (sin, cos) = turn.sin_cos();
        let alpha = sin / (2.0 * f64::from(band.q));
        let (up, down) = (amount + 1.0, amount - 1.0);
        let slope = 2.0 * amount.sqrt() * alpha;
        let (b, a) = match band.kind {
            BandKind::Peak => (
                [1.0 + alpha * amount, -2.0 * cos, 1.0 - alpha * amount],
                [1.0 + alpha / amount, -2.0 * cos, 1.0 - alpha / amount],
            ),
            BandKind::LowShelf => (
                [
                    amount * (up - down * cos + slope),
                    2.0 * amount * (down - up * cos),
                    amount * (up - down * cos - slope),
                ],
                [
                    up + down * cos + slope,
                    -2.0 * (down + up * cos),
                    up + down * cos - slope,
                ],
            ),
            BandKind::HighShelf => (
                [
                    amount * (up + down * cos + slope),
                    -2.0 * amount * (down + up * cos),
                    amount * (up + down * cos - slope),
                ],
                [
                    up - down * cos + slope,
                    2.0 * (down - up * cos),
                    up - down * cos - slope,
                ],
            ),
        };
        Self {
            b0: b[0] / a[0],
            b1: b[1] / a[0],
            b2: b[2] / a[0],
            a1: a[1] / a[0],
            a2: a[2] / a[0],
        }
    }
}

impl Filter {
    /// What a dynamic `band` listens through at `rate`: a bell through a band-pass of its
    /// frequency and width (0 dB at the frequency), a shelf through a low- or high-pass at its
    /// frequency.
    fn listening(band: Band, rate: f32) -> Self {
        let freq = band.freq_hz.min(rate * HIGHEST);
        let turn = TAU * f64::from(freq) / f64::from(rate);
        let (sin, cos) = turn.sin_cos();
        let q = match band.kind {
            BandKind::Peak => f64::from(band.q),
            BandKind::LowShelf | BandKind::HighShelf => std::f64::consts::FRAC_1_SQRT_2,
        };
        let alpha = sin / (2.0 * q);
        let b = match band.kind {
            BandKind::Peak => [alpha, 0.0, -alpha],
            BandKind::LowShelf => [0.5 * (1.0 - cos), 1.0 - cos, 0.5 * (1.0 - cos)],
            BandKind::HighShelf => {
                let half = f64::midpoint(1.0, cos);
                [half, -(1.0 + cos), half]
            }
        };
        let a0 = 1.0 + alpha;
        Self {
            b0: b[0] / a0,
            b1: b[1] / a0,
            b2: b[2] / a0,
            a1: -2.0 * cos / a0,
            a2: (1.0 - alpha) / a0,
        }
    }

    /// Its gain at `turn` radians per frame (the frequency over the rate times a full turn), in
    /// dB: the magnitude of `(b0 + b1 z^-1 + b2 z^-2) / (1 + a1 z^-1 + a2 z^-2)` at `z = e^(i turn)`.
    fn gain_db(&self, turn: f64) -> f64 {
        let (sin1, cos1) = turn.sin_cos();
        let (sin2, cos2) = (2.0 * turn).sin_cos();
        let squared = |c0: f64, c1: f64, c2: f64| {
            let re = c0 + c1 * cos1 + c2 * cos2;
            let im = c1 * sin1 + c2 * sin2;
            re * re + im * im
        };
        10.0 * (squared(self.b0, self.b1, self.b2) / squared(1.0, self.a1, self.a2)).log10()
    }
}

/// What `bands` do together to a sine of `freq_hz` at `rate` frames per second, in dB: the
/// frequency response of the equalizer once its bands stand where they are set, the same filters
/// its callback runs. For the control thread (a curve to draw); a flat band adds nothing. A
/// dynamic band counts with its whole gain: the most it does.
#[must_use]
pub fn response_db(bands: &[Band], rate: u32, freq_hz: f32) -> f32 {
    let rate = rate.max(1) as f32;
    let turn = TAU * f64::from(freq_hz) / f64::from(rate);
    let total: f64 = bands
        .iter()
        .filter(|band| band.gain_db != 0.0)
        .map(|band| Filter::of(*band, rate).gain_db(turn))
        .sum();
    #[allow(clippy::cast_possible_truncation)]
    let total = total as f32;
    total
}

/// The equalizer inside a bus's callback.
#[derive(Debug)]
pub struct Equalizer {
    rate: f32,
    /// Nothing played yet, or not for a while: it stands at what it sees next.
    fresh: bool,
    seen: u32,
    on: bool,
    /// The bands as set.
    set: [Band; EQ_BANDS],
    /// Where the bands stand on their way there.
    now: [Band; EQ_BANDS],
    filters: [Filter; EQ_BANDS],
    /// What each filter remembers, per channel and band: the two past inputs, then the two past
    /// outputs (direct form I). Its past is sound, not a scaled state: a filter whose
    /// coefficients move while it runs then moves with them without a step.
    memory: [[[f64; 4]; EQ_BANDS]; EQ_CHANNELS],
    /// How much of its gain each band has now: 1 for a plain band, 0 to 1 for a dynamic one.
    amount: [f32; EQ_BANDS],
    /// What the dynamic bands listen through, and what those filters remember.
    listeners: [Filter; EQ_BANDS],
    heard: [[[f64; 4]; EQ_BANDS]; EQ_CHANNELS],
    /// The level each dynamic band heard, falling by `release` per frame.
    envelope: [f32; EQ_BANDS],
    release: f32,
}

impl Equalizer {
    /// An equalizer at `rate` frames per second, which stands at what it sees first.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let set = std::array::from_fn(Band::at);
        let rate = rate.max(1) as f32;
        Self {
            rate,
            fresh: true,
            seen: 0,
            on: true,
            set,
            now: set,
            filters: set.map(|band| Filter::of(band, rate)),
            memory: [[[0.0; 4]; EQ_BANDS]; EQ_CHANNELS],
            amount: [1.0; EQ_BANDS],
            listeners: set.map(|band| Filter::listening(band, rate)),
            heard: [[[0.0; 4]; EQ_BANDS]; EQ_CHANNELS],
            envelope: [0.0; EQ_BANDS],
            release: (-1000.0 / (DYNAMIC_RELEASE_MS * rate)).exp(),
        }
    }

    /// Stands at what it sees next, with nothing remembered: nothing played for a while, so
    /// nothing can click.
    pub fn start_over(&mut self) {
        self.fresh = true;
    }

    /// Filters `data` (interleaved, `channels` per frame) in place as `controls` say.
    pub fn process(&mut self, controls: &EqControls, data: &mut [f32], channels: usize) {
        if channels == 0 {
            return;
        }
        self.follow(controls);
        if self.rests() {
            return;
        }
        let filtered = channels.min(EQ_CHANNELS);
        for block in data.chunks_mut(BLOCK * channels) {
            let frames = block.len() / channels;
            self.listen(block, channels, filtered);
            self.step(frames);
            for band in 0..EQ_BANDS {
                #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
                let flat = self.now[band].gain_db == 0.0;
                if flat && self.forgot(band) {
                    // Nothing to do, but the past has to be there when the band comes back: the
                    // last two frames, as they are after the bands before it.
                    for (channel, memory) in self.memory.iter_mut().enumerate().take(filtered) {
                        let last = f64::from(block[(frames - 1) * channels + channel]);
                        let before =
                            f64::from(block[frames.saturating_sub(2) * channels + channel]);
                        memory[band] = [last, before, last, before];
                    }
                    continue;
                }
                let filter = self.filters[band];
                for (channel, memory) in self.memory.iter_mut().enumerate().take(filtered) {
                    let [mut x1, mut x2, mut y1, mut y2] = memory[band];
                    for frame in block.chunks_exact_mut(channels) {
                        // A NaN would stay in the filter for ever; the bus silences it anyway.
                        let sample = frame[channel];
                        let x = if sample.is_finite() {
                            f64::from(sample)
                        } else {
                            0.0
                        };
                        let y = filter.b0 * x + filter.b1 * x1 + filter.b2 * x2
                            - filter.a1 * y1
                            - filter.a2 * y2;
                        (x2, x1, y2, y1) = (x1, x, y1, y);
                        #[allow(clippy::cast_possible_truncation)]
                        let out = y as f32;
                        frame[channel] = out;
                    }
                    if flat {
                        // Flat, the output follows the input once what it remembers has died
                        // away; given up once no one can hear it (240 dB down), so that it rests.
                        y1 = settled_to(y1, x1);
                        y2 = settled_to(y2, x2);
                    }
                    // What fades after the sound ends would become slow, tiny numbers.
                    memory[band] = [settled(x1), settled(x2), settled(y1), settled(y2)];
                }
            }
        }
    }

    /// Reads the bands when they changed.
    fn follow(&mut self, controls: &EqControls) {
        let version = controls.version.load(Ordering::Acquire);
        let on = controls.is_on();
        if !self.fresh && version == self.seen && on == self.on {
            return;
        }
        for (index, band) in self.set.iter_mut().enumerate() {
            if let Some(set) = controls.band(index) {
                *band = set;
            }
        }
        self.seen = version;
        self.on = on;
        let fresh = std::mem::take(&mut self.fresh);
        for index in 0..EQ_BANDS {
            let band = self.set[index];
            self.listeners[index] = Filter::listening(band, self.rate);
            if !band.is_dynamic() {
                self.amount[index] = 1.0;
            } else if fresh {
                // It has heard nothing yet.
                self.amount[index] = 0.0;
            }
        }
        if fresh {
            self.envelope = [0.0; EQ_BANDS];
            self.heard = [[[0.0; 4]; EQ_BANDS]; EQ_CHANNELS];
            self.now = std::array::from_fn(|index| self.goal(index));
            self.filters = self.now.map(|band| Filter::of(band, self.rate));
            self.memory = [[[0.0; 4]; EQ_BANDS]; EQ_CHANNELS];
        }
    }

    /// Where band `index` is to go: as set, with as much of its gain as it has now, or flat
    /// while the equalizer is off.
    fn goal(&self, index: usize) -> Band {
        let band = self.set[index];
        Band {
            gain_db: if self.on {
                band.gain_db * self.amount[index]
            } else {
                0.0
            },
            ..band
        }
    }

    /// Whether band `index` has to listen: dynamic, with a gain to bring in.
    fn listens(&self, index: usize) -> bool {
        let band = self.set[index];
        self.on && band.is_dynamic() && band.gain_db != 0.0
    }

    /// Lets every dynamic band hear `block` (before any band changed it) and says how much of
    /// its gain it has for it.
    fn listen(&mut self, block: &[f32], channels: usize, filtered: usize) {
        let frames = block.len() / channels;
        #[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
        let fallen = self.release.powi(frames as i32);
        for band in 0..EQ_BANDS {
            if !self.listens(band) {
                // What it heard before it stopped says nothing about what comes when it
                // listens again.
                self.envelope[band] = 0.0;
                for heard in &mut self.heard {
                    heard[band] = [0.0; 4];
                }
                continue;
            }
            let filter = self.listeners[band];
            let mut loudest = 0.0_f64;
            for (channel, heard) in self.heard.iter_mut().enumerate().take(filtered) {
                let [mut x1, mut x2, mut y1, mut y2] = heard[band];
                for frame in block.chunks_exact(channels) {
                    let sample = frame[channel];
                    let x = if sample.is_finite() {
                        f64::from(sample)
                    } else {
                        0.0
                    };
                    let y = filter.b0 * x + filter.b1 * x1 + filter.b2 * x2
                        - filter.a1 * y1
                        - filter.a2 * y2;
                    (x2, x1, y2, y1) = (x1, x, y1, y);
                    loudest = loudest.max(y.abs());
                }
                heard[band] = [settled(x1), settled(x2), settled(y1), settled(y2)];
            }
            #[allow(clippy::cast_possible_truncation)]
            let loudest = loudest as f32;
            let level = loudest.max(self.envelope[band] * fallen);
            self.envelope[band] = if level < 1e-9 { 0.0 } else { level };
            let over = 20.0 * level.max(1e-9).log10() - self.set[band].threshold_db;
            self.amount[band] = (over / DYNAMIC_REACH_DB).clamp(0.0, 1.0);
        }
    }

    /// Whether every band is flat, stays so and remembers nothing, and none listens: nothing
    /// to do.
    fn rests(&self) -> bool {
        #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
        (0..EQ_BANDS).all(|index| {
            self.now[index].gain_db == 0.0
                && self.goal(index).gain_db == 0.0
                && self.forgot(index)
                && !self.listens(index)
        })
    }

    /// Whether the filter of `band`, flat, has nothing left to let die away on any channel: its
    /// past outputs are its past inputs.
    fn forgot(&self, band: usize) -> bool {
        #[allow(clippy::float_cmp)] // Exactly: what is near enough is set equal.
        self.memory.iter().all(|memory| {
            let [x1, x2, y1, y2] = memory[band];
            y1 == x1 && y2 == x2
        })
    }

    /// Moves every band that is not where it is to be by `frames` frames' worth.
    fn step(&mut self, frames: usize) {
        let seconds = frames as f32 / self.rate;
        let octaves = 2f32.powf(OCTAVE_PACE * seconds);
        for index in 0..EQ_BANDS {
            let goal = self.goal(index);
            let forgot = self.forgot(index);
            let now = &mut self.now[index];
            // The threshold changes what the band listens for, not its filter.
            now.threshold_db = goal.threshold_db;
            if *now == goal {
                continue;
            }
            #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
            if now.gain_db == 0.0 && forgot {
                // Flat, with nothing remembered, sounds the same for every kind, frequency and
                // width: change them for free. (What is remembered would ring on through other
                // poles, with a step.)
                now.kind = goal.kind;
                now.freq_hz = goal.freq_hz;
                now.q = goal.q;
            }
            // Another kind is reached through flat.
            let gain = if now.kind == goal.kind {
                goal.gain_db
            } else {
                0.0
            };
            now.gain_db = toward(now.gain_db, gain, GAIN_PACE * seconds);
            now.freq_hz = toward_by(now.freq_hz, goal.freq_hz, octaves);
            now.q = toward_by(now.q, goal.q, octaves);
            self.filters[index] = Filter::of(*now, self.rate);
        }
    }
}

/// `value`, or zero when it is too small to matter.
fn settled(value: f64) -> f64 {
    if value.abs() < 1e-25 { 0.0 } else { value }
}

/// `value`, or `goal` when it is near enough that no one hears the difference.
fn settled_to(value: f64, goal: f64) -> f64 {
    if (value - goal).abs() < 1e-12 {
        goal
    } else {
        value
    }
}

/// `value` moved towards `goal` by at most `step`.
fn toward(value: f32, goal: f32, step: f32) -> f32 {
    if (goal - value).abs() <= step {
        goal
    } else {
        value + step.copysign(goal - value)
    }
}

/// `value` moved towards `goal` by at most the factor `factor` (above 1).
fn toward_by(value: f32, goal: f32, factor: f32) -> f32 {
    if value < goal {
        (value * factor).min(goal)
    } else {
        (value / factor).max(goal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;

    /// One second of a sine of `freq` Hz at half scale, on `channels` channels.
    fn sine(freq: f32, channels: usize) -> Vec<f32> {
        let step = std::f32::consts::TAU * freq / RATE as f32;
        (0..RATE as usize)
            .flat_map(|frame| std::iter::repeat_n(0.5 * (step * frame as f32).sin(), channels))
            .collect()
    }

    /// The level of the last quarter of `data`, a sine, against half scale, in dB. From its
    /// power: near half the rate the samples miss the peaks of the wave.
    fn level_db(data: &[f32]) -> f32 {
        let tail = &data[data.len() / 4 * 3..];
        let power = tail.iter().map(|s| s * s).sum::<f32>() / tail.len() as f32;
        20.0 * ((2.0 * power).sqrt() / 0.5).log10()
    }

    /// What an equalizer with band 0 set to `band` does to a sine of `freq` Hz, in dB.
    fn response(band: Band, freq: f32) -> f32 {
        let controls = EqControls::default();
        controls.set(0, band);
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(freq, 1);
        eq.process(&controls, &mut data, 1);
        level_db(&data)
    }

    fn band(kind: BandKind, freq_hz: f32, gain_db: f32) -> Band {
        Band {
            kind,
            freq_hz,
            gain_db,
            q: 0.7,
            threshold_db: 0.0,
        }
    }

    #[test]
    fn a_flat_equalizer_changes_nothing() {
        let controls = EqControls::default();
        let mut eq = Equalizer::new(RATE);
        let original = sine(1000.0, 2);
        let mut data = original.clone();
        eq.process(&controls, &mut data, 2);
        assert_eq!(data, original);
    }

    #[test]
    fn a_bell_raises_its_frequency_and_leaves_the_far_ones() {
        let bell = Band {
            q: 2.0,
            ..band(BandKind::Peak, 1000.0, 12.0)
        };
        assert!((response(bell, 1000.0) - 12.0).abs() < 0.1);
        assert!(response(bell, 60.0).abs() < 0.2);
        assert!(response(bell, 12_000.0).abs() < 0.2);
        let cut = Band {
            gain_db: -12.0,
            ..bell
        };
        assert!((response(cut, 1000.0) + 12.0).abs() < 0.1);
    }

    #[test]
    fn the_shelves_move_their_side_only() {
        let low = band(BandKind::LowShelf, 200.0, -12.0);
        assert!((response(low, 30.0) + 12.0).abs() < 0.3);
        assert!(response(low, 5000.0).abs() < 0.2);
        let high = band(BandKind::HighShelf, 4000.0, 6.0);
        assert!((response(high, 16_000.0) - 6.0).abs() < 0.3);
        assert!(response(high, 100.0).abs() < 0.2);
    }

    #[test]
    fn the_bands_add_up_and_channels_stay_apart() {
        let controls = EqControls::default();
        controls.set(1, band(BandKind::Peak, 1000.0, 6.0));
        controls.set(2, band(BandKind::Peak, 1000.0, 6.0));
        let mut eq = Equalizer::new(RATE);
        // Left a sine, right silence.
        let mut data = sine(1000.0, 2);
        for frame in data.as_chunks_mut::<2>().0 {
            frame[1] = 0.0;
        }
        eq.process(&controls, &mut data, 2);
        let frames = data.as_chunks::<2>().0;
        let left: Vec<f32> = frames.iter().map(|frame| frame[0]).collect();
        assert!((level_db(&left) - 12.0).abs() < 0.1);
        assert!(frames.iter().all(|frame| frame[1] == 0.0));
    }

    #[test]
    fn a_new_gain_slides_instead_of_jumping() {
        let controls = EqControls::default();
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(1000.0, 1);
        let (before, after) = data.split_at_mut(RATE as usize / 2);
        eq.process(&controls, before, 1);
        controls.set(1, band(BandKind::Peak, 1000.0, 12.0));
        eq.process(&controls, after, 1);
        // The first millisecond is hardly louder, 12 dB take 12 ms, then it stands there.
        let peak = |part: &[f32]| part.iter().fold(0.0f32, |max, s| max.max(s.abs()));
        assert!(peak(&after[..48]) < 0.5 * 1.15, "{}", peak(&after[..48]));
        assert!(peak(&after[..240]) < 0.5 * 2.0, "{}", peak(&after[..240]));
        assert!((level_db(after) - 12.0).abs() < 0.1);
    }

    #[test]
    fn switched_off_it_slides_to_flat_and_then_rests() {
        let controls = EqControls::default();
        controls.set(0, band(BandKind::LowShelf, 200.0, 12.0));
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!((level_db(&data) - 12.0).abs() < 0.3);

        controls.switch(false);
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!(level_db(&data).abs() < 0.01);
        assert!(eq.rests());
        // The setting is kept for when it comes back.
        assert_eq!(controls.band(0).map(|band| band.gain_db), Some(12.0));
        controls.switch(true);
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!((level_db(&data) - 12.0).abs() < 0.3);
    }

    /// The largest step between neighbouring samples.
    fn largest_step(data: &[f32]) -> f32 {
        data.windows(2)
            .map(|pair| (pair[1] - pair[0]).abs())
            .fold(0.0, f32::max)
    }

    #[test]
    fn a_band_that_arrives_at_flat_rings_out_without_a_step() {
        // A narrow bell on a low tone remembers much: where the slide to flat ends, the filter
        // is far from done.
        let bell = Band {
            q: 0.7,
            ..band(BandKind::Peak, 50.0, 12.0)
        };
        let controls = EqControls::default();
        controls.set(0, bell);
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        // The tone alone steps by 0.0033 a sample, raised by 12 dB by 0.013.
        let raised = largest_step(&data[RATE as usize / 2..]);
        controls.switch(false);
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!(
            largest_step(&data) <= raised * 1.05,
            "{} against {raised}",
            largest_step(&data)
        );
        assert!(level_db(&data).abs() < 0.01);
        assert!(eq.rests(), "and then it remembers nothing");

        // Another kind goes through flat too.
        controls.switch(true);
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        controls.set(0, band(BandKind::HighShelf, 50.0, 12.0));
        let mut data = sine(50.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!(
            largest_step(&data) <= raised * 1.05,
            "{} against {raised}",
            largest_step(&data)
        );
    }

    #[test]
    fn a_slide_through_flat_does_not_step_where_it_lands_on_it() {
        // At 32 kHz a slide moves by exactly 1 dB a block, so from +3 to -3 dB it stands on
        // 0 dB for one block.
        let controls = EqControls::default();
        controls.set(0, band(BandKind::LowShelf, 200.0, 3.0));
        let mut eq = Equalizer::new(32_000);
        let mut data = vec![0.25; 32_000];
        eq.process(&controls, &mut data, 1);
        controls.set(0, band(BandKind::LowShelf, 200.0, -3.0));
        let mut data = vec![0.25; 32_000];
        eq.process(&controls, &mut data, 1);
        assert!(largest_step(&data) < 0.002, "{}", largest_step(&data));
        let last = data[data.len() - 1];
        assert!(
            (last - 0.25 * 10f32.powf(-3.0 / 20.0)).abs() < 1e-3,
            "{last}"
        );
    }

    #[test]
    fn another_kind_is_reached_through_flat() {
        let controls = EqControls::default();
        controls.set(0, band(BandKind::LowShelf, 1000.0, 12.0));
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(100.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!((level_db(&data) - 12.0).abs() < 0.3);

        controls.set(0, band(BandKind::HighShelf, 1000.0, 12.0));
        let mut data = sine(100.0, 1);
        eq.process(&controls, &mut data, 1);
        assert_eq!(eq.now[0], band(BandKind::HighShelf, 1000.0, 12.0));
        assert!(level_db(&data).abs() < 0.3, "{}", level_db(&data));
        // On the way the level never rose above where it started.
        let peak = data.iter().fold(0.0f32, |max, s| max.max(s.abs()));
        assert!(peak < 0.5 * 4.1, "{peak}");
    }

    #[test]
    fn a_frequency_moves_by_octaves() {
        let controls = EqControls::default();
        controls.set(1, band(BandKind::Peak, 250.0, 12.0));
        let mut eq = Equalizer::new(RATE);
        let mut data = vec![0.0; 480];
        eq.process(&controls, &mut data, 1);
        controls.set(1, band(BandKind::Peak, 4000.0, 12.0));
        // Four octaves at 100 octaves a second: 40 ms. After 10 ms it is one octave on.
        eq.process(&controls, &mut data, 1);
        assert!((eq.now[1].freq_hz - 500.0).abs() < 25.0, "{:?}", eq.now[1]);
        for _ in 0..4 {
            eq.process(&controls, &mut data, 1);
        }
        assert_eq!(eq.now[1].freq_hz, 4000.0);
    }

    #[test]
    fn values_out_of_range_are_clamped_and_nan_is_flat() {
        let controls = EqControls::default();
        controls.set(
            0,
            Band {
                kind: BandKind::Peak,
                freq_hz: 1e9,
                gain_db: 99.0,
                q: 0.0,
                threshold_db: 0.0,
            },
        );
        let set = controls.band(0).expect("band 0");
        assert_eq!((set.freq_hz, set.gain_db, set.q), (20_000.0, 24.0, 0.1));
        controls.set(
            1,
            Band {
                kind: BandKind::Peak,
                freq_hz: f32::NAN,
                gain_db: f32::NAN,
                q: f32::NAN,
                threshold_db: 0.0,
            },
        );
        let set = controls.band(1).expect("band 1");
        assert_eq!((set.freq_hz, set.gain_db, set.q), (100.0, 0.0, 0.7));
        // Beyond the bands: nothing, and no panic.
        controls.set(EQ_BANDS, Band::at(0));
        assert_eq!(controls.band(EQ_BANDS), None);
    }

    #[test]
    fn a_change_outside_its_range_is_refused_not_clamped() {
        let band = Band::at(1);
        assert_eq!(band.changed(BandChange::Gain(6.0)), Ok(band.with_gain(6.0)));
        assert_eq!(
            band.changed(BandChange::Kind(BandKind::HighShelf))
                .map(|band| band.kind),
            Ok(BandKind::HighShelf)
        );
        let why = |change| band.changed(change).expect_err("refused");
        assert_eq!(
            why(BandChange::Gain(30.0)),
            "the gain of a band must be -24 to 24 dB, not 30"
        );
        assert_eq!(
            why(BandChange::Freq(5.0)),
            "the freq of a band must be 20 to 20000 Hz, not 5"
        );
        assert_eq!(
            why(BandChange::Q(0.0)),
            "the q of a band must be 0.1 to 10, not 0"
        );
        assert!(why(BandChange::Gain(f32::NAN)).contains("not NaN"));
        assert!(Band::at(0).checked().is_ok());
        assert!(Band::at(0).with_gain(-25.0).checked().is_err());
    }

    #[test]
    fn the_kinds_have_their_words() {
        for kind in [BandKind::LowShelf, BandKind::Peak, BandKind::HighShelf] {
            assert_eq!(BandKind::from_word(kind.word()), Some(kind));
            let json = serde_json::to_string(&kind).expect("to JSON");
            assert_eq!(json, format!("\"{}\"", kind.word()));
        }
        assert_eq!(BandKind::from_word("BELL"), Some(BandKind::Peak));
        assert_eq!(BandKind::from_word("notch"), None);
    }

    #[test]
    fn a_band_above_half_the_rate_stays_a_filter() {
        let controls = EqControls::default();
        controls.set(3, band(BandKind::HighShelf, 20_000.0, 12.0));
        let mut eq = Equalizer::new(22_050);
        let mut data = sine(1000.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!(data.iter().all(|sample| sample.is_finite()));
    }

    #[test]
    fn a_nan_does_not_stay_in_the_filter() {
        let controls = EqControls::default();
        controls.set(1, band(BandKind::Peak, 1000.0, 12.0));
        let mut eq = Equalizer::new(RATE);
        let mut broken = [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 0.1];
        eq.process(&controls, &mut broken, 1);
        let mut data = sine(1000.0, 1);
        eq.process(&controls, &mut data, 1);
        assert!(data.iter().all(|sample| sample.is_finite()));
        assert!((level_db(&data) - 12.0).abs() < 0.1);
    }

    #[test]
    fn more_channels_than_it_filters_pass_unchanged() {
        let controls = EqControls::default();
        controls.set(0, band(BandKind::LowShelf, 200.0, 12.0));
        let mut eq = Equalizer::new(RATE);
        let channels = EQ_CHANNELS + 1;
        let original = sine(50.0, channels);
        let mut data = original.clone();
        eq.process(&controls, &mut data, channels);
        let last = |data: &[f32]| -> Vec<f32> {
            data.chunks_exact(channels)
                .map(|frame| frame[channels - 1])
                .collect()
        };
        assert_eq!(last(&data), last(&original));
        let first: Vec<f32> = data.chunks_exact(channels).map(|frame| frame[0]).collect();
        assert!((level_db(&first) - 12.0).abs() < 0.3);
    }

    #[test]
    fn it_filters_and_follows_without_allocating() {
        let controls = EqControls::default();
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(1000.0, 2);
        assert_no_alloc::assert_no_alloc(|| {
            for (round, block) in data.chunks_mut(960).enumerate() {
                match round {
                    2 => controls.set(1, band(BandKind::Peak, 1000.0, 9.0)),
                    10 => controls.set(1, band(BandKind::HighShelf, 300.0, -9.0)),
                    20 => controls.switch(false),
                    30 => eq.start_over(),
                    _ => {}
                }
                eq.process(&controls, block, 2);
            }
        });
    }

    /// How much louder a sine of `freq` comes out, in dB, once `bands` stand and the filters have
    /// settled.
    fn measured_db(bands: [Band; EQ_BANDS], rate: u32, freq: f64) -> f32 {
        let controls = EqControls::default();
        controls.set_all(true, &bands);
        let mut eq = Equalizer::new(rate);
        let frames = rate as usize;
        let turn = TAU * freq / f64::from(rate);
        #[allow(clippy::cast_possible_truncation)]
        let mut data: Vec<f32> = (0..frames)
            .map(|frame| (0.5 * (turn * frame as f64).sin()) as f32)
            .collect();
        eq.process(&controls, &mut data, 1);
        // The effective value over the settled half: a peak of the samples misses the crest of a
        // high tone, which has only a few samples per period.
        let settled = &data[frames / 2..];
        let power: f64 =
            settled.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>() / settled.len() as f64;
        let amplitude = (2.0 * power).sqrt();
        #[allow(clippy::cast_possible_truncation)]
        let db = (20.0 * (amplitude / 0.5).log10()) as f32;
        db
    }

    #[test]
    fn the_response_is_what_a_sine_comes_out_with() {
        let mut bands = Band::all_flat();
        bands[0] = bands[0].with_gain(6.0);
        bands[1] = Band {
            kind: BandKind::Peak,
            freq_hz: 1000.0,
            gain_db: -9.0,
            q: 2.0,
            threshold_db: 0.0,
        };
        bands[3] = bands[3].with_gain(4.5);
        for rate in [44_100, 48_000] {
            for freq in [40.0, 100.0, 700.0, 1000.0, 1400.0, 5000.0, 12_000.0] {
                let measured = measured_db(bands, rate, freq);
                #[allow(clippy::cast_possible_truncation)]
                let computed = response_db(&bands, rate, freq as f32);
                assert!(
                    (measured - computed).abs() < 0.05,
                    "{rate} Hz, {freq} Hz: measured {measured}, computed {computed}"
                );
            }
        }
        // Where the bands stand at their frequencies: a shelf's own frequency is half its gain.
        assert!((response_db(&bands, 48_000, 1000.0) + 9.0).abs() < 0.2);
        assert!((response_db(&bands, 48_000, 20.0) - 6.0).abs() < 0.2);
    }

    /// A bell at 1 kHz that takes 12 dB away above -30 dB.
    fn dynamic() -> Band {
        Band {
            kind: BandKind::Peak,
            freq_hz: 1000.0,
            gain_db: -12.0,
            q: 1.0,
            threshold_db: -30.0,
        }
    }

    /// A second of a sine of `freq` Hz at `amplitude` through an equalizer with band 0 set to
    /// `band`, in callbacks of 480 frames: what it did to the level of the last quarter in dB,
    /// and how much of its gain the band had at the end.
    fn heard(band: Band, freq: f32, amplitude: f32) -> (f32, f32) {
        let controls = EqControls::default();
        controls.set(0, band);
        let mut eq = Equalizer::new(RATE);
        let mut data: Vec<f32> = sine(freq, 1).iter().map(|s| s * amplitude * 2.0).collect();
        for callback in data.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        (
            level_db(&data) - 20.0 * (amplitude * 2.0).log10(),
            eq.amount[0],
        )
    }

    #[test]
    fn a_dynamic_band_has_its_gain_only_above_its_threshold() {
        // -6 dBFS is 24 dB above the threshold: all of the gain.
        let (loud, amount) = heard(dynamic(), 1000.0, 0.5);
        assert!((loud + 12.0).abs() < 0.3, "loud: {loud} dB");
        assert!((amount - 1.0).abs() < f32::EPSILON);
        // 6 dB above it: half of it.
        let (half, amount) = heard(dynamic(), 1000.0, 10f32.powf(-24.0 / 20.0));
        assert!((half + 6.0).abs() < 0.5, "half way: {half} dB");
        assert!((amount - 0.5).abs() < 0.05, "{amount}");
        // Below it: nothing, and the band is flat.
        let (quiet, amount) = heard(dynamic(), 1000.0, 10f32.powf(-40.0 / 20.0));
        assert!(quiet.abs() < 0.05, "quiet: {quiet} dB");
        assert!(amount.abs() < f32::EPSILON);
        // A boost only lifts what is there.
        let lift = Band {
            gain_db: 6.0,
            ..dynamic()
        };
        assert!((heard(lift, 1000.0, 0.5).0 - 6.0).abs() < 0.3);
        assert!(heard(lift, 1000.0, 0.01).0.abs() < 0.05);
    }

    #[test]
    fn a_dynamic_band_listens_in_its_own_range_only() {
        // A loud bass does not bring a bell at 4 kHz in.
        let bell = Band {
            freq_hz: 4000.0,
            q: 2.0,
            ..dynamic()
        };
        assert!(heard(bell, 100.0, 0.5).1.abs() < f32::EPSILON);
        assert!((heard(bell, 4000.0, 0.5).1 - 1.0).abs() < f32::EPSILON);
        // A high shelf listens above its frequency, a low shelf below.
        let high = Band {
            kind: BandKind::HighShelf,
            freq_hz: 6000.0,
            ..dynamic()
        };
        assert!(heard(high, 200.0, 0.5).1.abs() < f32::EPSILON);
        let (top, amount) = heard(high, 12_000.0, 0.5);
        assert!((amount - 1.0).abs() < f32::EPSILON);
        assert!(top < -9.0, "the top goes down: {top} dB");
        let low = Band {
            kind: BandKind::LowShelf,
            freq_hz: 200.0,
            ..dynamic()
        };
        assert!(heard(low, 5000.0, 0.5).1.abs() < f32::EPSILON);
        assert!((heard(low, 60.0, 0.5).1 - 1.0).abs() < f32::EPSILON);
    }

    #[test]
    fn a_dynamic_band_comes_in_and_lets_go_without_a_step() {
        let controls = EqControls::default();
        controls.set(0, dynamic());
        let mut eq = Equalizer::new(RATE);
        // Quiet, loud, quiet again, a second each; 1 kHz has whole waves in a second, so the
        // sound itself has no step where the level changes.
        let mut data = Vec::new();
        for amplitude in [0.005, 0.5, 0.005] {
            data.extend(sine(1000.0, 1).iter().map(|s| s * amplitude * 2.0));
        }
        let own = 0.5 * std::f32::consts::TAU * 1000.0 / RATE as f32;
        for callback in data.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        assert!(
            largest_step(&data) < own * 1.02,
            "{} against {own}",
            largest_step(&data)
        );
        // It was down in the loud second and is flat again at the end.
        let loud = &data[RATE as usize..2 * RATE as usize];
        assert!((level_db(loud) + 12.0).abs() < 0.3, "{}", level_db(loud));
        assert!(eq.amount[0].abs() < f32::EPSILON);
        #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
        let flat = eq.now[0].gain_db == 0.0;
        assert!(flat, "{:?}", eq.now[0]);
    }

    #[test]
    fn a_threshold_taken_away_makes_a_plain_band_again() {
        let controls = EqControls::default();
        controls.set(0, dynamic());
        let mut eq = Equalizer::new(RATE);
        let mut quiet: Vec<f32> = sine(1000.0, 1).iter().map(|s| s * 0.02).collect();
        for callback in quiet.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        assert!(eq.amount[0].abs() < f32::EPSILON);
        controls.set(
            0,
            Band {
                threshold_db: 0.0,
                ..dynamic()
            },
        );
        let mut quiet: Vec<f32> = sine(1000.0, 1).iter().map(|s| s * 0.02).collect();
        for callback in quiet.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        assert!((level_db(&quiet) - 20.0 * 0.02f32.log10() + 12.0).abs() < 0.3);
        // And a dynamic band of an equalizer that is off does nothing and lets it rest.
        controls.set(0, dynamic());
        controls.switch(false);
        let mut loud = sine(1000.0, 1);
        for callback in loud.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        assert!(level_db(&loud).abs() < 0.05);
        assert!(eq.rests());
    }

    #[test]
    fn a_band_that_listens_again_starts_from_what_it_hears_now() {
        let controls = EqControls::default();
        controls.set(0, dynamic());
        // Another band keeps the equalizer at work while the first one does not listen.
        controls.set(3, band(BandKind::HighShelf, 8000.0, 3.0));
        let mut eq = Equalizer::new(RATE);
        let mut loud = sine(1000.0, 1);
        for callback in loud.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        assert!((eq.amount[0] - 1.0).abs() < f32::EPSILON);
        // Its gain is taken away: nothing to bring in, so it does not listen.
        controls.set(
            0,
            Band {
                gain_db: 0.0,
                ..dynamic()
            },
        );
        let quiet = || -> Vec<f32> { sine(1000.0, 1).iter().map(|s| s * 0.01).collect() };
        let mut data = quiet();
        for callback in data.chunks_mut(480) {
            eq.process(&controls, callback, 1);
        }
        // Back again, on a quiet sound: the loud second from before does not bring it in.
        controls.set(0, dynamic());
        let mut data = quiet();
        eq.process(&controls, &mut data[..480], 1);
        assert!(eq.amount[0].abs() < f32::EPSILON, "{}", eq.amount[0]);
    }

    #[test]
    fn a_dynamic_band_listens_without_allocating_and_is_checked() {
        let controls = EqControls::default();
        controls.set(0, dynamic());
        let mut eq = Equalizer::new(RATE);
        let mut data = sine(1000.0, 2);
        assert_no_alloc::assert_no_alloc(|| {
            for callback in data.chunks_mut(480 * 2) {
                eq.process(&controls, callback, 2);
            }
        });
        assert!(dynamic().is_dynamic());
        assert!(!Band::at(1).is_dynamic());
        assert!(BandChange::Threshold(-60.0).checked().is_ok());
        let said = BandChange::Threshold(3.0).checked().unwrap_err();
        assert!(
            said.contains("threshold") && said.contains("-60 to 0 dB"),
            "{said}"
        );
        assert!(BandChange::Threshold(f32::NAN).checked().is_err());
        let changed = Band::at(1)
            .changed(BandChange::Threshold(-20.0))
            .expect("in range");
        assert!((changed.threshold_db + 20.0).abs() < f32::EPSILON);
        // A band from a state or a profile older than thresholds is a plain one.
        let old: Band =
            serde_json::from_str(r#"{"kind": "bell", "freq_hz": 500.0, "gain_db": 3.0, "q": 0.7}"#)
                .expect("an old band");
        assert!(!old.is_dynamic());
    }

    #[test]
    fn flat_bands_respond_with_nothing() {
        let flat = Band::all_flat();
        for freq in [20.0, 1000.0, 20_000.0] {
            assert_eq!(response_db(&flat, 48_000, freq), 0.0);
        }
    }
}
