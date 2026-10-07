# 0014. PDFium acquisition, bindings and loading

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

ADR-0002 makes PDFium the read-only renderer, and the security model requires a V8-free, XFA-free build (ADR-0011). M0 task 14 has to decide how the binary gets into the build, how Rust talks to it, and how it is linked. The forces:

- Builds must be reproducible and tamper-evident (supply chain, security model).
- `cargo build`, `cargo clippy` and `cargo test` of the *whole workspace* must keep working on a machine where PDFium has not been fetched, and on the UI and CLI processes, which never load PDFium.
- The binding surface is tiny (13 functions) and the engine's safety argument depends on every `unsafe` block being small and reviewable.
- PDFium has global state and is not thread-safe.

## Decision

1. **Acquisition:** pinned prebuilt binaries from [`bblanchon/pdfium-binaries`](https://github.com/bblanchon/pdfium-binaries), recorded in `third_party/pdfium.lock` (release tag, per-platform URL, archive SHA-256, library path). Only the plain `pdfium-<platform>.tgz` assets are accepted, which are built with `pdf_enable_v8 = false` and `pdf_enable_xfa = false` (checked in each archive's `args.gn` when pinning). `cargo xtask pdfium fetch` downloads, verifies and extracts to `third_party/pdfium/<platform>/`, and records the library's own hash so later runs detect a modified file. The lock is validated by xtask: HTTPS, the upstream release path and the exact asset name (which keeps `pdfium-v8-*` out).
2. **Bindings:** hand-written `extern "C"` declarations in one module (`crates/render/src/ffi.rs`), transcribed from `fpdfview.h` of the pinned release. Only read-side functions are declared; no save or edit entry point exists in the table (ADR-0002).
3. **Loading:** the shared library is opened at run time with [`libloading`](https://crates.io/crates/libloading) (ISC), not linked at build time. The engine ships the library next to its executable. `Pdfium::locate` checks `$VELLORA_PDFIUM_LIB`, then the executable's directory.
4. **Ownership:** at most one `Pdfium` per process (a process-wide claim, so a second `load` is a typed error), neither `Send` nor `Sync`. Task 15 puts it on the dedicated PDFium thread.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| `bindgen` output committed | Mechanically correct against the headers | Needs libclang to regenerate; generates the whole of `fpdfview.h` (hundreds of items) for the 13 used; reviewers still read generated code |
| `pdfium-render` crate (MIT/Apache) | Ready-made safe API | Much larger surface than we want, its own threading and lifetime model, an extra dependency chain to audit; we still need our own loader policy |
| **Hand-written thin bindings** | Minimal, reviewable, nothing declared that ADR-0002 forbids | Signatures can drift from the header: mitigated by tests that call every declared function against the real library |
| Link at build time (`build.rs`, import library) | Simplest call sites | Every workspace build and test would need PDFium present; the UI and CLI would depend on it |
| **Load at run time** | Workspace builds without PDFium; a missing or wrong library is a typed error; easy to ship beside the engine | One more crate; symbol typos surface at load time (covered by the tests) |
| Build PDFium from source now | Full auditability | Hours of CI per platform; planned before 1.0 |

## Consequences

- Positive: the whole `unsafe` surface is one module of a few hundred lines, and CI exercises each binding on all three OSes.
- Positive: bumping PDFium is a one-file change (`pdfium.lock`) plus a re-run of the tests.
- Negative: the lock pins prebuilt third-party binaries until we build from source; the archive hash protects against tampering after pinning, not against a compromised upstream at pin time.
- Negative: PDFium's `FPDF_FILEACCESS` length is a C `unsigned long` (32 bits on Windows), so a document of 4 GiB or more cannot be opened through it there; `Pdfium::page_count` returns `DocumentTooLarge`. Task 17 must not hit it: M0's largest document is about 500 MB.
- Follow-ups: task 15 builds the renderer thread and tile API on this table; moving to a from-source PDFium build before 1.0; collecting PDFium's bundled licenses (`licenses/` in each archive) into `THIRD_PARTY_LICENSES` in task 25.

## References

- ADR-0002, ADR-0011, `docs/architecture/security-model.md`
- `third_party/pdfium.lock`, `xtask/src/pdfium.rs`, `crates/render/src/ffi.rs`
