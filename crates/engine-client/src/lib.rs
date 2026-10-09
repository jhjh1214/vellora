//! # vellora-engine-client — the UI's window onto the engine
//!
//! **Responsibility:**
//!
//! - spawn and supervise `vellora-engine` processes: launch, handle passing,
//!   restart after a crash
//! - typed request/response API over `vellora-ipc`
//! - map shared-memory tile slots and maintain the UI-side tile cache (with an
//!   LRU budget)
//! - expose all of the above to the C++/Qt shell through a narrow `cxx` bridge
//!   (ADR-0003: `cxx`, not `cxx-qt`)
//!
//! **Boundaries:** never parses PDF bytes (CLAUDE.md invariant 2). It handles
//! only protocol messages and pixels. No Qt types cross the bridge; the C++
//! side adapts to Qt.
//!
//! **Status:** M0 task 19 added process start-up under OS resource limits ([`process`],
//! [`limits`]); task 20 added the protocol client with restart after a crash ([`client`]) and the
//! `cxx` bridge ([`bridge`]); task 21 added the tile cache ([`cache`]); task 22a wired it into the
//! client and bridge (`docs/milestones/M0.md`). M1 task 2 added the client's own deadlines (engine
//! start-up and stalled tiles) and the write-deny open of the document ([`document`]). Task 9b added
//! the log files ([`logging`]) and the capture of the engine's log lines (`capture`).
//!
//! **`unsafe`:** denied crate-wide. Only the bridge module allows it, for the glue the `cxx` macro
//! generates, and the small OS modules in [`limits`] and [`document`] (every block has a
//! `// SAFETY:` comment; CLAUDE.md invariant 6).

pub mod bridge;
pub mod cache;
mod capture;
pub mod client;
mod document;
pub mod limits;
pub mod logging;
pub mod process;
#[cfg(windows)]
#[allow(unsafe_code)]
mod sandbox;

pub use cache::{ReserveError, ScaleBucket, TileCache, TileKey};
pub use client::{
    Client, ClientConfig, ClientError, DEFAULT_HELLO_TIMEOUT, DEFAULT_OPEN_TIMEOUT, Event, Stage,
    TILE_PIXELS, TileLookup, TileRequest,
};
pub use document::Change;
pub use limits::ResourceLimits;
pub use process::{
    Crash, EngineProcess, PDFIUM_ENV, PDFIUM_LIBRARY_WINDOWS, SpawnConfig, SpawnError, Termination,
};
