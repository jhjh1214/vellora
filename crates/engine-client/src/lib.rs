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
//! [`limits`]); the protocol client, restart logic, tile cache and the `cxx` bridge follow in
//! tasks 20–21 (`docs/milestones/M0.md`).

pub mod limits;
pub mod process;

pub use limits::ResourceLimits;
pub use process::{Crash, EngineProcess, SpawnConfig, SpawnError, Termination};
