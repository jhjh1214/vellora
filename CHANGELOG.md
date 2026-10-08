# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Repository foundation: governance documents, architecture documentation and ADRs, Rust workspace skeleton, CI.
- `cargo xtask pdfium fetch`: downloads the PDFium build pinned in `third_party/pdfium.lock`, verifies its SHA-256 and extracts it.
- `vellora-render`: thin bindings to PDFium, loaded at run time (ADR-0014).
- `vellora inspect <file> [--json] [--password <pw>]`: version, pages, objects, revisions, encryption, repairs and risky features (JavaScript, launch/URI/submit/remote actions, embedded files, forms, XFA, layers, signature fields). Output formats in `docs/user/cli.md`.
- `cargo xtask bench`: synthetic 10,000-page and ~500 MB documents and a harness for open → first page, peak memory and re-open-after-commit latency (ADR-0016).
- Corpus gate: `crates/engine/tests/corpus.rs` opens every corpus v0 document in the engine and renders page 1 (97.0% of the non-password files open, no engine crash or hang).
- `THIRD_PARTY_LICENSES` (cargo-about for Rust crates, plus PDFium and Qt notices); milestone M1 (viewer) task plan.

### Milestone M0 (walking skeleton) complete
All acceptance criteria in `docs/milestones/M0.md` are ticked with evidence. Not tagged.
