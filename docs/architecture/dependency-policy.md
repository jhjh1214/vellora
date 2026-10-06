# Dependency and license policy

Vellora is **MPL-2.0** ([ADR-0001](../adr/0001-license-mpl-2.0-and-dco.md)). This policy keeps the distribution clean for users, packagers, commercial embedders and proprietary plugin authors. Enforcement: `deny.toml` (`cargo deny check` in CI) plus PR review for non-Rust dependencies.

## Allowed for linked dependencies

MIT, MIT-0, Apache-2.0 (incl. LLVM exception), BSD-2-Clause, BSD-3-Clause, ISC, Zlib, MPL-2.0, Unicode-3.0, BSL-1.0, CC0-1.0. Fonts: OFL-1.1.

## The single LGPL exception: Qt 6

- Dynamically linked only. We ship Qt as shared libraries so users can replace or relink them. **Never link Qt statically.**
- Use only Qt modules available under LGPLv3. Don't use GPL-only Qt add-ons. KDDockWidgets is GPL/commercial, so use `QDockWidget`.
- The About dialog and docs carry the Qt license notice and relinking information.

## Never linked or bundled

| Project | License | Status |
|---|---|---|
| MuPDF | AGPL-3.0 | Rejected: would force AGPL |
| Poppler | GPL | Rejected |
| Ghostscript | AGPL | Optional **external** executable only, user-installed, invoked at arm's length |
| ExifTool | GPL/Artistic (Perl) | Not used |
| Any GPL/AGPL/LGPL source code | — | Must never be copied into the repo |

## Optional external tools (invoked as separate processes, never bundled unless their license allows)

- LibreOffice (`soffice --headless`, MPL-2.0) for Office → PDF
- veraPDF (MPL-2.0/GPL-3.0, Java) for PDF/A and PDF/UA validation
- Ghostscript (AGPL), for specific conversions only if installed by the user

## Key adopted dependencies (as they are introduced)

| Dependency | License | Introduced |
|---|---|---|
| PDFium (pinned prebuilt, `bblanchon/pdfium-binaries`) | BSD-3 / Apache-2.0 | M0 |
| Qt 6.8 LTS | LGPLv3 (dynamic) | M0 |
| cxx | MIT/Apache-2.0 | M0 |
| serde, postcard, thiserror, anyhow, tracing | MIT/Apache-2.0 | M0 |
| clap (CLI) | MIT/Apache-2.0 | M0 |
| flate2 / zlib-rs, weezl (LZW) | MIT/Apache-2.0 / Zlib | M0 |
| RustCrypto (aes, sha2, md-5, rc4…) | MIT/Apache-2.0 | M0 (decryption) |
| proptest, cargo-fuzz/libfuzzer-sys | MIT/Apache-2.0 | M0 (dev) |
| OpenSSL 3 | Apache-2.0 | Phase 9 (signatures) |
| Tesseract 5 | Apache-2.0 | Phase 10 (OCR) |
| harfrust, skrifa/read-fonts, subsetter | MIT/Apache-2.0 | Phase 4 |

## Adding a dependency

1. Check that its license is in the allow list (`cargo deny check`).
2. Prefer well-maintained crates with a security track record. Avoid crates that pull large trees for small features.
3. Justify it in the commit body: why it's needed, alternatives considered, and how much it adds to the dependency tree.
4. A new **non-Rust** dependency (C/C++ library, external tool) requires an ADR.

## Ported code

Porting from permissively licensed projects is allowed: e.g. PDF4QT (MIT since 2025-04-27), PDFBox (Apache-2.0), pdf.js (Apache-2.0), qpdf (Apache-2.0). Keep the original header and add an entry to [`NOTICE`](../../NOTICE).
