//! The automixer: of several microphones, the ones that are spoken into are open, the others
//! lowered, and the sum stays as loud as one open microphone.
//!
//! # Gain sharing
//!
//! The principle Dan Dugan published for his automixer (the one in Yamaha's RIVAGE PM10 and,
//! in its own form, Lawo's AutoMix): every microphone of a group gets the share of the total
//! power that is its own. With powers `p_i`, microphone `i` gets the gain `sqrt(p_i / sum p)`.
//!
//! * One talker: nearly all the power is his, his gain is about 0 dB, the others are far down.
//! * Two at once: each gets about -3 dB; the sum of the powers stays the same.
//! * Silence: all share alike, `1 / N` of the power each, so the room sounds as through one
//!   microphone (the "number of open microphones" attenuation), and nothing pumps.
//!
//! No thresholds, no gate: the gains glide with the levels. A small floor in the power keeps
//! silence equal and the division defined.
//!
//! # Who does what
//!
//! The level is measured once per input in its [`crate::stage::InputStage`], after gate and
//! compressor, by a [`PowerFollower`], and stored in the group; every ring of the input reads
//! the shares of the group when it mixes and glides to its own over the block ([`crate::mixer`]).
//! The group is atomics only: the stages run in the device threads, the rings in the output
//! threads, and neither waits for the other.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::level::AtomicF32;

/// Power added to every member, about -70 dBFS: silence shares alike, and the division is
/// defined.
pub const FLOOR_POWER: f32 = 1e-7;

/// Seconds in which the measured power follows a step to about two thirds.
pub const FOLLOW_SECONDS: f32 = 0.01;

/// One microphone of a group.
#[derive(Debug)]
pub struct AutomixMember {
    power: AtomicF32,
    on: AtomicBool,
    /// Its priority as a factor on its power (1: none), as Dugan's weight.
    weight: AtomicF32,
}

impl AutomixMember {
    /// Its measured power (mean square), as its stage stored it last.
    #[must_use]
    pub fn power(&self) -> f32 {
        self.power.load()
    }

    /// Stores the measured power; the stage of the input does.
    pub fn store_power(&self, power: f32) {
        self.power.store(power);
    }

    /// Whether it takes part: a member that is off keeps its gain and counts for no share.
    #[must_use]
    pub fn is_on(&self) -> bool {
        self.on.load(Ordering::Relaxed)
    }

    /// Takes it into the sharing or out of it.
    pub fn set_on(&self, on: bool) {
        self.on.store(on, Ordering::Relaxed);
    }

    /// Its priority in dB, -12 to 12 (0: none): a microphone weighted up by 6 dB counts as if it
    /// were 6 dB louder when the gain is shared, so it opens first, and stays open against a
    /// louder neighbour (a host against a guest).
    pub fn set_weight_db(&self, db: f32) {
        let db = if db.is_finite() {
            db.clamp(-12.0, 12.0)
        } else {
            0.0
        };
        self.weight.store(10_f32.powf(db / 10.0));
    }
}

/// The microphones that share one gain.
#[derive(Debug)]
pub struct AutomixGroup {
    members: Box<[AutomixMember]>,
}

impl AutomixGroup {
    /// A group of `count` members, all on, all silent.
    #[must_use]
    pub fn new(count: usize) -> Self {
        Self {
            members: (0..count)
                .map(|_| AutomixMember {
                    power: AtomicF32::new(0.0),
                    on: AtomicBool::new(true),
                    weight: AtomicF32::new(1.0),
                })
                .collect(),
        }
    }

    /// Number of members.
    #[must_use]
    pub fn len(&self) -> usize {
        self.members.len()
    }

    /// Whether it has no members.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.members.is_empty()
    }

    /// Member `index`.
    ///
    /// # Panics
    ///
    /// When `index` is not below [`Self::len`].
    #[must_use]
    pub fn member(&self, index: usize) -> &AutomixMember {
        &self.members[index]
    }

    /// The gain of member `index` now: the root of its share of the power of the members that
    /// are on; 1 for a member that is off (or not in the group).
    #[must_use]
    pub fn gain(&self, index: usize) -> f32 {
        let Some(member) = self.members.get(index).filter(|m| m.is_on()) else {
            return 1.0;
        };
        let level = |m: &AutomixMember| {
            let p = m.power();
            // A broken measurement counts as silence, never as a share of everything.
            let level = if p.is_finite() && p > 0.0 {
                p + FLOOR_POWER
            } else {
                FLOOR_POWER
            };
            level * m.weight.load()
        };
        let total: f32 = self.members.iter().filter(|m| m.is_on()).map(level).sum();
        (level(member) / total).sqrt()
    }
}

/// The mean square of a signal, followed with the time constant [`FOLLOW_SECONDS`].
#[derive(Debug, Clone)]
pub struct PowerFollower {
    rate: f32,
    value: f32,
}

impl PowerFollower {
    /// A follower for a signal at `rate` Hz, starting at silence.
    #[must_use]
    #[expect(
        clippy::cast_precision_loss,
        reason = "a sample rate fits an f32 exactly"
    )]
    pub fn new(rate: u32) -> Self {
        Self {
            rate: rate as f32,
            value: 0.0,
        }
    }

    /// Takes in one block of `channels` interleaved and returns the power now.
    #[expect(
        clippy::cast_precision_loss,
        reason = "a block length fits an f32 well enough"
    )]
    pub fn follow(&mut self, block: &[f32], channels: usize) -> f32 {
        if block.is_empty() || channels == 0 {
            return self.value;
        }
        let square: f32 = block.iter().map(|s| s * s).sum::<f32>() / block.len() as f32;
        let frames = (block.len() / channels) as f32;
        let step = 1.0 - (-frames / (FOLLOW_SECONDS * self.rate)).exp();
        if square.is_finite() {
            self.value += step * (square - self.value);
        }
        self.value
    }

    /// Back to silence, as after a new stream.
    pub fn start_over(&mut self) {
        self.value = 0.0;
    }
}

/// Multiplies `block` (interleaved, `channels`) by a gain that goes linearly from `from` to
/// `to` over its frames, so a change of the share never steps.
pub fn glide_gain(block: &mut [f32], channels: usize, from: f32, to: f32) {
    if channels == 0 || block.is_empty() {
        return;
    }
    if (from - 1.0).abs() < f32::EPSILON && (to - 1.0).abs() < f32::EPSILON {
        return;
    }
    let frames = block.len() / channels;
    #[expect(
        clippy::cast_precision_loss,
        reason = "a block length fits an f32 well enough"
    )]
    let step = (to - from) / frames.max(1) as f32;
    let mut gain = from;
    for frame in block.chunks_exact_mut(channels) {
        gain += step;
        for sample in frame {
            *sample *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db(gain: f32) -> f32 {
        20.0 * gain.log10()
    }

    #[test]
    fn the_talker_is_open_and_the_others_are_down() {
        let group = AutomixGroup::new(3);
        group.member(0).store_power(0.01); // -20 dBFS speech
        group.member(1).store_power(1e-6); // room
        group.member(2).store_power(1e-6);
        assert!(db(group.gain(0)) > -0.01, "{}", db(group.gain(0)));
        assert!(db(group.gain(1)) < -38.0, "{}", db(group.gain(1)));
    }

    #[test]
    fn two_talkers_share_and_the_power_stays_the_same() {
        let group = AutomixGroup::new(4);
        group.member(0).store_power(0.01);
        group.member(1).store_power(0.01);
        let total: f32 = (0..4).map(|i| group.gain(i).powi(2)).sum();
        assert!((total - 1.0).abs() < 1e-5, "{total}");
        assert!(
            (db(group.gain(0)) + 3.01).abs() < 0.05,
            "{}",
            db(group.gain(0))
        );
    }

    #[test]
    fn silence_shares_alike_as_one_open_microphone() {
        let group = AutomixGroup::new(4);
        for i in 0..4 {
            assert!(
                (db(group.gain(i)) + 6.02).abs() < 0.01,
                "{}",
                db(group.gain(i))
            );
        }
    }

    #[test]
    fn a_member_that_is_off_keeps_its_gain_and_takes_no_share() {
        let group = AutomixGroup::new(2);
        group.member(0).store_power(0.01);
        group.member(1).store_power(0.01);
        group.member(1).set_on(false);
        assert_eq!(group.gain(1), 1.0);
        assert!(db(group.gain(0)) > -0.01, "alone in the sharing");
        assert_eq!(group.gain(7), 1.0, "not in the group");
    }

    #[test]
    fn a_weighted_microphone_counts_as_louder() {
        let group = AutomixGroup::new(2);
        group.member(0).store_power(0.01);
        group.member(1).store_power(0.01);
        group.member(0).set_weight_db(6.0);
        // Weighted by 6 dB, four times the power: 80 % of the share, -1 dB against -7 dB.
        assert!(
            (db(group.gain(0)) + 0.97).abs() < 0.05,
            "{}",
            db(group.gain(0))
        );
        assert!(
            (db(group.gain(1)) + 6.99).abs() < 0.05,
            "{}",
            db(group.gain(1))
        );
        group.member(0).set_weight_db(f32::NAN);
        assert!(
            (db(group.gain(0)) + 3.01).abs() < 0.05,
            "a weight that is no number is none"
        );
    }

    #[test]
    fn a_broken_level_is_silence() {
        let group = AutomixGroup::new(2);
        group.member(0).store_power(f32::NAN);
        group.member(1).store_power(f32::INFINITY);
        let (a, b) = (group.gain(0), group.gain(1));
        assert!(a.is_finite() && b.is_finite());
        assert!((a - b).abs() < 1e-6);
    }

    #[test]
    fn the_follower_reaches_a_step_in_its_time() {
        let mut f = PowerFollower::new(48_000);
        let block = vec![0.5_f32; 48]; // one ms, mono
        let mut p = 0.0;
        for _ in 0..10 {
            p = f.follow(&block, 1);
        }
        // After one time constant about two thirds of 0.25.
        assert!((p / 0.25 - (1.0 - (-1.0_f32).exp())).abs() < 0.01, "{p}");
        for _ in 0..200 {
            p = f.follow(&block, 1);
        }
        assert!((p - 0.25).abs() < 1e-4);
        f.follow(&[f32::NAN; 48], 1);
        assert!(
            (f.follow(&block, 1) - 0.25).abs() < 1e-4,
            "a NaN block is ignored"
        );
    }

    #[test]
    fn a_glide_has_no_step() {
        let mut block = vec![1.0_f32; 8];
        glide_gain(&mut block, 2, 1.0, 0.0);
        assert_eq!(block, [0.75, 0.75, 0.5, 0.5, 0.25, 0.25, 0.0, 0.0]);
        let mut same = vec![0.3_f32; 4];
        glide_gain(&mut same, 1, 1.0, 1.0);
        assert_eq!(same, [0.3; 4]);
    }
}
