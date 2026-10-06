# 0013. `cos` parser foundation: hayro-syntax vs our own

- **Status:** Accepted (M0 task 2). Option C: write our own.
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

`vellora-cos` must parse hostile PDFs lazily and support a revision-aware, byte-preserving writer (ADR-0002, ADR-0009). `hayro-syntax` (Apache-2.0/MIT, part of the hayro project: v0.7 in June 2026, >1000-file regression suite) is a lazy pure-Rust PDF parser that might serve as a foundation. `lopdf` (MIT) loads documents eagerly and doesn't retain byte spans, so it's not suitable as a foundation.

## Evaluation criteria

| # | Criterion | Weight |
|---|---|---|
| 1 | Lazy, mmap-friendly object access (10k pages / 500 MB without full load) | must |
| 2 | Retains raw byte spans for objects, streams and tokens (needed for splicing and byte-identical saves) | must |
| 3 | Xref repair quality on the malformed corpus subset | high |
| 4 | Can support a revision-aware object store and incremental writer without forking deeply | high |
| 5 | Encryption support (R2–R6), or a clean place to add it | medium |
| 6 | Panic-freedom on fuzzed input (run our fuzz targets against it) | must |
| 7 | License (permissive), maintainer health, API stability | high |

## Options

- **A. Build on `hayro-syntax`:** depend on it, contribute upstream, and wrap it with our object store and writer.
- **B. Fork `hayro-syntax`** into `cos`, keeping attribution in NOTICE.
- **C. Write our own** lexer and parser, using hayro-syntax, pdf.js, PDFBox, qpdf and PDF4QT as references.

## Decision

**Option C: write our own lexer and parser in `cos`.** Keep `hayro-syntax` as a reference implementation (and a candidate dev-only differential oracle later); do not depend on it or fork it.

### Evidence

Evaluated `hayro-syntax` 0.8.0 (Apache-2.0 OR MIT, default features, so no `unsafe` feature) with a throwaway harness outside the repo. Rust 1.99.0 release build, Windows 11, corpus v0 (275 files, 54 tagged `malformed`). Panics were caught with `catch_unwind`; each file ran on its own thread with a timeout.

| # | Criterion | Result |
|---|---|---|
| 1 | Lazy, mmap-friendly | **Pass, with a caveat.** `Pdf::new` takes `Arc<impl AsRef<[u8]>>`, so a `memmap2::Mmap` works: the 524 MB / 500-page image document opened in 1.7 ms with 5 MB peak working set (reading it into a `Vec` instead: 176 ms, 505 MB). The page tree is built eagerly at open: 10,000 flat pages open in 22 ms using 26 MB; 100,000 pages in 240 ms using 218 MB (about 2 KB per page, scaling with page count). |
| 2 | Raw byte spans | **Fail.** No public span for objects or tokens. `Dict::data()` and `Stream::raw_data()` return slices, but objects from object streams point into a decoded buffer, not the file, and decrypted streams are owned copies. File offsets are not exposed. |
| 3 | Repair on `malformed` | **Good.** 47 of 54 `malformed` files opened; none panicked or hung. The `repaired` state is internal and not observable, so we cannot report it or tell a repaired open from a clean one. |
| 4 | Revision-aware store and incremental writer | **Fail without a deep change.** `/Prev` chains are flattened into one map at open: no revision list, no per-revision byte ranges, no way to ask for the newest xref section's style (table or stream) or to read `/Prev`/`/Size` for a new trailer. `/Prev` is read as `i32`, which breaks offsets above 2 GiB. |
| 5 | Encryption R2–R6 | **Present** (RC4, AES-128, AES-256 R5/R6). The 11 encrypted corpus files that failed to open all returned `PasswordProtected` for the empty password, which looks right for those fixtures but is not verified against qpdf yet. |
| 6 | Panic-freedom | **Pass on input, fail on resource limits.** Two seeds of 6,000 mutated corpus files each (byte flips, truncation, deletion, splicing, huge numbers): 0 panics, 0 hangs. But there are no decode limits: `pdfjs-bomb_giant.pdf` (123 KB) took 5 s to decode one page and reached a **9.5 GB peak working set**. This violates invariant 5 and cannot be fixed from outside because the filters are internal. |
| 7 | License and health | Permissive; active (0.8.0). API is pre-1.0 and changing. Always-on `unsafe` exists even without the `unsafe` feature (`page.rs` transmutes a reference to `'static`; `smallvec` is a hard dependency). |

Corpus open results: 255 of 275 opened (92.7%). Of the 20 that did not, 11 are password-protected and 9 returned `Invalid` (`pdfium-bug_298`, `pdfium-bug_343`, `pdfium-circular_viewer_ref`, `pdfium-repeat_viewer_ref`, `pdfjs-REDHAT-1531897-0`, `pdfjs-bug1020226`, `pdfjs-encrypted-attachment`, `pdfjs-poppler-742-0-fuzzed`, `pdfjs-poppler-937-0-fuzzed`). "Opened" only means no error: correctness was not compared against a reference.

### Why not A or B

- **A (depend and wrap):** criteria 2 and 4 are "must"/"high" and cannot be met through the public API. We would still write our own xref, revision and recovery layer, and the limit problem (criterion 6) stays outside our control. What is left to reuse is the object syntax and filters, the part of the parser that is cheapest to write and test.
- **B (fork):** it would give us the filters and crypto, but a fork would have to add spans, revisions, limits and a repair flag across most of the crate. It would also bring `unsafe` into `cos`, where the workspace lints forbid it (invariant 6), and we would own a diverging copy of a fast-moving project.

## Consequences

- Tasks 3–12 stay as written; no task changes from "wrap" to "write". Task 3's limits and task 10's filters are the main correction to what the spike found: every decoder gets output-size and ratio caps from the start, and the corpus bomb files are regression tests for them.
- Task 6/7/8/9 must keep per-revision xref sections with byte ranges, `/Prev` as an unsigned 64-bit offset, and an observable `repaired` flag with reasons.
- Task 9 should build the page-tree walk lazily (the spike shows about 2 KB per page when it is eager), so the 10k-page open stays under the 1.5 s target comfortably.
- Writing the parser ourselves costs time (the estimate stays inside M0's 8–10 weeks) and bug risk in code that hayro already exercises. Mitigations: proptest round-trips, fuzz targets (task 13) and, if useful, `hayro-syntax` as a dev-only differential oracle on the corpus. Adding it would need its own dependency check and is not part of this decision.
- Reference material to read for each task: `hayro-syntax`, pdf.js, PDFBox, qpdf and PDF4QT, as the plan says. Copy no GPL/AGPL/LGPL code; Apache-2.0/MIT code may be ported with attribution in `NOTICE`.
- Revisit only through a new ADR.
