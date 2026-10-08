# Changelog

All notable changes to this project are documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added
- Repository foundation: governance documents, architecture documentation and ADRs, Rust workspace skeleton, CI.
- `cargo xtask pdfium fetch`: downloads the PDFium build pinned in `third_party/pdfium.lock`, verifies its SHA-256 and extracts it.
- `vellora-render`: thin bindings to PDFium, loaded at run time (ADR-0014).
- `vellora inspect <file> [--json] [--password <pw>]`: version, pages, objects, revisions, encryption, repairs and risky features (JavaScript, launch/URI/submit/remote actions, embedded files, forms, XFA, layers, signature fields). Output formats in `docs/user/cli.md`.
