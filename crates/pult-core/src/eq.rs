//! The parametric equalizer of a bus: a few bands, each one biquad filter.
//!
//! The control thread writes the bands into [`EqControls`] (atomics); the [`Equalizer`] in the
//! bus's callback follows them. Like every other change of the sound it does not jump: a gain
//! slides, a frequency and a width move by octaves, and a band changes its kind while it is
//! flat. A flat band costs nothing, it is skipped.
//!
//! All three kinds are flat at 0 dB, which is what makes switching a band or the whole
//! equalizer off a slide to 0 dB instead of a crossfade.

use std::f64::consts::TAU;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use crate::level::AtomicF32;

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

/// Frames between two steps of a moving band: short enough that a step is not heard.
const BLOCK: usize = 32;

/// How fast a gain slides, in dB per second: the whole range in 48 ms.
const GAIN_PACE: f32 = 1000.0;

/// How fast a frequency or a width moves, in octaves per second.
const OCTAVE_PACE: f32 = 100.0;

/// The highest frequency a band can stand at, as a part of the sample rate: below half of it,
/// where the filter is still well behaved.
const HIGHEST: f32 = 0.45;

/// What a band does to the sound.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub enum BandKind {
    /// Raises or lowers everything below its frequency.
    LowShelf,
    /// Raises or lowers a bell around its frequency.
    #[default]
    Peak,
    /// Raises or lowers everything above its frequency.
    HighShelf,
}

impl BandKind {
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
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Band {
    /// What it does.
    pub kind: BandKind,
    /// Its frequency in Hz.
    pub freq_hz: f32,
    /// Its gain in dB; 0 is flat.
    pub gain_db: f32,
    /// Its width: a higher Q is a narrower bell or a steeper shelf.
    pub q: f32,
}

impl Band {
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
        }
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
        }
    }
}

/// `value` inside `range`, or `fallback` for a NaN.
fn within(value: f32, range: (f32, f32), fallback: f32) -> f32 {
    if value.is_nan() {
        fallback
    } else {
        value.clamp(range.0, range.1)
    }
}

#[derive(Debug)]
struct BandControls {
    kind: AtomicU8,
    freq_hz: AtomicF32,
    gain_db: AtomicF32,
    q: AtomicF32,
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
        // After the values: a callback that saw half of them reads them again.
        self.version.fetch_add(1, Ordering::Release);
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

/// The filter of one band: `y = b0 x + b1 x1 + b2 x2 - a1 y1 - a2 y2`.
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
    /// What each filter remembers, per channel and band.
    memory: [[[f64; 2]; EQ_BANDS]; EQ_CHANNELS],
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
            memory: [[[0.0; 2]; EQ_BANDS]; EQ_CHANNELS],
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
            self.step(block.len() / channels);
            for band in 0..EQ_BANDS {
                #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
                if self.now[band].gain_db == 0.0 {
                    for memory in &mut self.memory {
                        memory[band] = [0.0; 2];
                    }
                    continue;
                }
                let filter = self.filters[band];
                for (channel, memory) in self.memory.iter_mut().enumerate().take(filtered) {
                    let [mut first, mut second] = memory[band];
                    for frame in block.chunks_exact_mut(channels) {
                        // A NaN would stay in the filter for ever; the bus silences it anyway.
                        let sample = frame[channel];
                        let x = if sample.is_finite() {
                            f64::from(sample)
                        } else {
                            0.0
                        };
                        let y = filter.b0 * x + first;
                        first = filter.b1 * x - filter.a1 * y + second;
                        second = filter.b2 * x - filter.a2 * y;
                        #[allow(clippy::cast_possible_truncation)]
                        let out = y as f32;
                        frame[channel] = out;
                    }
                    // What fades after the sound ends would become slow, tiny numbers.
                    memory[band] = [settled(first), settled(second)];
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
        if std::mem::take(&mut self.fresh) {
            self.now = std::array::from_fn(|index| self.goal(index));
            self.filters = self.now.map(|band| Filter::of(band, self.rate));
            self.memory = [[[0.0; 2]; EQ_BANDS]; EQ_CHANNELS];
        }
    }

    /// Where band `index` is to go: as set, or flat while the equalizer is off.
    fn goal(&self, index: usize) -> Band {
        let band = self.set[index];
        Band {
            gain_db: if self.on { band.gain_db } else { 0.0 },
            ..band
        }
    }

    /// Whether every band is flat and stays so: nothing to do.
    fn rests(&self) -> bool {
        #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
        (0..EQ_BANDS).all(|index| self.now[index].gain_db == 0.0 && self.goal(index).gain_db == 0.0)
    }

    /// Moves every band that is not where it is to be by `frames` frames' worth.
    fn step(&mut self, frames: usize) {
        let seconds = frames as f32 / self.rate;
        let octaves = 2f32.powf(OCTAVE_PACE * seconds);
        for index in 0..EQ_BANDS {
            let goal = self.goal(index);
            let now = &mut self.now[index];
            if *now == goal {
                continue;
            }
            #[allow(clippy::float_cmp)] // Exactly: a slide ends on its goal.
            if now.gain_db == 0.0 {
                // Flat sounds the same for every kind, frequency and width: change them for free.
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
            },
        );
        let set = controls.band(1).expect("band 1");
        assert_eq!((set.freq_hz, set.gain_db, set.q), (100.0, 0.0, 0.7));
        // Beyond the bands: nothing, and no panic.
        controls.set(EQ_BANDS, Band::at(0));
        assert_eq!(controls.band(EQ_BANDS), None);
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
}
