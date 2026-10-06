//! # vellora-cos — the PDF object layer
//!
//! **Responsibility:** everything between raw PDF bytes and PDF objects, in both
//! directions:
//!
//! - lexer and object parser (ISO 32000-2 §7.2–7.3)
//! - cross-reference tables, cross-reference streams, object streams and hybrid
//!   files (§7.5), plus a recovery scan for damaged files
//! - a lazy, revision-aware object store over a memory-mapped or read-handle
//!   byte source
//! - stream filters (§7.4) and Standard Security Handler decryption R2–R6 (§7.6)
//! - **the only PDF writer in Vellora:** incremental updates (§7.5.6) and full
//!   rewrites
//! - resource limits for every decode and recursion (`limits` module)
//!
//! **Boundaries:**
//!
//! - No rendering, no content-stream interpretation (that is `vellora-content`,
//!   created in Phase 4), no UI concerns.
//! - Treats all input as hostile. It never panics on input and never allocates
//!   without bound (CLAUDE.md invariant 5).
//! - `unsafe` is forbidden.
//!
//! **Status:** skeleton. Implemented by M0 tasks 3–12 (`docs/milestones/M0.md`).
//! M0 task 2 decides whether the parser builds on `hayro-syntax` or is written
//! from scratch (ADR-0013).
