# third_party/ — pinned binary dependencies

- `pdfium.lock` (M0 task 14): the pinned [`bblanchon/pdfium-binaries`](https://github.com/bblanchon/pdfium-binaries) release tag, with per-platform asset names and SHA-256 checksums. Builds must be **V8-free and XFA-free** (ADR-0011, security model).
- `cargo xtask pdfium fetch` downloads and verifies the binaries into `third_party/pdfium/<platform>/` (git-ignored).
- License texts of bundled binaries are collected into `THIRD_PARTY_LICENSES` at release time.

Before 1.0 we move from prebuilt binaries to building PDFium from source in CI (supply-chain auditability).
