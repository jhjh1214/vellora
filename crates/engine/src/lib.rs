//! # vellora-engine — engine host process
//!
//! **Responsibility:** one process per open document, launched by the UI (or by the CLI for
//! commands that need rendering):
//!
//! - IPC server (`vellora-ipc`) over the process's standard input and output
//! - document session: the file mapped read-only, parsed by `vellora-cos` and opened in PDFium,
//!   with the two cross-checked ([`Engine`])
//! - rendering through `vellora-render` (PDFium) on its single PDFium thread, straight into the
//!   shared tile region (`vellora-shm`)
//! - a priority queue for tiles with cancellation, and soft/hard deadlines per tile ([`Deadlines`])
//! - the executable aborts itself when a tile passes its hard deadline (`main.rs`); the parent
//!   applies the OS limits at spawn (`vellora-engine-client`) and, on Windows, starts it in an
//!   `AppContainer` (ADR-0017); on Linux the engine sandboxes itself with Landlock and seccomp
//!   after loading PDFium ([sandbox], ADR-0018)
//!
//! **Boundaries:** no filesystem access beyond the handles passed by the parent, no network,
//! never executes document content, never writes a file. This crate contains no `unsafe`; the
//! operating-system primitives live in `vellora-shm` (ADR-0015).
//!
//! **Launch contract** (see [`args`]): the parent opens the document and the tile region, marks
//! both inheritable and spawns `vellora-engine` with their handle numbers and the slot geometry
//! on the command line. Standard input and output carry the length-prefixed protocol; standard
//! error carries logs (never document content at `info` level or above).
//!
//! **Status:** M0 task 19 (host, handshake, open with cross-check, scheduled tile serving,
//! deadlines that end the process). A reader thread validates requests and queues tiles by
//! priority; one worker renders them, so answers can arrive in a different order than the
//! requests.

pub mod args;
mod document;
#[cfg(target_os = "linux")]
pub mod sandbox;
mod scheduler;
mod session;

pub use args::{ArgsError, DEFAULT_MAX_DOCUMENT_BYTES, Invocation, LaunchArgs};
pub use scheduler::Deadlines;
pub use session::{Engine, Exit, ServeError};
