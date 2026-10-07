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
//! client and bridge (`docs/milestones/M0.md`).
//!
//! **`unsafe`:** denied crate-wide. Only the bridge module allows it, for the glue the `cxx` macro
//! generates; it contains no hand-written `unsafe` (CLAUDE.md invariant 6).

pub mod bridge;
pub mod cache;
pub mod client;
pub mod limits;
pub mod process;

pub use cache::{ReserveError, ScaleBucket, TileCache, TileKey};
pub use client::{Client, ClientConfig, ClientError, Event, TILE_PIXELS, TileLookup, TileRequest};
pub use limits::ResourceLimits;
pub use process::{Crash, EngineProcess, SpawnConfig, SpawnError, Termination};
