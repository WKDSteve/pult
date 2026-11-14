//! A ring of blocks in a [`Region`], for one writer and one reader that may live in two processes.
//!
//! The writer owns the write index, the reader the read index; both count blocks and wrap with the
//! word. A block is there for the reader once the write index has passed it. Neither side trusts
//! the index of the other: a slot is always the index modulo the ring's size, which each side
//! knows from its own [`Geometry`], and a distance between the two that a ring cannot have reads
//! as "full" for the writer and as "empty, start over" for the reader. So a peer gone wrong can
//! spoil the sound of its own chain, and nothing else.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::layout::{BLOCK_HEAD, Geometry, RING_HEAD};
use crate::region::Region;

/// One of the two rings of a region, as one of its two sides uses it.
#[derive(Debug, Clone)]
pub struct Ring {
    region: Arc<Region>,
    base: usize,
    samples: usize,
    block_words: usize,
    blocks: u32,
}

impl Ring {
    /// The ring that starts at word `base` of `region`.
    ///
    /// # Errors
    ///
    /// When the region is too small to hold it there.
    pub fn new(region: Arc<Region>, base: usize, geometry: Geometry) -> Result<Self, String> {
        let end = base + geometry.ring_words();
        if region.words().len() < end {
            return Err(format!(
                "a ring up to word {end} in a region of {} words",
                region.words().len()
            ));
        }
        Ok(Self {
            region,
            base,
            samples: geometry.samples(),
            block_words: geometry.block_words(),
            // Within [`crate::layout::BLOCKS`], so it fits.
            blocks: u32::try_from(geometry.blocks).unwrap_or(1),
        })
    }

    fn words(&self) -> &[AtomicU32] {
        self.region.words()
    }

    fn write_index(&self) -> &AtomicU32 {
        &self.words()[self.base]
    }

    fn read_index(&self) -> &AtomicU32 {
        &self.words()[self.base + 1]
    }

    /// The words of the block at `index`.
    fn slot(&self, index: u32) -> &[AtomicU32] {
        let start = self.base + RING_HEAD + (index % self.blocks) as usize * self.block_words;
        &self.words()[start..start + self.block_words]
    }

    /// Blocks written and not read yet; never more than the ring holds.
    #[must_use]
    pub fn waiting(&self) -> u32 {
        let written = self.write_index().load(Ordering::Acquire);
        let read = self.read_index().load(Ordering::Acquire);
        written.wrapping_sub(read).min(self.blocks)
    }

    /// Writes one block; only the writer calls it. False, with nothing written, when the ring is
    /// full or `samples` is not one block.
    #[must_use]
    pub fn push(&self, sequence: u64, sent_ns: u64, samples: &[f32]) -> bool {
        if samples.len() != self.samples {
            return false;
        }
        let write = self.write_index().load(Ordering::Relaxed);
        let read = self.read_index().load(Ordering::Acquire);
        if write.wrapping_sub(read) >= self.blocks {
            return false;
        }
        let slot = self.slot(write);
        put_u64(&slot[0..2], sequence);
        put_u64(&slot[2..4], sent_ns);
        for (word, sample) in slot[BLOCK_HEAD..].iter().zip(samples) {
            word.store(sample.to_bits(), Ordering::Relaxed);
        }
        self.write_index()
            .store(write.wrapping_add(1), Ordering::Release);
        true
    }

    /// Reads the oldest block into `into`; only the reader calls it. Its sequence number and when
    /// it was sent, or `None` when there is none (or `into` is not one block).
    pub fn pop(&self, into: &mut [f32]) -> Option<(u64, u64)> {
        if into.len() != self.samples {
            return None;
        }
        let read = self.read_index().load(Ordering::Relaxed);
        let write = self.write_index().load(Ordering::Acquire);
        let waiting = write.wrapping_sub(read);
        if waiting == 0 {
            return None;
        }
        if waiting > self.blocks {
            // No writer that keeps to the rules gets there: start over at its index.
            self.read_index().store(write, Ordering::Release);
            return None;
        }
        let slot = self.slot(read);
        let head = (get_u64(&slot[0..2]), get_u64(&slot[2..4]));
        for (sample, word) in into.iter_mut().zip(&slot[BLOCK_HEAD..]) {
            *sample = f32::from_bits(word.load(Ordering::Relaxed));
        }
        self.read_index()
            .store(read.wrapping_add(1), Ordering::Release);
        Some(head)
    }

    /// Drops everything that waits; only the reader calls it. For a host that starts while
    /// blocks of before it lie in the ring.
    pub fn skip_all(&self) {
        let write = self.write_index().load(Ordering::Acquire);
        self.read_index().store(write, Ordering::Release);
    }
}

fn put_u64(words: &[AtomicU32], value: u64) {
    #[allow(clippy::cast_possible_truncation)]
    words[0].store(value as u32, Ordering::Relaxed);
    words[1].store((value >> 32) as u32, Ordering::Relaxed);
}

fn get_u64(words: &[AtomicU32]) -> u64 {
    u64::from(words[0].load(Ordering::Relaxed))
        | (u64::from(words[1].load(Ordering::Relaxed)) << 32)
}

#[cfg(test)]
#[allow(clippy::float_cmp)] // Samples that were only copied compare exactly.
mod tests {
    use super::*;

    fn ring() -> (Ring, Geometry) {
        let geometry = Geometry::new(1, 16, 4).expect("sizes");
        let region = Arc::new(Region::local(geometry));
        let ring = Ring::new(region, geometry.send_base(), geometry).expect("fits");
        (ring, geometry)
    }

    #[test]
    fn blocks_come_out_in_order_with_their_heads() {
        let (ring, geometry) = ring();
        let mut out = vec![0.0; geometry.samples()];
        assert_eq!(ring.pop(&mut out), None);
        for n in 0..3_u8 {
            let block = vec![f32::from(n) + 0.5; geometry.samples()];
            assert!(ring.push(u64::from(n) + (1 << 40), 7 + u64::from(n), &block));
        }
        assert_eq!(ring.waiting(), 3);
        for n in 0..3_u8 {
            let head = ring.pop(&mut out).expect("a block");
            assert_eq!(head, (u64::from(n) + (1 << 40), 7 + u64::from(n)));
            assert!(out.iter().all(|&s| s == f32::from(n) + 0.5));
        }
        assert_eq!(ring.pop(&mut out), None);
    }

    #[test]
    fn a_full_ring_refuses_and_keeps_what_it_has() {
        let (ring, geometry) = ring();
        let block = vec![1.0; geometry.samples()];
        for n in 0..4 {
            assert!(ring.push(n, 0, &block));
        }
        assert!(!ring.push(4, 0, &block));
        assert!(!ring.push(0, 0, &block[1..]), "not one block");
        let mut out = vec![0.0; geometry.samples()];
        assert_eq!(ring.pop(&mut out).map(|head| head.0), Some(0));
        assert!(ring.push(4, 0, &block));
        ring.skip_all();
        assert_eq!(ring.pop(&mut out), None);
    }

    #[test]
    fn the_indices_count_on_through_their_wrap() {
        let (ring, geometry) = ring();
        ring.write_index().store(u32::MAX - 1, Ordering::Release);
        ring.read_index().store(u32::MAX - 1, Ordering::Release);
        let mut out = vec![0.0; geometry.samples()];
        for n in 0..6_u8 {
            assert!(ring.push(u64::from(n), 0, &vec![f32::from(n); geometry.samples()]));
            assert_eq!(ring.pop(&mut out).map(|head| head.0), Some(u64::from(n)));
            assert!(out.iter().all(|&s| s == f32::from(n)));
        }
    }

    #[test]
    fn an_index_no_ring_can_have_does_no_harm() {
        let (ring, geometry) = ring();
        let block = vec![1.0; geometry.samples()];
        let mut out = vec![0.0; geometry.samples()];
        // A reader gone wrong: the writer sees a full ring and writes nothing.
        ring.read_index().store(12_345, Ordering::Release);
        assert!(!ring.push(0, 0, &block));
        // A writer gone wrong: the reader starts over at its index.
        ring.read_index().store(0, Ordering::Release);
        ring.write_index().store(4_000_000_000, Ordering::Release);
        assert_eq!(ring.waiting(), 4);
        assert_eq!(ring.pop(&mut out), None);
        assert_eq!(ring.waiting(), 0);
        assert!(ring.push(9, 0, &block));
        assert_eq!(ring.pop(&mut out).map(|head| head.0), Some(9));
    }
}
