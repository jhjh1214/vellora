# bench/ — performance benchmarks

Created in **M0 task 24**.

- Generators for large synthetic documents (10,000 pages; ~500 MB image-heavy), written into `bench/data/` (git-ignored).
- A harness measuring open → first page, peak RSS and re-open-after-commit latency. Run it with `cargo xtask bench`.

Targets: [`docs/architecture/performance-targets.md`](../docs/architecture/performance-targets.md).
