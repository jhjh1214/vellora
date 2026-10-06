//! # vellora-inspect — document inspection and security analysis
//!
//! **Responsibility:** read-only analysis of a parsed document:
//!
//! - document summary: version, page count, object count, revisions, encryption
//! - feature detection: JavaScript, embedded files, launch / URI / submit
//!   actions, AcroForm, XFA, optional content, signatures
//! - later: object browser data, size attribution, fonts and images
//!
//! Every report type is serialisable, so the CLI (`--json`) and the UI share it.
//!
//! **Boundaries:** depends only on `vellora-cos`. It never modifies documents,
//! never renders, and never executes anything it finds.
//!
//! **Status:** skeleton. Implemented by M0 task 23 (`docs/milestones/M0.md`).
