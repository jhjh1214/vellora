# 0016. Re-open-after-commit strategy

- **Status:** Accepted
- **Date:** 2026-10-08
- **Deciders:** @jhjh1214

## Context

[ADR-0002](0002-engine-split.md) makes `cos` the only writer and PDFium a read-only renderer. Its consequence: every commit (an edit made permanent) appends an incremental section with `cos`, and the engine then **re-opens the new revision in PDFium** to show the result. ADR-0002 left the latency of that re-open to be measured in M0.

The risk is a document where re-opening is slow enough that each commit stalls the UI: the two worst cases in the performance targets are a 10,000-page text document and a ~500 MB image-heavy one.

## Measurements

`cargo xtask bench` (M0 task 24) generates both documents with the `cos` writers and measures them through the real engine process (`Client::open` to the `TileReady` of page 1 at 100 %, so it includes process start, the file mapping, PDFium's open and one rendered tile). A guard fails the run if that tile is blank.

Machine: the dev laptop (Windows 11, x86-64, SSD, 13.8 GB RAM), release build, PDFium from `third_party/pdfium.lock`, file in the OS cache. Median of 3 runs where repeated; one run per commit.

| | 10k-page text (45 MiB, 10,000 pages) | image-heavy (478 MiB, 102 pages, raw RGB) |
|---|---|---|
| `cos` open (cross-reference only) | 2 ms | < 1 ms |
| Engine: open | 182 ms | 19 ms |
| **Engine: open → first page** | **189 ms** | **36 ms** |
| Engine peak working set | 76 MiB | 32 MiB |
| Commit, metadata edit (≈260 B section): `cos` build | 7 ms | < 1 ms |
| … append + `fsync` | 1 ms | 1 ms |
| … **re-open → first page** | **190 ms** | **45 ms** |
| Commit, 5 MiB insert: build / append + `fsync` | 14 / 5 ms | 7 / 5 ms |
| … **re-open → first page** | **199 ms** | **77 ms** |
| Total commit-to-visible (5 MiB insert) | 217 ms | 89 ms |

Targets from `docs/architecture/performance-targets.md`: the 10k-page document shows its first page in **189 ms against < 1.5 s**; the 500 MB document in **36 ms against < 2 s**.

What the table says:

- Re-open after a commit costs about the same as a first open, and it does not grow with the size of the appended section in the sizes tried (5 MiB added 9 ms and 32 ms).
- The cost is dominated by PDFium's open of the page tree, which scales with the page count and not with the file size: 182 ms for 10,000 pages, 19 ms for 102 pages in a file ten times bigger.
- Nothing in the path reads the whole file: peak memory stays far below the document size.

## Decision

Keep the ADR-0002 approach. A commit is `cos::incremental_update`, an append plus `fsync` of the new section, and a re-open of the whole new revision in the engine. No incremental update of PDFium's in-memory document, and no PDFium save or edit API, is needed.

The UI keeps showing the previous tiles until the re-opened document delivers the first tile, then swaps. At the measured latencies this takes well under a second for both worst-case documents.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| **Re-open the new revision (chosen)** | Simple; the renderer always shows exactly the bytes on disk; no PDFium edit API | Pays PDFium's open again on each commit (≈ 0.2 s at 10,000 pages) |
| Patch PDFium's in-memory document through its edit API | No re-open | Breaks ADR-0002 (lossy regeneration, two writers); far more code |
| Keep a long-lived engine and only swap the document inside it | Saves process start (not separately measured; a share of the numbers above) | Not needed at these latencies; adds state to carry over a crash |

## Consequences

- Positive: ADR-0002 stands unchanged. Commit latency is not a risk for M0 or for the Phase 1 viewer.
- Negative / costs: the cost is per commit and grows with page count, so a batch of edits should be committed once and not per keystroke (the ChangeSet layer of [ADR-0009](0009-non-destructive-document-model.md) already works that way).
- Limits of this measurement: Windows only so far; **warm cache only** (a cold first open of a 500 MB file from a slow disk is not measured); one machine; the working set includes file pages PDFium touched through the mapping, so it overstates private memory; page content is simple (text lines, one image per page), so heavy vector or font-rich pages will make the first tile slower than shown, though not the re-open itself.
- Follow-ups: run the same harness on Linux and macOS and with a cold cache before the Phase 1 targets are enforced; measure a document with a deep or very wide page tree and one with thousands of annotations; consider `cargo xtask bench` in the nightly job once the 10 % regression rule applies.

## References

- [ADR-0002](0002-engine-split.md), [ADR-0009](0009-non-destructive-document-model.md)
- `bench/` and `cargo xtask bench`; `docs/architecture/performance-targets.md`
- The milestone task: `docs/milestones/M0.md`, task 24 (which called this ADR "0014"; that number was already taken by the PDFium ADR).
