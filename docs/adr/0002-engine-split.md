# 0002. PDFium is a read-only renderer; `cos` is the sole writer

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

Rendering, parsing, object editing, content editing, incremental updates and full rewrites are different problems. A library that renders well does not necessarily edit faithfully.

- **PDFium** (BSD/Apache) has the best real-world rendering compatibility and is continuously fuzzed by Chromium.
- But its editing write-back (`FPDFPage_GenerateContent`) re-serialises the *whole* content stream containing an edited object. In doing so it drops `Tc`/`Tw` spacing operators and converts `cs/scn` colour spaces to device colours. It's also not thread-safe.
- MuPDF is excluded by license (ADR-0001). qpdf is excellent at structure but doesn't write incremental updates.

## Decision

- **PDFium** is used **read-only**: rasterisation, glyph geometry, annotation and form rendering, form interaction. It is **never** used to save or modify documents.
- **`vellora-cos`** (ours, Rust) is the **authority on document structure** and the **only writer**: incremental updates, full rewrites, and later content-stream splicing, redaction and signatures.
- PDFium always renders the current revision produced by `cos`: the original bytes plus pending incremental sections, served from memory via `FPDF_FILEACCESS`.
- On open, page count, page-tree refs and per-page `/Contents` refs are cross-checked between `cos` and PDFium. On disagreement, or if either needed repair, the document is marked **Repaired**; saving it requires a normalising full rewrite.
- Signatures are validated only through `cos`.
- qpdf serves as a **test oracle** (`--check`, `--json`), not a runtime dependency.
- PDF4QT's engine (MIT) serves as a **reference and port source**.

## Alternatives considered

| Option | Verdict |
|---|---|
| A. Build entirely on PDFium | ✗ lossy edits, no redaction or signing |
| B. One engine + our model | Insufficient alone: no permissive engine is both a renderer and a faithful writer |
| **C + D. PDFium renders, we own the write path** | ✓ |
| Link PDF4QT's engine | ✗ C++ hostile-input parsing; core depends on Qt types; single maintainer |
| hayro (pure-Rust renderer) now | ✗ not yet complete or fast enough; revisit later as an optional renderer |

## Consequences

- We must build a robust parser and writer ourselves. That's the main early cost and risk (see ADR-0013 for build vs adopt).
- Two parsers means differential-parsing risk, mitigated by the cross-check and the *Repaired* mode.
- Each commit re-opens the revision in PDFium. Its latency is measured in M0 and recorded in a follow-up ADR.
