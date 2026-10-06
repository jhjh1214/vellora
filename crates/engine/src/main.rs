//! # vellora-engine — engine host process
//!
//! **Responsibility:** one process per open document, launched by the UI (or by
//! the CLI for commands that need rendering):
//!
//! - IPC server (`vellora-ipc`) over the pipes handed to it at launch
//! - document session: `vellora-cos` object store, pending revisions, history
//! - job scheduler: priorities (visible > prefetch > thumbnails), cancellation,
//!   deadlines
//! - rendering through `vellora-render` (PDFium), on a single PDFium thread
//! - self-imposed resource limits; the parent applies OS limits (Job object,
//!   rlimit/cgroup) and the per-OS sandbox (ADR-0004)
//!
//! **Boundaries:** no filesystem access beyond handles passed by the parent, no
//! network, never executes document content.
//!
//! **Status:** skeleton. Implemented by M0 tasks 17–19 (`docs/milestones/M0.md`).

use std::process::ExitCode;

fn main() -> ExitCode {
    if let Some("--version" | "-V") = std::env::args().nth(1).as_deref() {
        println!("vellora-engine {}", env!("CARGO_PKG_VERSION"));
        ExitCode::SUCCESS
    } else {
        eprintln!("vellora-engine is started by Vellora; it is not meant to be run directly.");
        ExitCode::from(2)
    }
}
