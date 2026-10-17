# bench/ — performance benchmarks

`vellora-bench`: generators for two large synthetic documents and a harness that measures them through the real engine process.

```sh
cargo xtask bench                          # build (release), generate if missing, measure, print a table
cargo xtask bench generate [--only <name>] # regenerate the documents
cargo xtask bench run --only text-10k --runs 5
```

```sh
cargo xtask bench run --runs 5 --json out.json    # also write every number to a file
cargo xtask bench compare --previous a.json [b.json ...] --current c.json [d.json ...] [--max-regression 10]
```

`run` also measures the cold open (the OS file cache is dropped on Linux with root or `sudo` and on macOS with `sudo purge`; elsewhere a freshly written copy is opened and the result file says `fresh-file`) and the reading scenarios of `src/scenarios.rs`: scrolling in single, facing and continuous view, a thumbnail fling, a search over all 10,000 pages and printing at 150 dpi, each reported in work per second. The JSON file holds the best of the runs for each metric. `compare` merges several files on a side (best value of each metric) and exits 1 if a tracked metric is worse by more than the limit and by more than its floor. Run only code that behaves identically on both sides: numbers from different machines are not comparable ([ADR-0016](../docs/adr/0016-re-open-after-commit-strategy.md), amendment). The nightly job (`.github/workflows/bench.yml`) builds the last good revision and measures it next to the current one.

Names: `text-10k` (10,000 pages of text, ~45 MiB) and `images-500mb` (102 pages with one raw 1280 x 1280 RGB image each, ~478 MiB). They are written to `bench/data/` (git-ignored) and need about 0.5 GB of disk. Generation is deterministic and takes about a second; every byte of PDF structure comes from the `cos` writers (a tiny seed is repaired, rewritten with `write_full`, then the pages are appended as one incremental section).

What is measured (details in `src/measure.rs`): `cos` open, engine open → first page (process start to the first tile of page 1), the engine's peak memory, and for two commit sizes the section build, append + `fsync` and re-open → first page. Each file is cut back to its original length after a commit, so runs repeat. The numbers and their limits are recorded in [ADR-0016](../docs/adr/0016-re-open-after-commit-strategy.md).

Peak memory comes from the OS: the peak working set (via PowerShell) on Windows, `VmHWM` on Linux; macOS has no peak counter without platform calls (`unsafe` is forbidden here), so it prints the current RSS and says so.

Targets: [`docs/architecture/performance-targets.md`](../docs/architecture/performance-targets.md).
