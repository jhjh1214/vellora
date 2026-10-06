# 0010. OCR engine: Tesseract 5

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

OCR is a document capability. It must be local, offline and permissively licensed.

## Decision

- **Tesseract 5** (Apache-2.0, LSTM models Apache) through its C API, in a separate sandboxed worker process.
- Output (hOCR/TSV word boxes) becomes an invisible text layer (render mode 3) in an appended content stream, written by `cos` (incremental save).
- OCRmyPDF (MPL-2.0) is a design reference only. It isn't bundled: Python + Ghostscript.
- Higher-accuracy engines (PaddleOCR, docTR via ONNX Runtime) may be added later as providers behind the out-of-process boundary.

## Consequences

- Language packs are downloaded on demand by explicit user action, or bundled per distribution.
- Implementation is in roadmap Phase 10.
