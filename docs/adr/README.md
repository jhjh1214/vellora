# Architecture Decision Records

We record every significant architectural decision as an ADR ([MADR](https://adr.github.io/madr/) format). ADRs are **immutable once accepted**. To change a decision, write a new ADR that supersedes the old one, and update the old one's status line to point at it.

**Process:** copy [`0000-template.md`](0000-template.md) to `NNNN-short-title.md` and open a PR with status *Proposed*. It becomes *Accepted* when merged.

| # | Title | Status |
|---|---|---|
| [0001](0001-license-mpl-2.0-and-dco.md) | License: MPL-2.0, contributions under DCO | Accepted |
| [0002](0002-engine-split.md) | PDFium is a read-only renderer; `cos` is the sole writer | Accepted |
| [0003](0003-rust-core-qt-shell.md) | Rust core, C++/Qt 6 Widgets shell, `cxx` bridge | Accepted |
| [0004](0004-process-model-and-sandboxing.md) | One sandboxed engine process per document | Accepted |
| [0005](0005-ipc-protocol.md) | IPC: postcard over pipes, tiles in shared memory, Rust on both ends | Accepted |
| [0006](0006-crypto-backend.md) | Cryptography: OpenSSL 3 for PKI/CMS, RustCrypto for primitives | Accepted |
| [0007](0007-dependency-license-policy.md) | Dependency license policy enforced by cargo-deny | Accepted |
| [0008](0008-mvp-scope.md) | MVP scope | Accepted |
| [0009](0009-non-destructive-document-model.md) | Non-destructive document model and explicit save modes | Accepted |
| [0010](0010-ocr-engine-tesseract.md) | OCR engine: Tesseract 5 | Accepted |
| [0011](0011-xfa-out-of-scope.md) | XFA forms are out of scope | Accepted |
| [0012](0012-no-telemetry-local-crash-dumps.md) | No telemetry; local-only crash dumps | Accepted |
| [0013](0013-cos-parser-foundation.md) | `cos` parser foundation: hayro-syntax vs our own | **Accepted**: own parser (option C) |
