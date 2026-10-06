# 0013. `cos` parser foundation: hayro-syntax vs our own

- **Status:** Proposed. To be decided in M0 task 2 (time-box: 1 week).
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

*To be filled in by M0 task 2, with measurements: corpus open rate, repair results, fuzz findings, and the API gaps found.*

## Consequences

*To be filled in.*
