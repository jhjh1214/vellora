//! Typed errors for mapping, the tile region and handle passing.

use std::io;

/// Everything that can go wrong in this crate.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// An operating-system call failed.
    #[error("{context}: {source}")]
    Io {
        /// What was being attempted.
        context: &'static str,
        /// The operating system's error.
        source: io::Error,
    },
    /// The slot geometry is unusable (zero or too large).
    #[error("invalid slot geometry: {0}")]
    InvalidGeometry(&'static str),
    /// A slot index is not in the region.
    #[error("slot {slot} is out of range: the region has {count} slots")]
    SlotOutOfRange {
        /// The requested slot.
        slot: u32,
        /// Slots in the region.
        count: u32,
    },
    /// The file behind a region is shorter than its geometry needs.
    #[error("the shared region holds {have} bytes but its geometry needs {need}")]
    RegionTooSmall {
        /// Size of the backing file.
        have: u64,
        /// Size the geometry requires.
        need: u64,
    },
    /// A file is bigger than the caller allows to be mapped.
    #[error("file of {len} bytes exceeds the {max}-byte limit")]
    TooLarge {
        /// Size of the file.
        len: u64,
        /// The limit that applies.
        max: u64,
    },
    /// A handle number received from the parent does not name a usable open file.
    #[error("handle {token} is not usable: {reason}")]
    InvalidHandle {
        /// The number that was passed.
        token: u64,
        /// Why it was refused.
        reason: &'static str,
    },
}

impl Error {
    pub(crate) fn io(context: &'static str, source: io::Error) -> Self {
        Self::Io { context, source }
    }
}
