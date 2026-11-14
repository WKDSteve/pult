//! The memory both sides of a chain see, as a slice of atomic words.
//!
//! Between two processes it is a named file mapping (Windows, in `ffi`); inside one process, for
//! the tests and for systems without a mapping yet, it is plain memory behind an `Arc`.

use std::sync::atomic::{AtomicU32, Ordering};

use crate::layout::{Geometry, MAGIC, VERSION, word};

/// The memory of one chain.
#[derive(Debug)]
pub enum Region {
    /// Memory of this process only.
    Local(Box<[AtomicU32]>),
    /// A named mapping another process can open.
    #[cfg(windows)]
    Mapped(crate::ffi::Mapping),
}

impl Region {
    /// Memory of this process for `geometry`, all zero.
    #[must_use]
    pub fn local(geometry: Geometry) -> Self {
        Self::Local((0..geometry.words()).map(|_| AtomicU32::new(0)).collect())
    }

    /// A new named mapping for `geometry`, all zero. For the engine, before any stream starts.
    ///
    /// # Errors
    ///
    /// When Windows refuses the mapping, or one of that name exists already.
    #[cfg(windows)]
    pub fn create(name: &str, geometry: Geometry) -> std::io::Result<Self> {
        crate::ffi::Mapping::create(name, geometry.words()).map(Self::Mapped)
    }

    /// The mapping `name` another process created. For the host.
    ///
    /// # Errors
    ///
    /// When there is none of that name, or it is smaller than `geometry` needs.
    #[cfg(windows)]
    pub fn open(name: &str, geometry: Geometry) -> std::io::Result<Self> {
        crate::ffi::Mapping::open(name, geometry.words()).map(Self::Mapped)
    }

    /// All its words.
    #[must_use]
    pub fn words(&self) -> &[AtomicU32] {
        match self {
            Self::Local(words) => words,
            #[cfg(windows)]
            Self::Mapped(mapping) => mapping.words(),
        }
    }

    /// Word `index` of the header as it is now.
    #[must_use]
    pub fn get(&self, index: usize) -> u32 {
        self.words()[index].load(Ordering::Acquire)
    }

    /// Writes word `index` of the header.
    pub fn set(&self, index: usize, value: u32) {
        self.words()[index].store(value, Ordering::Release);
    }

    /// Adds one to word `index` of the header; it wraps.
    pub fn bump(&self, index: usize) {
        self.words()[index].fetch_add(1, Ordering::AcqRel);
    }

    /// Writes the header of a new region: what it is and its sizes. The engine does that before
    /// it starts a host.
    pub fn stamp(&self, geometry: Geometry) {
        self.set(word::MAGIC, MAGIC);
        self.set(word::VERSION, VERSION);
        self.set(word::CHANNELS, as_word(geometry.channels));
        self.set(word::FRAMES, as_word(geometry.frames));
        self.set(word::BLOCKS, as_word(geometry.blocks));
    }

    /// Whether the header says what `geometry` says. The host checks that before it attaches.
    ///
    /// # Errors
    ///
    /// Says what does not fit.
    pub fn check(&self, geometry: Geometry) -> Result<(), String> {
        if self.words().len() < geometry.words() {
            return Err(format!(
                "the region has {} words, {} are needed",
                self.words().len(),
                geometry.words()
            ));
        }
        let expect = [
            ("magic", word::MAGIC, MAGIC),
            ("version", word::VERSION, VERSION),
            ("channels", word::CHANNELS, as_word(geometry.channels)),
            ("frames", word::FRAMES, as_word(geometry.frames)),
            ("blocks", word::BLOCKS, as_word(geometry.blocks)),
        ];
        for (what, index, wanted) in expect {
            let found = self.get(index);
            if found != wanted {
                return Err(format!("{what} is {found}, expected {wanted}"));
            }
        }
        Ok(())
    }
}

/// A size as a word; the ranges of a [`Geometry`] are far below what a word holds.
fn as_word(size: usize) -> u32 {
    u32::try_from(size).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_stamped_region_checks_against_its_own_sizes_only() {
        let geometry = Geometry::new(2, 64, 8).expect("sizes");
        let region = Region::local(geometry);
        assert!(region.check(geometry).unwrap_err().contains("magic"));
        region.stamp(geometry);
        assert_eq!(region.check(geometry), Ok(()));
        let other = Geometry::new(2, 128, 8).expect("sizes");
        assert!(region.check(other).unwrap_err().contains("words"));
        let fewer = Geometry::new(1, 64, 8).expect("sizes");
        assert!(region.check(fewer).unwrap_err().contains("channels"));
    }
}
