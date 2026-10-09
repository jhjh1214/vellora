# Backlog

Known gaps and ideas that are **not scheduled** in a milestone. Each entry links to where it was found. When a milestone picks one up, move the line into that milestone's task and delete it here.

| Item | Found in | Notes |
|---|---|---|
| Rebuilding the xref of a damaged *encrypted* file that uses object streams loses those objects (`ObjectStreamNotExpanded`) | M0 task 11 | Needs a decrypt hook in `recovery::rebuild`; no corpus file hits it yet |
| An incremental update of a damaged *encrypted* file (V2/R3 Standard handler) does not reopen: Encryption { kind: UnsupportedHandler }, found by the `write_roundtrip` fuzz smoke (uzz/fuzz_targets/write_roundtrip.rs:52) | M1 task 1 (PR CI, run 37775143833) | Unrelated to the UI work. Crash input: artifact `fuzz-artifacts-write_roundtrip` of that run (1,142 bytes, expires after the retention period); add it as a regression fixture when fixing |
| A file whose `/Encrypt` dictionary is broken (`/O` too short: `pdfium-encrypted_hello_world_r2_bad_okey` and `_r3_`) is answered with `PasswordRequired`, because PDFium reports a password error for it, so the prompt asks for a password that cannot work. `cos` knows better (`malformed /Encrypt dictionary`) | M1 task 7 | Report it as `OpenFailed` with `cos`'s reason when `cos` refuses the dictionary and PDFium asks for a password; needs a corpus test |
| Passwords are wiped where the code controls the buffer, but the frame reader grows its buffer with `read_to_end` (earlier, smaller copies are freed unwiped), the Qt line edit holds a `QString` copy, and PDFium keeps its own copy for the life of the document | M1 task 7 | Best effort by design (`docs/architecture/security-model.md`); revisit if the M11 audit asks. A sized read buffer in `ipc` is a small change |
| `stringprep` (SASLprep for revision 6 passwords) brings `unicode-normalization`, `unicode-bidi`, `unicode-properties` and `tinyvec` into the engine process; replace it with a small in-tree normaliser if the dependency count matters | M1 task 7 | Needed for `pdfjs-saslprep-r6` |
| Single-page and spread-at-a-time views do not prefetch the neighbouring rows, so turning a page can show a white page until its tiles arrive (continuous views prefetch a viewport above and below) | M1 task 10 | Ask for the first tiles of the next and previous row at `Prefetch` priority; measure with the frame-time test |
| The thumbnail cache counts bytes but allocates whole 1 MiB slots, so its 64 MB budget holds 64 thumbnails however small they are (a 120 px thumbnail uses a tenth of its slot). Enough for the cells on screen plus the margin, wasteful for memory | M1 task 11 | Cut a slot into several thumbnail-sized cells, or give thumbnails a region with smaller slots; measure with the sidebar fling test first |
| Thumbnails are upright and ignore the view rotation; pages beyond the first 4,096 are laid out and rendered at the last known page size until the engine sends their sizes | M1 task 11 | Acrobat turns the thumbnails with the view; the page-size limit is the protocol's `MAX_PAGE_SIZES_PER_MESSAGE` and also affects the canvas |
| Linearised output (fast web view) | M0 task 12 | Post-1.0 optimisation phase |
| Render PDFium tiles straight into shared memory instead of one memcpy | M0 task 15 | Only if profiling shows the copy |
| Tile region backed by a pagefile section on Windows / `shm_open` on macOS instead of an anonymous temp file | ADR-0015 | Linux has a sealed `memfd` since M1 task 5 |
| Saving over the open file: on Windows the document is held with `FILE_SHARE_READ` only (no rename or delete), so M2's atomic save must close or hand over the handle (or the engine must be stopped) before the rename | M1 task 2 | Decide in M2 task 0 together with ADR-0009 (ChangeSets) |
| Linux sandbox: turn the seccomp deny list into an allow list from traced sessions; run the corpus gate and a font-rendering comparison on Linux in nightly; test the `kill`/`tgkill` rules | M1 task 5 (ADR-0018) | Needs a Linux machine or a CI job that runs `strace -c` over the corpus |
| Windows engine hardening beyond the AppContainer: process mitigation policies (ACG, CFG, win32k lockdown, which need testing against PDFium's GDI font mapping) and a low-integrity token | M1 task 4 (ADR-0017) | Only if the security review (M11) asks for it |
| Measure documents with very deep/wide page trees and thousands of annotations | ADR-0016 | Fold into the M2 or M3 benchmarks if one of them regresses |
| `/Version` in the catalog overriding the header version in `vellora inspect` | M0 task 23 | Small; good first issue |
| OSS-Fuzz integration | PLAN §I | Apply once the project is public and stable (M11 task 4) |
| Portfolios (PDF collections), read-only view | PLAN §C | Post-1.0 |
| PDF → Word/Excel export | PLAN §C | Out of scope until a mature permissive engine exists |
