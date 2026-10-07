# Fuzzer-found regression inputs

Inputs the `write_roundtrip` target (`fuzz/`) crashed on, kept for `tests/fuzz_regressions.rs`. Each is a mutated copy of a corpus file, so it is not a redistribution of anything new.

| File | Derived from | Mutation | Found by |
|---|---|---|---|
| `linearized-shifted-offsets.pdf` | `pdfium-linearized` (PDFium testing corpus, BSD-3-Clause) | `%PDF-1.4\n` inserted at byte 7192, which shifts every later offset | CI fuzz smoke, M0 task 13 |
