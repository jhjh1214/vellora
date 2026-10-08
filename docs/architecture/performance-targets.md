# Performance targets

Reference machine: mid-range laptop (4–8 cores, 16 GB RAM, SSD, integrated GPU). Benchmarks in `bench/` track these in CI. A regression of more than 10% on a tracked metric fails the nightly job.

| Metric | Target | First enforced |
|---|---|---|
| App start → window ready | < 400 ms warm, < 1 s cold | Phase 1 |
| Open 10-page text PDF → first page sharp | < 300 ms | Phase 1 |
| Open 10,000-page PDF → first page | < 1.5 s, no full parse | **M0** |
| Open 500 MB image-heavy PDF → first page | < 2 s; RSS bounded by tile-cache budget | M0 (measured), Phase 1 (enforced) |
| Scroll | 60 fps compositing; placeholders never block; sharp tiles typically < 100 ms | Phase 1 |
| UI thread | our code ≤ 8 ms of every 16.7 ms frame: no event handler or timer, and no `CanvasWidget::render()`, over 8 ms. The vsynced present is not ours and is not counted. The debug watchdog logs violations; a Qt test asserts none during a scripted 2,000-page scroll with 20 zooms of a 10,000-page document | M0 (logged), **M1** (enforced) |
| Idle memory, one document open | < 150 MB across processes, excluding tile cache (default cap 256 MB) | Phase 1 |
| Search, 1,000-page text document | first hit < 200 ms; full scan < 3 s, streaming | Phase 1 |
| Reorder pages in a 5,000-page document | instant interaction; incremental save < 2 s | Phase 2 |
| Compare two 100-page contracts | < 5 s to change list | Phase 5 |
| Re-open after commit (10k pages / 500 MB) | measured in M0; ADR records the result | **M0** |

## Architecture choices that serve these targets

- Lazy xref and object loading. The whole document is never materialised.
- mmap or positioned reads over the original. Incremental sections are kept in memory.
- Tiled rendering with priority scheduling and cancellation, plus progressive resolution.
- GPU compositing of tiles (QRhi). Rasterisation on the CPU (PDFium).
- Background, streaming text extraction for search. Results arrive as they are found.
- A bounded LRU tile cache in shared memory.
