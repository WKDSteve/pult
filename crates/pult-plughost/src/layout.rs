//! What the shared memory of one chain looks like: a header of 32-bit words, then two rings of
//! blocks, one to the host and one back.
//!
//! Everything in it is a 32-bit word, read and written as an atomic: samples as the bits of an
//! `f32`, 64-bit numbers as two words. So no word is ever torn, in either process, and the layout
//! needs no padding and no pointer.
//!
//! ```text
//! header   HEADER_WORDS words, see [`word`]
//! ring     to the host:  write index, read index, then `blocks` blocks
//! ring     back:         write index, read index, then `blocks` blocks
//! block    sequence number (2 words), time it was sent in ns (2 words), frames * channels samples
//! ```

/// The first word of a region that is laid out like this: `PLTH`.
pub const MAGIC: u32 = 0x504C_5448;
/// The version of this layout.
pub const VERSION: u32 = 1;
/// Words of the header.
pub const HEADER_WORDS: usize = 32;
/// Words a ring has before its blocks: its write index and its read index.
pub const RING_HEAD: usize = 2;
/// Words a block has before its samples: its sequence number and when it was sent.
pub const BLOCK_HEAD: usize = 4;
/// Buckets of the host's wake delays: up to 100, 250, 500 and 1000 µs, and beyond.
pub const WAKE_BUCKETS: usize = 5;
/// The upper edges of all buckets but the last, in µs.
pub const WAKE_EDGES_US: [u64; WAKE_BUCKETS - 1] = [100, 250, 500, 1000];

/// Where the words of the header are.
pub mod word {
    /// [`super::MAGIC`], written by the engine.
    pub const MAGIC: usize = 0;
    /// [`super::VERSION`], written by the engine.
    pub const VERSION: usize = 1;
    /// Channels per frame, written by the engine.
    pub const CHANNELS: usize = 2;
    /// Frames per block, written by the engine.
    pub const FRAMES: usize = 3;
    /// Blocks per ring, written by the engine.
    pub const BLOCKS: usize = 4;
    /// Counts the blocks the engine sent or would have sent: the host ends when it stands.
    pub const ENGINE_BEAT: usize = 5;
    /// Counts the blocks the host worked through: its heartbeat.
    pub const HOST_BEAT: usize = 6;
    /// 1 once a host is attached and waits for blocks; the host writes it.
    pub const HOST_READY: usize = 7;
    /// What the host would need to come back as it is: here the bits of its gain.
    pub const HOST_STATE: usize = 8;
    /// 1 asks the host to end; the engine writes it.
    pub const STOP: usize = 9;
    /// The longest the host took from a block being sent to working on it, in µs.
    pub const WAKE_MAX_US: usize = 10;
    /// The first of [`super::WAKE_BUCKETS`] counters of those delays.
    pub const WAKE_BUCKET: usize = 11;
    /// Counts up when the engine sets a new parameter; the host then reads [`PARAM`].
    pub const PARAM_SERIAL: usize = 16;
    /// The parameter: here the bits of the gain the host shall apply.
    pub const PARAM: usize = 17;
}

/// Fewest and most channels of a chain.
pub const CHANNELS: (usize, usize) = (1, 8);
/// Fewest and most frames of a block.
pub const FRAMES: (usize, usize) = (16, 4096);
/// Fewest and most blocks of a ring; a power of two.
pub const BLOCKS: (usize, usize) = (2, 64);

/// The sizes of one chain's memory. Both sides are given them at their start and never read them
/// back from the memory for an index: the other process could have written anything there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Geometry {
    /// Channels per frame.
    pub channels: usize,
    /// Frames per block.
    pub frames: usize,
    /// Blocks per ring, a power of two: the indices count on through their wrap.
    pub blocks: usize,
}

impl Geometry {
    /// Sizes that are within their ranges.
    ///
    /// # Errors
    ///
    /// Says which size is outside its range, or that `blocks` is no power of two.
    pub fn new(channels: usize, frames: usize, blocks: usize) -> Result<Self, String> {
        let within = |what: &str, value: usize, range: (usize, usize)| {
            if (range.0..=range.1).contains(&value) {
                Ok(())
            } else {
                Err(format!(
                    "{what} must be {} to {}, not {value}",
                    range.0, range.1
                ))
            }
        };
        within("channels", channels, CHANNELS)?;
        within("frames", frames, FRAMES)?;
        within("blocks", blocks, BLOCKS)?;
        if !blocks.is_power_of_two() {
            return Err(format!("blocks must be a power of two, not {blocks}"));
        }
        Ok(Self {
            channels,
            frames,
            blocks,
        })
    }

    /// Samples of one block.
    #[must_use]
    pub fn samples(&self) -> usize {
        self.frames * self.channels
    }

    /// Words of one block, its head included.
    #[must_use]
    pub fn block_words(&self) -> usize {
        BLOCK_HEAD + self.samples()
    }

    /// Words of one ring, its head included.
    #[must_use]
    pub fn ring_words(&self) -> usize {
        RING_HEAD + self.blocks * self.block_words()
    }

    /// Words of the whole region.
    #[must_use]
    pub fn words(&self) -> usize {
        HEADER_WORDS + 2 * self.ring_words()
    }

    /// Where the ring to the host starts.
    #[must_use]
    pub fn send_base(&self) -> usize {
        HEADER_WORDS
    }

    /// Where the ring back starts.
    #[must_use]
    pub fn return_base(&self) -> usize {
        HEADER_WORDS + self.ring_words()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_outside_their_ranges_are_refused() {
        assert!(Geometry::new(2, 64, 8).is_ok());
        assert!(Geometry::new(0, 64, 8).unwrap_err().contains("channels"));
        assert!(Geometry::new(2, 8, 8).unwrap_err().contains("frames"));
        assert!(Geometry::new(2, 64, 128).unwrap_err().contains("blocks"));
        assert!(Geometry::new(2, 64, 6).unwrap_err().contains("power"));
    }

    #[test]
    fn the_rings_lie_behind_the_header_and_each_other() {
        let g = Geometry::new(2, 64, 8).expect("sizes");
        assert_eq!(g.block_words(), 4 + 128);
        assert_eq!(g.ring_words(), 2 + 8 * 132);
        assert_eq!(g.send_base(), HEADER_WORDS);
        assert_eq!(g.return_base(), HEADER_WORDS + g.ring_words());
        assert_eq!(g.words(), g.return_base() + g.ring_words());
    }
}
