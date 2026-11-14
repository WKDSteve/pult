//! The host's side of a chain: reads the blocks the engine sent, works on them and sends them
//! back. In this spike the work is a gain, so that only the way between the processes is measured.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use crate::layout::{Geometry, WAKE_EDGES_US, word};
use crate::region::Region;
use crate::ring::Ring;

/// A host attached to the memory of one chain.
#[derive(Debug)]
pub struct Host {
    region: Arc<Region>,
    send: Ring,
    back: Ring,
    gain: f32,
    param_serial: u32,
    block: Vec<f32>,
}

impl Host {
    /// Attaches to `region`, drops what waits there from before, and says it is ready. `state`
    /// is what a host before it left behind (its gain); without one the engine's parameter
    /// counts, and 1 where none was set.
    ///
    /// # Errors
    ///
    /// When the header of the region does not say what `geometry` says.
    pub fn attach(
        region: Arc<Region>,
        geometry: Geometry,
        state: Option<f32>,
    ) -> Result<Self, String> {
        region.check(geometry)?;
        let send = Ring::new(Arc::clone(&region), geometry.send_base(), geometry)?;
        let back = Ring::new(Arc::clone(&region), geometry.return_base(), geometry)?;
        send.skip_all();
        let param_serial = region.get(word::PARAM_SERIAL);
        let gain = state
            .or_else(|| (param_serial > 0).then(|| f32::from_bits(region.get(word::PARAM))))
            .filter(|gain| gain.is_finite())
            .unwrap_or(1.0);
        region.set(word::HOST_STATE, gain.to_bits());
        region.set(word::HOST_READY, 1);
        Ok(Self {
            region,
            send,
            back,
            gain,
            param_serial,
            block: vec![0.0; geometry.samples()],
        })
    }

    /// Works through every block that waits and returns how many there were. `now_ns` is the
    /// time on the clock both processes read; how long each block waited goes into the header.
    pub fn work(&mut self, now_ns: u64) -> u32 {
        let serial = self.region.get(word::PARAM_SERIAL);
        if serial != self.param_serial {
            self.param_serial = serial;
            let gain = f32::from_bits(self.region.get(word::PARAM));
            if gain.is_finite() {
                self.gain = gain;
                self.region.set(word::HOST_STATE, gain.to_bits());
            }
        }
        let mut done = 0;
        while let Some((sequence, sent_ns)) = self.send.pop(&mut self.block) {
            for sample in &mut self.block {
                *sample *= self.gain;
            }
            // A full ring back means the engine does not read: the block is dropped.
            let _ = self.back.push(sequence, sent_ns, &self.block);
            self.region.bump(word::HOST_BEAT);
            self.note_wake(now_ns.saturating_sub(sent_ns) / 1000);
            done += 1;
        }
        done
    }

    /// Counts a block that waited `waited_us` before the host worked on it.
    fn note_wake(&self, waited_us: u64) {
        let bucket = WAKE_EDGES_US
            .iter()
            .position(|&edge| waited_us <= edge)
            .unwrap_or(WAKE_EDGES_US.len());
        self.region.bump(word::WAKE_BUCKET + bucket);
        let waited = u32::try_from(waited_us).unwrap_or(u32::MAX);
        self.region.words()[word::WAKE_MAX_US].fetch_max(waited, Ordering::AcqRel);
    }

    /// Whether the engine asked the host to end.
    #[must_use]
    pub fn stop_requested(&self) -> bool {
        self.region.get(word::STOP) != 0
    }

    /// The engine's block counter: while it moves, the engine is there.
    #[must_use]
    pub fn engine_beat(&self) -> u32 {
        self.region.get(word::ENGINE_BEAT)
    }
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // Samples scaled by a power of two compare exactly.
mod tests {
    use super::*;
    use crate::layout::WAKE_BUCKETS;

    fn region() -> (Arc<Region>, Geometry) {
        let geometry = Geometry::new(1, 16, 4).expect("sizes");
        let region = Arc::new(Region::local(geometry));
        region.stamp(geometry);
        (region, geometry)
    }

    #[test]
    fn a_host_refuses_a_region_that_is_not_laid_out_for_it() {
        let geometry = Geometry::new(1, 16, 4).expect("sizes");
        let region = Arc::new(Region::local(geometry));
        assert!(Host::attach(region, geometry, None).is_err());
    }

    #[test]
    fn a_host_applies_its_state_then_the_parameters_that_come_later() {
        let (region, geometry) = region();
        // A parameter from before the host started loses against its state.
        region.set(word::PARAM, 0.25_f32.to_bits());
        region.bump(word::PARAM_SERIAL);
        let send = Ring::new(Arc::clone(&region), geometry.send_base(), geometry).expect("fits");
        let back = Ring::new(Arc::clone(&region), geometry.return_base(), geometry).expect("fits");
        assert!(send.push(0, 0, &[1.0; 16]), "stale, dropped at the start");
        let mut host = Host::attach(Arc::clone(&region), geometry, Some(0.5)).expect("attaches");
        assert_eq!(region.get(word::HOST_READY), 1);
        assert_eq!(host.work(0), 0);
        assert!(send.push(1, 1_000_000, &[1.0; 16]));
        assert_eq!(host.work(1_300_000), 1);
        let mut out = [0.0; 16];
        assert_eq!(back.pop(&mut out), Some((1, 1_000_000)));
        assert!(out.iter().all(|&s| s == 0.5));
        assert_eq!(region.get(word::HOST_BEAT), 1);
        // It waited 300 µs: the third bucket.
        let buckets: Vec<u32> = (0..WAKE_BUCKETS)
            .map(|bucket| region.get(word::WAKE_BUCKET + bucket))
            .collect();
        assert_eq!(buckets, [0, 0, 1, 0, 0]);
        assert_eq!(region.get(word::WAKE_MAX_US), 300);
        // A new parameter, and one that is no number.
        region.set(word::PARAM, 2.0_f32.to_bits());
        region.bump(word::PARAM_SERIAL);
        assert!(send.push(2, 0, &[1.0; 16]));
        host.work(0);
        assert_eq!(back.pop(&mut out).map(|head| head.0), Some(2));
        assert!(out.iter().all(|&s| s == 2.0));
        assert_eq!(f32::from_bits(region.get(word::HOST_STATE)), 2.0);
        region.set(word::PARAM, f32::NAN.to_bits());
        region.bump(word::PARAM_SERIAL);
        host.work(0);
        assert_eq!(f32::from_bits(region.get(word::HOST_STATE)), 2.0);
    }

    #[test]
    fn a_host_without_a_state_takes_the_parameter_or_one() {
        let (region, geometry) = region();
        Host::attach(Arc::clone(&region), geometry, None).expect("attaches");
        assert_eq!(f32::from_bits(region.get(word::HOST_STATE)), 1.0);
        region.set(word::PARAM, 0.25_f32.to_bits());
        region.bump(word::PARAM_SERIAL);
        let host = Host::attach(Arc::clone(&region), geometry, None).expect("attaches");
        assert_eq!(f32::from_bits(region.get(word::HOST_STATE)), 0.25);
        assert!(!host.stop_requested());
        region.set(word::STOP, 1);
        assert!(host.stop_requested());
        assert_eq!(host.engine_beat(), 0);
    }
}
