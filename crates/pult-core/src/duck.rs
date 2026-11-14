//! Ducking: an input gets quieter while another one sounds, the music under the microphone.
//!
//! # Who does what
//!
//! Every input measures its own level once per block in its [`crate::stage::InputStage`], after
//! its gate and compressor, and stores it in the [`KeyBoard`] of the route: one cell per input,
//! atomics only. An input that ducks reads the cell of its key there, in its own stage and its
//! own thread, and turns itself down by its depth while the key is above the threshold. Neither
//! thread waits for the other; what the ducker sees of the key is at most one callback old.
//!
//! * The key is taken before the fader of its input and before its own ducker, so two inputs can
//!   duck each other without feeding back. A muted key ducks nothing.
//! * The ducker works before the rings, so the turn down travels with the sound through the
//!   buffering of every bus and meets the key there, both about one latency later.
//! * A key whose thread stops (its device is gone) stops beating; after [`STALE_MS`] without a
//!   beat its level counts as silence, so a lost microphone never holds the music down.
//!
//! The gain glides along the attack and the release, so nothing clicks, and nothing allocates.

use std::sync::atomic::{AtomicU32, Ordering};

use serde::{Deserialize, Serialize};

use crate::dynamics::pace;
use crate::level::{AtomicF32, db_to_linear};

/// How far a ducker turns down, in dB; 0 is off.
pub const DEPTH_DB: (f32, f32) = (0.0, 60.0);
/// The level of the key in dB above which it ducks.
pub const THRESHOLD_DB: (f32, f32) = (-70.0, 0.0);
/// Shortest and longest time it takes to turn down, in milliseconds (99 % of the way).
pub const ATTACK_MS: (f32, f32) = (1.0, 1000.0);
/// Shortest and longest time it stays down after the key fell silent, in milliseconds.
pub const HOLD_MS: (f32, f32) = (0.0, 5000.0);
/// Shortest and longest time it takes to come back, in milliseconds (99 % of the way).
pub const RELEASE_MS: (f32, f32) = (10.0, 5000.0);
/// The depth a ducker gets with its first key, when none was set: 12 dB, under speech.
pub const FIRST_DEPTH_DB: f32 = 12.0;
/// Milliseconds without a beat of the key after which its level counts as silence.
pub const STALE_MS: f32 = 250.0;

/// The key stays in this far below the level that ducks, so a level at the threshold does not
/// flutter.
const HYSTERESIS_DB: f32 = 3.0;

/// The times are the times after which the gain has all but arrived: 99 % of the way, which is
/// this many time constants (the natural logarithm of 100).
const ARRIVED: f32 = 4.605;

/// The settings of a ducker, as a view, a profile and the protocol carry them.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DuckSettings {
    /// The input whose sound ducks this one, by its number from 1 as in the addresses; 0 is none.
    pub key: usize,
    /// How far it turns down in dB; 0 is off.
    pub depth_db: f32,
    /// The level of the key in dB above which it ducks.
    pub threshold_db: f32,
    /// Milliseconds it takes to turn down.
    pub attack_ms: f32,
    /// Milliseconds it stays down after the key fell silent.
    pub hold_ms: f32,
    /// Milliseconds it takes to come back.
    pub release_ms: f32,
}

impl Default for DuckSettings {
    fn default() -> Self {
        Self {
            key: 0,
            depth_db: 0.0,
            threshold_db: -40.0,
            attack_ms: 60.0,
            hold_ms: 400.0,
            release_ms: 800.0,
        }
    }
}

impl DuckSettings {
    /// Whether it ducks at all: it has a key and a depth.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.key > 0 && self.depth_db > 0.0
    }

    /// Every setting as it starts.
    #[must_use]
    pub fn is_neutral(&self) -> bool {
        *self == Self::default()
    }

    /// Makes one change, unchecked ([`DuckChange::checked`] checks). A first key brings
    /// [`FIRST_DEPTH_DB`] along, so that naming it is enough to hear it.
    pub fn apply(&mut self, change: DuckChange) {
        match change {
            DuckChange::Key(key) => {
                self.key = key;
                if key > 0 && self.depth_db <= 0.0 {
                    self.depth_db = FIRST_DEPTH_DB;
                }
            }
            DuckChange::Depth(db) => self.depth_db = db,
            DuckChange::Threshold(db) => self.threshold_db = db,
            DuckChange::Attack(ms) => self.attack_ms = ms,
            DuckChange::Hold(ms) => self.hold_ms = ms,
            DuckChange::Release(ms) => self.release_ms = ms,
        }
    }

    /// Every setting, checked; the key only as a number ([`check_key`] knows the route).
    ///
    /// # Errors
    ///
    /// The message of the first one outside its range.
    pub fn checked(self) -> Result<Self, String> {
        DuckChange::Depth(self.depth_db).checked()?;
        DuckChange::Threshold(self.threshold_db).checked()?;
        DuckChange::Attack(self.attack_ms).checked()?;
        DuckChange::Hold(self.hold_ms).checked()?;
        DuckChange::Release(self.release_ms).checked()?;
        Ok(self)
    }

    /// What `show` says of it behind the rest of an input: nothing while it is off, else
    /// `  duck by [2] 12.0 dB`.
    #[must_use]
    pub fn summary(&self) -> String {
        if self.is_on() {
            format!("  duck by [{}] {:.1} dB", self.key, self.depth_db)
        } else {
            String::new()
        }
    }
}

/// Which setting of a ducker an address or a control names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DuckParam {
    /// Its depth in dB: `input/<n>/duck`.
    Depth,
    /// Its key, the number of an input: `input/<n>/duck/key`.
    Key,
    /// Its threshold in dB: `input/<n>/duck/threshold`.
    Threshold,
    /// Its attack in milliseconds: `input/<n>/duck/attack`.
    Attack,
    /// Its hold in milliseconds: `input/<n>/duck/hold`.
    Hold,
    /// Its release in milliseconds: `input/<n>/duck/release`.
    Release,
}

impl DuckParam {
    /// All of them, in the order of the addresses.
    pub const ALL: [Self; 6] = [
        Self::Depth,
        Self::Key,
        Self::Threshold,
        Self::Attack,
        Self::Hold,
        Self::Release,
    ];

    /// The words of its address behind `input/<n>/`.
    #[must_use]
    pub fn address(self) -> &'static str {
        match self {
            Self::Depth => "duck",
            Self::Key => "duck/key",
            Self::Threshold => "duck/threshold",
            Self::Attack => "duck/attack",
            Self::Hold => "duck/hold",
            Self::Release => "duck/release",
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
            Self::Depth => Some(DEPTH_DB),
            Self::Key => None,
            Self::Threshold => Some(THRESHOLD_DB),
            Self::Attack => Some(ATTACK_MS),
            Self::Hold => Some(HOLD_MS),
            Self::Release => Some(RELEASE_MS),
        }
    }

    /// The change that sets it to `number`; the key takes a whole number, anything else becomes
    /// a key no route has, which [`check_key`] refuses.
    #[must_use]
    pub fn change(self, number: f32) -> DuckChange {
        match self {
            Self::Depth => DuckChange::Depth(number),
            Self::Key => {
                let whole = number.is_finite() && number >= 0.0 && number.fract() == 0.0;
                #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                DuckChange::Key(if whole { number as usize } else { usize::MAX })
            }
            Self::Threshold => DuckChange::Threshold(number),
            Self::Attack => DuckChange::Attack(number),
            Self::Hold => DuckChange::Hold(number),
            Self::Release => DuckChange::Release(number),
        }
    }

    /// What `settings` has for it, as the change that would set it.
    #[must_use]
    pub fn read(self, settings: &DuckSettings) -> DuckChange {
        match self {
            Self::Depth => DuckChange::Depth(settings.depth_db),
            Self::Key => DuckChange::Key(settings.key),
            Self::Threshold => DuckChange::Threshold(settings.threshold_db),
            Self::Attack => DuckChange::Attack(settings.attack_ms),
            Self::Hold => DuckChange::Hold(settings.hold_ms),
            Self::Release => DuckChange::Release(settings.release_ms),
        }
    }
}

/// One change of a ducker.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DuckChange {
    /// The input that ducks it, by its number from 1; 0 takes the key away.
    Key(usize),
    /// How far it turns down in dB; 0 switches it off.
    Depth(f32),
    /// The level of the key in dB above which it ducks.
    Threshold(f32),
    /// Milliseconds it takes to turn down.
    Attack(f32),
    /// Milliseconds it stays down after the key fell silent.
    Hold(f32),
    /// Milliseconds it takes to come back.
    Release(f32),
}

impl DuckChange {
    /// Which setting it changes.
    #[must_use]
    pub fn param(self) -> DuckParam {
        match self {
            Self::Key(_) => DuckParam::Key,
            Self::Depth(_) => DuckParam::Depth,
            Self::Threshold(_) => DuckParam::Threshold,
            Self::Attack(_) => DuckParam::Attack,
            Self::Hold(_) => DuckParam::Hold,
            Self::Release(_) => DuckParam::Release,
        }
    }

    /// Its value as a number, the key as the number of its input.
    #[must_use]
    pub fn number(self) -> f32 {
        match self {
            #[allow(clippy::cast_precision_loss)]
            Self::Key(key) => key as f32,
            Self::Depth(n)
            | Self::Threshold(n)
            | Self::Attack(n)
            | Self::Hold(n)
            | Self::Release(n) => n,
        }
    }

    /// What it sets, as a reply says it: `ducked by [2]`, `duck off`, `duck depth 12.0 dB`.
    #[must_use]
    pub fn said(self) -> String {
        match self {
            Self::Key(0) => "duck off".to_owned(),
            Self::Key(key) => format!("ducked by [{key}]"),
            Self::Depth(db) if db <= 0.0 => "duck off (depth 0 dB)".to_owned(),
            Self::Depth(db) => format!("duck depth {db:.1} dB"),
            Self::Threshold(db) => format!("duck threshold {db:.1} dB"),
            Self::Attack(ms) => format!("duck attack {ms:.0} ms"),
            Self::Hold(ms) => format!("duck hold {ms:.0} ms"),
            Self::Release(ms) => format!("duck release {ms:.0} ms"),
        }
    }

    /// The change when its number is inside its range: nothing is clamped, as with a gain. A key
    /// passes here; whether the route has that input, [`check_key`] says.
    ///
    /// # Errors
    ///
    /// A message naming the range otherwise.
    pub fn checked(self) -> Result<Self, String> {
        let (what, unit) = match self {
            Self::Key(_) => return Ok(self),
            Self::Depth(_) => ("depth", " dB"),
            Self::Threshold(_) => ("threshold", " dB"),
            Self::Attack(_) => ("attack", " ms"),
            Self::Hold(_) => ("hold", " ms"),
            Self::Release(_) => ("release", " ms"),
        };
        let value = self.number();
        match self.param().range() {
            Some((low, high)) if !(low..=high).contains(&value) => Err(format!(
                "the duck {what} must be {low} to {high}{unit}, not {value}"
            )),
            _ => Ok(self),
        }
    }
}

/// `key` (a number from 1, 0 for none) as the key of input `input` (an index from 0) in a route
/// of `inputs` inputs.
///
/// # Errors
///
/// A message when the route has no such input, or the input would duck itself.
pub fn check_key(input: usize, key: usize, inputs: usize) -> Result<usize, String> {
    if key > inputs {
        Err(format!(
            "there is no input {key} to duck by (only {inputs})"
        ))
    } else if key == input + 1 {
        Err(format!("[{key}] cannot duck itself"))
    } else {
        Ok(key)
    }
}

/// The levels of all inputs of a route, each written by the stage of its input and read by the
/// duckers of the others.
#[derive(Debug)]
pub struct KeyBoard {
    cells: Box<[KeyCell]>,
}

#[derive(Debug, Default)]
struct KeyCell {
    /// The loudest sample of the latest block, linear, behind gate and compressor.
    level: AtomicF32,
    /// The same before gate and compressor: what a sidechain listens to
    /// ([`crate::sidechain`]).
    raw: AtomicF32,
    /// Counts the blocks: a reader that sees it stand knows the writer has stopped.
    beat: AtomicU32,
}

impl KeyBoard {
    /// A board for `inputs` inputs, all silent.
    #[must_use]
    pub fn new(inputs: usize) -> Self {
        Self {
            cells: (0..inputs).map(|_| KeyCell::default()).collect(),
        }
    }

    /// Number of inputs it has a cell for.
    #[must_use]
    pub fn len(&self) -> usize {
        self.cells.len()
    }

    /// Whether it has no cells.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.cells.is_empty()
    }

    /// Stores the level of input `index` (from 0) for one more block; the stage of that input
    /// does. An index the board does not have is ignored.
    pub fn store(&self, index: usize, level: f32) {
        if let Some(cell) = self.cells.get(index) {
            cell.level
                .store(if level.is_finite() { level } else { 0.0 });
            cell.beat.fetch_add(1, Ordering::Relaxed);
        }
    }

    /// Stores the level of input `index` before its gate and compressor, for the same block as
    /// the next [`Self::store`], which beats for both.
    pub fn store_raw(&self, index: usize, level: f32) {
        if let Some(cell) = self.cells.get(index) {
            cell.raw.store(if level.is_finite() { level } else { 0.0 });
        }
    }

    /// The level of input `index` (from 0) and its beat; `None` when the board has no such cell.
    #[must_use]
    pub fn read(&self, index: usize) -> Option<(f32, u32)> {
        let cell = self.cells.get(index)?;
        Some((cell.level.load(), cell.beat.load(Ordering::Relaxed)))
    }

    /// As [`Self::read`], with the level before gate and compressor.
    #[must_use]
    pub fn read_raw(&self, index: usize) -> Option<(f32, u32)> {
        let cell = self.cells.get(index)?;
        Some((cell.raw.load(), cell.beat.load(Ordering::Relaxed)))
    }
}

/// Watches one key on a board from the thread of another input: its level, and silence once it
/// no longer beats.
#[derive(Debug)]
pub struct KeyWatch {
    /// Own frames after which a key without a beat counts as silent.
    stale_frames: u32,
    /// The beat of the key as last seen, and own frames since it moved.
    seen_beat: Option<u32>,
    unmoved: u32,
}

impl KeyWatch {
    /// A watch in a thread that runs at `rate` frames per second.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        #[allow(clippy::cast_precision_loss)]
        let rate = rate.max(1) as f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let stale_frames = (STALE_MS * rate / 1000.0) as u32;
        Self {
            stale_frames,
            seen_beat: None,
            unmoved: 0,
        }
    }

    /// Forgets what it saw: for another key, or a new stream.
    pub fn start_over(&mut self) {
        self.seen_beat = None;
        self.unmoved = 0;
    }

    /// The level of `key` (a number from 1) now, `frames` own frames after the last look: before
    /// its gate and compressor when `raw`, else behind them. Silence without a key, for a key
    /// the board does not have and for one that has not beaten for [`STALE_MS`].
    pub fn level(&mut self, board: &KeyBoard, key: usize, frames: usize, raw: bool) -> f32 {
        let read = |index| {
            if raw {
                board.read_raw(index)
            } else {
                board.read(index)
            }
        };
        let Some((level, beat)) = key.checked_sub(1).and_then(read) else {
            return 0.0;
        };
        if self.seen_beat == Some(beat) {
            let frames = u32::try_from(frames).unwrap_or(u32::MAX);
            self.unmoved = self.unmoved.saturating_add(frames);
        } else {
            self.seen_beat = Some(beat);
            self.unmoved = 0;
        }
        if self.unmoved > self.stale_frames {
            0.0
        } else {
            level
        }
    }
}

/// The ducker of one input as the control thread sets it; off at first.
#[derive(Debug)]
pub struct DuckControls {
    key: AtomicU32,
    depth_db: AtomicF32,
    threshold_db: AtomicF32,
    attack_ms: AtomicF32,
    hold_ms: AtomicF32,
    release_ms: AtomicF32,
}

impl Default for DuckControls {
    fn default() -> Self {
        let settings = DuckSettings::default();
        Self {
            key: AtomicU32::new(0),
            depth_db: AtomicF32::new(settings.depth_db),
            threshold_db: AtomicF32::new(settings.threshold_db),
            attack_ms: AtomicF32::new(settings.attack_ms),
            hold_ms: AtomicF32::new(settings.hold_ms),
            release_ms: AtomicF32::new(settings.release_ms),
        }
    }
}

impl DuckControls {
    /// Sets everything at once; a number outside its range is clamped (check before with
    /// [`DuckSettings::checked`]), one that is no number leaves its setting as it is.
    pub fn set_all(&self, settings: DuckSettings) {
        let store = |cell: &AtomicF32, value: f32, (low, high): (f32, f32)| {
            if value.is_finite() {
                cell.store(value.clamp(low, high));
            }
        };
        self.key
            .store(u32::try_from(settings.key).unwrap_or(0), Ordering::Relaxed);
        store(&self.depth_db, settings.depth_db, DEPTH_DB);
        store(&self.threshold_db, settings.threshold_db, THRESHOLD_DB);
        store(&self.attack_ms, settings.attack_ms, ATTACK_MS);
        store(&self.hold_ms, settings.hold_ms, HOLD_MS);
        store(&self.release_ms, settings.release_ms, RELEASE_MS);
    }

    /// What it is set to now.
    #[must_use]
    pub fn settings(&self) -> DuckSettings {
        DuckSettings {
            key: self.key.load(Ordering::Relaxed) as usize,
            depth_db: self.depth_db.load(),
            threshold_db: self.threshold_db.load(),
            attack_ms: self.attack_ms.load(),
            hold_ms: self.hold_ms.load(),
            release_ms: self.release_ms.load(),
        }
    }
}

/// The ducker inside the stage of one input.
#[derive(Debug)]
pub struct Ducker {
    rate: f32,
    settings: DuckSettings,
    /// Linear level of the key that ducks, and the lower one that keeps it ducked.
    ducks_at: f32,
    stays_above: f32,
    /// Linear gain while ducked.
    floor: f64,
    attack: f64,
    release: f64,
    hold_frames: u32,
    /// Its look at the key: silent once that no longer beats.
    watch: KeyWatch,
    /// Whether the key is above the threshold.
    keyed: bool,
    /// Frames it still stays down after the key fell silent.
    hold: u32,
    /// Where its gain stands.
    gain: f64,
}

impl Ducker {
    /// A ducker at `rate` frames per second, off.
    #[must_use]
    pub fn new(rate: u32) -> Self {
        let watch = KeyWatch::new(rate);
        #[allow(clippy::cast_precision_loss)]
        let rate = rate.max(1) as f32;
        let mut ducker = Self {
            rate,
            settings: DuckSettings::default(),
            ducks_at: 0.0,
            stays_above: 0.0,
            floor: 1.0,
            attack: 0.0,
            release: 0.0,
            hold_frames: 0,
            watch,
            keyed: false,
            hold: 0,
            gain: 1.0,
        };
        ducker.configure(DuckSettings::default());
        ducker
    }

    fn configure(&mut self, settings: DuckSettings) {
        if settings.key != self.settings.key {
            // Another key: what was seen of the old one says nothing about it.
            self.watch.start_over();
            self.keyed = false;
        }
        self.settings = settings;
        self.ducks_at = db_to_linear(settings.threshold_db);
        self.stays_above = db_to_linear(settings.threshold_db - HYSTERESIS_DB);
        self.floor = if settings.is_on() {
            f64::from(db_to_linear(-settings.depth_db))
        } else {
            1.0
        };
        self.attack = pace(settings.attack_ms / ARRIVED, self.rate);
        self.release = pace(settings.release_ms / ARRIVED, self.rate);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let hold_frames = (settings.hold_ms * self.rate / 1000.0) as u32;
        self.hold_frames = hold_frames;
    }

    /// Lets go at once, as after a new stream: what played before does not hold it down.
    pub fn start_over(&mut self) {
        self.watch.start_over();
        self.keyed = false;
        self.hold = 0;
        self.gain = 1.0;
    }

    /// How many dB it takes away now (0 while the key is silent or it is off).
    #[must_use]
    pub fn reduction_db(&self) -> f32 {
        if self.gain >= 1.0 {
            return 0.0;
        }
        #[allow(clippy::cast_possible_truncation)]
        let gain = self.gain as f32;
        -20.0 * gain.max(1e-5).log10()
    }

    /// Ducks `data` (interleaved, `channels` per frame) in place as `controls` and the level of
    /// the key on `board` say.
    pub fn process(
        &mut self,
        controls: &DuckControls,
        board: &KeyBoard,
        data: &mut [f32],
        channels: usize,
    ) {
        if channels == 0 {
            return;
        }
        let settings = controls.settings();
        if settings != self.settings {
            self.configure(settings);
        }
        #[allow(clippy::float_cmp)] // Exactly: it is set to unity when it arrives there.
        if !self.settings.is_on() && self.gain == 1.0 {
            return;
        }
        let level = self
            .watch
            .level(board, self.settings.key, data.len() / channels, false);
        let threshold = if self.keyed {
            self.stays_above
        } else {
            self.ducks_at
        };
        self.keyed = self.settings.is_on() && level >= threshold;
        if self.keyed {
            self.hold = self.hold_frames;
        }
        for frame in data.chunks_exact_mut(channels) {
            let down = self.keyed || self.hold > 0;
            if !self.keyed && self.hold > 0 {
                self.hold -= 1;
            }
            let goal = if down && self.settings.is_on() {
                self.floor
            } else {
                1.0
            };
            let pace = if goal < self.gain {
                self.attack
            } else {
                self.release
            };
            self.gain += (goal - self.gain) * pace;
            // Arrived, as far as anyone hears: stand there, and cost nothing once back at unity.
            if (goal - self.gain).abs() < 1e-6 {
                self.gain = goal;
            }
            #[allow(clippy::cast_possible_truncation)]
            let gain = self.gain as f32;
            for sample in frame {
                *sample *= gain;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 48_000;
    const BLOCK: usize = 480;

    fn on(key: usize, depth_db: f32) -> DuckSettings {
        DuckSettings {
            key,
            depth_db,
            ..DuckSettings::default()
        }
    }

    /// Runs `blocks` blocks of a constant 0.5 through `ducker` while the key at index 0 beats
    /// with `key_level`; returns the last sample.
    fn run(
        ducker: &mut Ducker,
        controls: &DuckControls,
        board: &KeyBoard,
        key_level: Option<f32>,
        blocks: usize,
    ) -> f32 {
        let mut data = vec![0.5_f32; BLOCK];
        for _ in 0..blocks {
            if let Some(level) = key_level {
                board.store(0, level);
            }
            data.fill(0.5);
            ducker.process(controls, board, &mut data, 1);
        }
        data[BLOCK - 1]
    }

    #[test]
    fn a_loud_key_turns_it_down_by_its_depth_and_silence_lets_it_back() {
        let board = KeyBoard::new(2);
        let controls = DuckControls::default();
        controls.set_all(on(1, 12.0));
        let mut ducker = Ducker::new(RATE);
        // One second of the key at -20 dB, above the threshold of -40.
        let down = run(&mut ducker, &controls, &board, Some(0.1), 100);
        assert!((down - 0.5 * db_to_linear(-12.0)).abs() < 1e-4, "{down}");
        assert!((ducker.reduction_db() - 12.0).abs() < 0.01);
        // The key falls silent: hold (400 ms), then the release (800 ms).
        let held = run(&mut ducker, &controls, &board, Some(0.0), 30);
        assert!(
            (held - down).abs() < 1e-4,
            "still down within the hold: {held}"
        );
        let back = run(&mut ducker, &controls, &board, Some(0.0), 300);
        assert!((back - 0.5).abs() < 1e-6, "{back}");
        assert_eq!(ducker.reduction_db(), 0.0);
    }

    #[test]
    fn a_key_below_the_threshold_ducks_nothing() {
        let board = KeyBoard::new(2);
        let controls = DuckControls::default();
        controls.set_all(on(1, 20.0));
        let mut ducker = Ducker::new(RATE);
        // -46 dB under a threshold of -40 dB.
        let out = run(&mut ducker, &controls, &board, Some(0.005), 100);
        assert!((out - 0.5).abs() < 1e-6, "{out}");
    }

    #[test]
    fn the_gain_glides_and_never_steps() {
        let board = KeyBoard::new(2);
        let controls = DuckControls::default();
        controls.set_all(on(1, 40.0));
        let mut ducker = Ducker::new(RATE);
        let mut last = 0.5_f32;
        let mut largest = 0.0_f32;
        for block in 0..300 {
            // Loud for a second, then silent.
            board.store(0, if block < 100 { 0.5 } else { 0.0 });
            let mut data = vec![0.5_f32; BLOCK];
            ducker.process(&controls, &board, &mut data, 1);
            for sample in data {
                largest = largest.max((sample - last).abs());
                last = sample;
            }
        }
        // 40 dB within 60 ms: the steepest step is far below what clicks.
        assert!(largest < 2e-3, "largest step {largest}");
    }

    #[test]
    fn a_key_that_stops_beating_counts_as_silent() {
        let board = KeyBoard::new(2);
        let controls = DuckControls::default();
        controls.set_all(DuckSettings {
            hold_ms: 0.0,
            release_ms: 10.0,
            ..on(1, 12.0)
        });
        let mut ducker = Ducker::new(RATE);
        let down = run(&mut ducker, &controls, &board, Some(0.3), 50);
        assert!(down < 0.2, "{down}");
        // Its thread is gone: the loud level stays on the board, without a beat.
        let soon = run(&mut ducker, &controls, &board, None, 20);
        assert!(
            soon < 0.2,
            "200 ms without a beat are not stale yet: {soon}"
        );
        let later = run(&mut ducker, &controls, &board, None, 40);
        assert!((later - 0.5).abs() < 1e-6, "{later}");
        // Back again, it ducks again.
        let again = run(&mut ducker, &controls, &board, Some(0.3), 50);
        assert!(again < 0.2, "{again}");
    }

    #[test]
    fn without_a_key_or_a_depth_it_is_off_and_lets_go() {
        let board = KeyBoard::new(2);
        let controls = DuckControls::default();
        controls.set_all(on(1, 12.0));
        let mut ducker = Ducker::new(RATE);
        run(&mut ducker, &controls, &board, Some(0.3), 100);
        // The key is taken away while it is down: it comes back along its release.
        controls.set_all(on(0, 12.0));
        let back = run(&mut ducker, &controls, &board, Some(0.3), 300);
        assert!((back - 0.5).abs() < 1e-6, "{back}");
        controls.set_all(on(1, 0.0));
        let off = run(&mut ducker, &controls, &board, Some(0.3), 50);
        assert!((off - 0.5).abs() < 1e-6, "{off}");
        // A key the board does not have is silence.
        controls.set_all(on(7, 12.0));
        let none = run(&mut ducker, &controls, &board, Some(0.3), 50);
        assert!((none - 0.5).abs() < 1e-6, "{none}");
    }

    #[test]
    fn a_broken_level_is_silence_and_all_channels_turn_alike() {
        let board = KeyBoard::new(1);
        board.store(0, f32::NAN);
        assert_eq!(board.read(0), Some((0.0, 1)));
        assert_eq!(board.read(1), None);
        board.store(5, 1.0);
        let controls = DuckControls::default();
        controls.set_all(on(1, 6.0));
        let mut ducker = Ducker::new(RATE);
        let mut data = [0.4_f32, -0.2].repeat(BLOCK);
        for _ in 0..100 {
            board.store(0, 0.5);
            data = [0.4_f32, -0.2].repeat(BLOCK);
            ducker.process(&controls, &board, &mut data, 2);
        }
        let gain = db_to_linear(-6.0);
        assert!((data[2 * BLOCK - 2] - 0.4 * gain).abs() < 1e-4);
        assert!((data[2 * BLOCK - 1] + 0.2 * gain).abs() < 1e-4);
    }

    #[test]
    fn the_ducker_does_not_allocate() {
        let board = KeyBoard::new(2);
        let controls = DuckControls::default();
        controls.set_all(on(1, 12.0));
        let mut ducker = Ducker::new(RATE);
        let mut data = vec![0.5_f32; BLOCK * 2];
        assert_no_alloc::assert_no_alloc(|| {
            for block in 0..20 {
                board.store(0, if block % 2 == 0 { 0.5 } else { 0.0 });
                ducker.process(&controls, &board, &mut data, 2);
            }
            controls.set_all(on(0, 0.0));
            ducker.process(&controls, &board, &mut data, 2);
            ducker.start_over();
        });
    }

    #[test]
    fn changes_are_checked_and_said() {
        assert!(DuckChange::Depth(61.0).checked().is_err());
        assert!(DuckChange::Depth(f32::NAN).checked().is_err());
        assert!(DuckChange::Threshold(-71.0).checked().is_err());
        assert!(DuckChange::Attack(0.5).checked().is_err());
        assert!(DuckChange::Hold(5000.0).checked().is_ok());
        assert!(DuckChange::Release(5.0).checked().is_err());
        assert_eq!(DuckChange::Key(2).said(), "ducked by [2]");
        assert_eq!(DuckChange::Key(0).said(), "duck off");
        assert_eq!(DuckChange::Depth(12.0).said(), "duck depth 12.0 dB");
        assert_eq!(check_key(0, 2, 3), Ok(2));
        assert_eq!(check_key(0, 0, 3), Ok(0));
        assert!(check_key(1, 2, 3).unwrap_err().contains("itself"));
        assert!(check_key(0, 4, 3).unwrap_err().contains("no input 4"));
        // A key that is no whole number names no input of any route.
        assert!(matches!(
            DuckParam::Key.change(1.5),
            DuckChange::Key(usize::MAX)
        ));
        assert_eq!(DuckParam::Key.change(2.0), DuckChange::Key(2));
    }

    #[test]
    fn a_first_key_brings_a_depth_and_addresses_go_both_ways() {
        let mut settings = DuckSettings::default();
        assert!(!settings.is_on());
        settings.apply(DuckChange::Key(2));
        assert_eq!(settings.depth_db, FIRST_DEPTH_DB);
        assert!(settings.is_on());
        assert_eq!(settings.summary(), "  duck by [2] 12.0 dB");
        settings.apply(DuckChange::Depth(20.0));
        settings.apply(DuckChange::Key(3));
        assert_eq!(settings.depth_db, 20.0, "a depth that was set stays");
        for param in DuckParam::ALL {
            assert_eq!(DuckParam::from_address(param.address()), Some(param));
            assert_eq!(param.read(&settings).param(), param);
            let number = param.read(&settings).number();
            assert_eq!(param.change(number), param.read(&settings));
        }
        assert_eq!(DuckParam::from_address("duck/depth"), None);
    }
}
