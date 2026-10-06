//! # vellora-render — safe PDFium wrapper (read-only)
//!
//! **Responsibility:** the only place Vellora talks to PDFium:
//!
//! - load a document revision from bytes we provide (`FPDF_FILEACCESS`)
//! - rasterise tiles into caller-provided buffers (shared memory)
//! - report page sizes and, later, glyph geometry, annotation and form rendering
//!
//! **Boundaries:**
//!
//! - **Read-only.** Never call PDFium save or edit APIs (`FPDF_SaveAsCopy`,
//!   `FPDFPage_GenerateContent`, …). `vellora-cos` is the only writer
//!   (ADR-0002).
//! - PDFium is not thread-safe. One PDFium instance per process; all calls go
//!   through a single thread owned by this crate.
//! - Linked only into `vellora-engine`, never into the UI or the CLI process.
//! - `unsafe` is confined to the FFI module, and every block has a `// SAFETY:`
//!   comment.
//!
//! **Status:** skeleton. Implemented by M0 tasks 14–15 (`docs/milestones/M0.md`).
