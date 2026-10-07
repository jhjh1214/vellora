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

## When a test fails

Find out whether the **code**, the **test** or the **environment** is wrong, and say which in the PR or commit. Never weaken an assertion, mark a test `#[ignore]`, or raise a limit just to get green.
