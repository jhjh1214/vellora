//! # vellora-ipc — UI ↔ engine protocol
//!
//! **Responsibility:**
//!
//! - message types for the engine protocol (requests, responses, events),
//!   framed as length-prefixed `postcard` messages over pipes
//! - a protocol version handshake
//! - descriptors for shared-memory tile slots (the pixels themselves never
//!   travel through the pipe)
//!
//! **Boundaries:**
//!
//! - Pure data and framing. No I/O policy, no PDF knowledge, no PDFium.
//! - Both ends are Rust (`vellora-engine` and `vellora-engine-client`), so no
//!   cross-language schema is needed (ADR-0005).
//! - Every message read from the peer is validated (size caps) before
//!   allocation.
//!
//! **Status:** skeleton. Implemented by M0 task 16 (`docs/milestones/M0.md`).
