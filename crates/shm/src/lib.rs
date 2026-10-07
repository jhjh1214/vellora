//! # vellora-shm — operating-system primitives for the engine process
//!
//! **Responsibility:** the small set of things the engine host needs from the operating system
//! that cannot be written in safe Rust, kept in one reviewable place (ADR-0015):
//!
//! - [`MappedFile`]: a read-only memory-mapped document, usable as a `vellora-cos`
//!   [`ByteSource`](vellora_cos::ByteSource) and as the bytes PDFium reads
//! - [`TileRegion`] and [`SlotGeometry`]: the shared-memory tile region of the process model
//! - [`share_with_child`], [`stop_sharing`] and [`adopt`]: passing open files to the engine as
//!   inherited handles, so it never receives a path
//!
//! **Boundaries:**
//!
//! - No PDF knowledge, no protocol, no policy. The engine decides what to map and when.
//! - `unsafe` is confined to the modules `mapped`, `region` and `handle`; each block carries a
//!   `// SAFETY:` comment and tests exercise each operation on every supported OS.
//! - Linked into `vellora-engine` and `vellora-engine-client`, never into a crate that parses
//!   PDF data.
//!
//! **Status:** M0 task 17.

// The three modules below are the only places in this crate that need `unsafe`.
#[allow(unsafe_code)]
mod handle;
#[allow(unsafe_code)]
mod mapped;
#[allow(unsafe_code)]
mod region;

mod error;

pub use error::Error;
pub use handle::{HandleToken, adopt, share_with_child, stop_sharing};
pub use mapped::MappedFile;
pub use region::{MAX_REGION_BYTES, SlotGeometry, TileRegion};
