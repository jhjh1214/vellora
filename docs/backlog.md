# Backlog

Known gaps and ideas that are **not scheduled** in a milestone. Each entry links to where it was found. When a milestone picks one up, move the line into that milestone's task and delete it here.

| Item | Found in | Notes |
|---|---|---|
| Rebuilding the xref of a damaged *encrypted* file that uses object streams loses those objects (`ObjectStreamNotExpanded`) | M0 task 11 | Needs a decrypt hook in `recovery::rebuild`; no corpus file hits it yet |
| An incremental update of a damaged *encrypted* file (V2/R3 Standard handler) does not reopen: Encryption { kind: UnsupportedHandler }, found by the `write_roundtrip` fuzz smoke (uzz/fuzz_targets/write_roundtrip.rs:52) | M1 task 1 (PR CI, run 37775143833) | Unrelated to the UI work. Crash input: artifact `fuzz-artifacts-write_roundtrip` of that run (1,142 bytes, expires after the retention period); add it as a regression fixture when fixing |
| Linearised output (fast web view) | M0 task 12 | Post-1.0 optimisation phase |
| Render PDFium tiles straight into shared memory instead of one memcpy | M0 task 15 | Only if profiling shows the copy |
| Tile region backed by a pagefile section on Windows / `shm_open` on macOS instead of an anonymous temp file | ADR-0015 | Linux `memfd` is M1 task 5 |
| Saving over the open file: on Windows the document is held with `FILE_SHARE_READ` only (no rename or delete), so M2's atomic save must close or hand over the handle (or the engine must be stopped) before the rename | M1 task 2 | Decide in M2 task 0 together with ADR-0009 (ChangeSets) |
| Measure documents with very deep/wide page trees and thousands of annotations | ADR-0016 | Fold into the M2 or M3 benchmarks if one of them regresses |
| `/Version` in the catalog overriding the header version in `vellora inspect` | M0 task 23 | Small; good first issue |
| OSS-Fuzz integration | PLAN §I | Apply once the project is public and stable (M11 task 4) |
| Portfolios (PDF collections), read-only view | PLAN §C | Post-1.0 |
| PDF → Word/Excel export | PLAN §C | Out of scope until a mature permissive engine exists |
