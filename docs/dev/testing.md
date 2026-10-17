# Testing how-to

Strategy and invariants: [`../architecture/testing-strategy.md`](../architecture/testing-strategy.md).

## Where tests go

| Kind | Location |
|---|---|
| Unit | `#[cfg(test)] mod tests` next to the code |
| Crate integration | `crates/<crate>/tests/*.rs` |
| Cross-crate invariants and corpus runs | `tests/invariants/` (crate `vellora-invariants`) |
| Fuzz targets | `fuzz/fuzz_targets/*.rs` (cargo-fuzz, nightly) |
| Benchmarks | `bench/` (from M0 task 24) |

## Fixtures

- Prefer constructing PDFs **in the test**, as byte strings or with the `cos` writer once it exists. This keeps the bytes visible and reviewable.
- Small binary fixtures (< 100 KB) go in `tests/fixtures/`, with a README stating how each was generated and its license.
- Real-world PDFs live in the corpus (`cargo xtask corpus fetch`), never in this repository.

## Running

```sh
cargo test --workspace                     # unit + integration
cargo test -p vellora-cos                  # one crate
# Invariant harness: needs qpdf (PATH, or QPDF=<path>) and the corpus. Full corpus, ~1 min:
cargo xtask corpus fetch && cargo test -p vellora-invariants -- --ignored --nocapture
# What CI runs (20 files):
cargo xtask corpus fetch --manifest tests/corpus/ci-manifest.toml --dir target/ci-corpus
VELLORA_CORPUS_DIR=target/ci-corpus cargo test -p vellora-invariants -- --ignored --nocapture
# Fuzzing (Linux/macOS, nightly; targets: lexer, object_parser, xref_open, write_roundtrip):
cd fuzz && cargo +nightly fuzz run lexer -- ../tests/corpus-data -max_total_time=60
```

## The rendering regression suite

`crates/cli/tests/goldens.rs` draws the first two pages of 40 corpus documents (`tests/corpus/golden-manifest.toml`) with `vellora render` at 96 dpi and compares them with golden images. A page passes when it has the size of its golden and the share of pixels that differ by more than `pixel_delta` is at most `max_ratio`; both are per operating system in `tests/goldens/tolerance.toml`, with a measured, named allowance for the few text-heavy pages that macOS draws with other hinting.

The goldens are not in the repository. They are one *bundle* attached to a GitHub release (`goldens-vN`), pinned by SHA-256 in `tests/goldens/manifest.toml`; the bundle also holds `ATTRIBUTION.txt` (source and license of every document, because the images are renderings of other projects' files).

```sh
cargo xtask corpus fetch --manifest tests/corpus/golden-manifest.toml
cargo xtask goldens fetch
cargo test -p vellora-cli --test goldens -- --ignored --nocapture
```

`--nocapture` prints, for every page, the share of differing pixels at several thresholds: use it to see how close a page is to the limit. CI runs the suite on Windows, Linux and macOS. `a_broken_render_fails_the_suite` (always on, nothing fetched) shows the comparison is not vacuous: a page shifted by two pixels, one with a corner inverted, a blank page and a page of another size are each refused.

**After a reviewed PDFium bump** (or any deliberate change of rendering), regenerate the goldens on a machine with the corpus subset and PDFium:

```sh
cargo xtask goldens update          # renders again, writes target/goldens/<tag>.bundle and the manifest
gh release create goldens-vN target/goldens/goldens-vN.bundle --prerelease ...   # the command is printed
```

Then commit `tests/goldens/manifest.toml`, and look at the numbers CI prints on the other systems before touching `tolerance.toml`. A tolerance is raised only with the measured reason written next to it, never to make a change pass.

## The nightly corpus gates

`nightly.yml` runs, on all three systems, the engine corpus gate (every corpus document through the real engine: open, password path, first tile), the engine report against the CLI on 20 documents, the `cos` corpus tests and the `inspect` corpus test, all with `--ignored` against the full corpus, plus the writer invariants and the fuzzers.

## When a test fails

Find out whether the **code**, the **test** or the **environment** is wrong, and say which in the PR or commit. Never weaken an assertion, mark a test `#[ignore]`, or raise a limit just to get green.
