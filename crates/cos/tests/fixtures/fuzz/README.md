# Fuzzer-found regression inputs

Inputs the `write_roundtrip` target (`fuzz/`) crashed on, kept for `tests/fuzz_regressions.rs`. Each is a mutated copy of a corpus file, so it is not a redistribution of anything new.

| File | Derived from | Mutation | Found by |
|---|---|---|---|
| `linearized-shifted-offsets.pdf` | `pdfium-linearized` (PDFium testing corpus, BSD-3-Clause) | `%PDF-1.4\n` inserted at byte 7192, which shifts every later offset | CI fuzz smoke, M0 task 13 |
| `duplicate-object-after-bad-offset.pdf` | a `hello_world`-style file (PDFium testing corpus, BSD-3-Clause) | object 4 replaced by a second `2 0 obj` (a Font), so the table's offset for 4 is wrong and the rebuilt table resolves 2 to the Font | CI fuzz smoke, M0 task 13 |
