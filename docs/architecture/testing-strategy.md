# Testing strategy

Tests are evidence. Never weaken an assertion, skip a test or loosen a limit to get green. Find out whether the code, the test or the environment is wrong.

## Layers

| Layer | What | Where / tooling |
|---|---|---|
| Unit | Lexer, xref, filters, encryption test vectors (R2–R6), writer, ops, diff algorithms | Next to the code; `cargo test` |
| Property | `parse(write(x)) == x`; lexer never panics; limits always enforced | `proptest` |
| Invariant (golden) | Per operation × corpus document: see below | `tests/` harness over corpus manifests |
| Rendering regression | Render pages at fixed DPI with pinned PDFium and compare to approved PNGs with a perceptual threshold | `tests/` harness; re-baselined only on PDFium bumps, with review |
| Integration | CLI end-to-end, engine IPC, crash and restart, cancellation | `crates/*/tests`, engine test client |
| UI | Command registry, keyboard reachability, accessibility-tree snapshots | Qt Test; manual screen-reader checklist per release (NVDA, VoiceOver, Orca) |
| Fuzzing | Lexer, xref repair, object parser, decryption, writer round-trip; later content tokeniser and ops | `fuzz/` (cargo-fuzz): 60 s per target in CI, long runs nightly |
| Differential | `cos` object graph vs `qpdf --json`; page tree vs PDFium | `tests/` |
| Security | Malicious and CVE reproducers, bombs, signature-attack sets, redaction-leak suite | Restricted corpus repo |
| Performance | Open time, first page, scroll frame times, peak RSS | `bench/` with trend tracking |
| Conformance (later) | PDF/A and PDF/UA outputs vs veraPDF | CI job (Phase 13) |

## Core invariants (checked for every write operation)

1. The output passes `qpdf --check`.
2. **Incremental save:** the original file is a byte-identical prefix of the output.
3. **Untouched pages render pixel-identically** before and after.
4. Content edits change pixels **only inside the edit bounding box**.
5. Extracted text outside edit regions is unchanged.
6. Existing signatures still validate after an incremental save.
7. The output re-parses with `cos` and loads in PDFium with the same page count.

## Compatibility corpus

- Lives in a **separate repository**, fetched by manifest: `tests/corpus/manifest.toml` with entries `{id, url, sha256, license, categories, notes}`. Downloaded into `tests/corpus-data/` (git-ignored).
- **Sources:** pdf.js test corpus, PDFium test corpus, DARPA SafeDocs corpora, GovDocs1 sample, veraPDF / Isartor suites, PDF Association samples, Arlington model test files.
- **Categories:** forms (AcroForm, XFA), scans, contracts, textbooks, image-heavy, many-fonts, many-annotations, encrypted, signed (incl. LTV), attachments, malformed, by producer (Word, LibreOffice, InDesign, LaTeX, Chrome, scanners, Acrobat).
- **Synthetic generators** (in `bench/`) for scale: 10,000 pages, 500 MB image-heavy, 50,000 annotations.
- Malicious samples live only in the restricted security corpus.

## CI tiers

| Trigger | Runs |
|---|---|
| Every PR | fmt, clippy, unit/property/integration tests, `cargo deny`, fuzz smoke, corpus subset (once available) |
| Nightly | full corpus invariants, rendering regression, long fuzzing, benchmarks with trend alerts |
| Release | all of the above + packaging smoke tests on each OS |
