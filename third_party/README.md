# third_party/ — pinned binary dependencies

- `pdfium.lock` (M0 task 14): the pinned [`bblanchon/pdfium-binaries`](https://github.com/bblanchon/pdfium-binaries) release tag, with per-platform asset names and SHA-256 checksums. Builds must be **V8-free and XFA-free** (ADR-0011, security model).
- `cargo xtask pdfium fetch` downloads and verifies the binaries into `third_party/pdfium/<platform>/` (git-ignored).
- License texts of bundled binaries are collected into `THIRD_PARTY_LICENSES` at release time.

Before 1.0 we move from prebuilt binaries to building PDFium from source in CI (supply-chain auditability).

## THIRD_PARTY_LICENSES

`THIRD_PARTY_LICENSES` at the repository root is generated; do not edit it by hand. Regenerate it after dependency changes (needs `cargo install --locked cargo-about --features cli`, and the PDFium build from `cargo xtask pdfium fetch` for the license texts):

The file is embedded in the application (Help → About Vellora → Third-party licenses). `cargo test -p xtask` fails when a crate that is linked into a binary is missing from it, so regenerate it whenever `Cargo.lock` changes.

```sh
cargo about generate --workspace about.hbs -o THIRD_PARTY_LICENSES
cat third_party/NOTICES.md >> THIRD_PARTY_LICENSES
printf '
PDFium bundled license texts
==============================
' >> THIRD_PARTY_LICENSES
for f in third_party/pdfium/win-x64/licenses/*; do printf '\n--- %s ---\n' "$(basename "$f")"; cat "$f"; done >> THIRD_PARTY_LICENSES
```
