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
| [0014](0014-pdfium-acquisition-and-bindings.md) | PDFium: pinned prebuilt binaries, thin hand-written bindings, loaded at run time | Accepted |
| [0015](0015-os-primitives-crate-and-engine-launch-contract.md) | A dedicated crate for OS primitives (`vellora-shm`), and the engine launch contract | Accepted |
| [0016](0016-re-open-after-commit-strategy.md) | Re-open-after-commit strategy: keep ADR-0002's re-open of the new revision | Accepted |
| [0017](0017-engine-sandbox-on-windows.md) | Engine sandbox on Windows: AppContainer, handle list, jailed before it runs | Accepted |
| [0018](0018-engine-sandbox-on-linux.md) | Engine sandbox on Linux: Landlock, seccomp deny list, sealed tile region | Accepted |
