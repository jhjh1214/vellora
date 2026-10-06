# CLAUDE.md — Vellora implementation rules

These rules apply to every implementation session, human or AI. They are derived from the approved plan ([`docs/PLAN.md`](docs/PLAN.md)) and the ADRs ([`docs/adr/`](docs/adr/)). **If a task seems to require breaking a rule, stop and ask the maintainer. Don't redesign silently.**

## What Vellora is

A local-first, open-source (MPL-2.0) PDF workstation:
- a Rust core
- a C++/Qt 6 Widgets desktop shell
- PDFium as a read-only renderer, running in a sandboxed engine process

Read in this order before your first task:
1. `docs/architecture/overview.md`
2. `docs/architecture/data-model.md`
3. `docs/architecture/security-model.md`
4. the current milestone file

## How to work

1. Open the current milestone file: **`docs/milestones/M0.md`**.
2. Take the **first unchecked task** whose dependencies are all checked. Do tasks in order unless the milestone file says otherwise.
3. Implement it within the scope the task lists. Don't start the next task in the same change. Don't add features that no task asks for.
4. Run the task's **acceptance checks** plus the standard checks below. Read your own diff.
5. Tick the task (`- [x]`) in the milestone file in the same commit. Add a one-line note under it if you deviated or learned something the next task needs.
6. Commit using Conventional Commits, one task per commit (or a few commits for big tasks). Only commit or push when the maintainer has asked you to.

If a task is ambiguous, contradicts an ADR, or turns out to be much bigger than described: stop, explain, and propose a split or an ADR amendment.

## Standard checks (definition of done)

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny check            # licenses, advisories, bans
```

C++ (once `app/` exists): the CMake build succeeds, `clang-format --dry-run -Werror` passes, and Qt tests pass.

Tests are evidence. **Never** weaken an assertion, skip or ignore a test, or loosen a resource limit to get green. Find out whether the code, the test or the environment is wrong, and say which. Report exactly what you ran and what you could not run.

## Architecture invariants (non-negotiable)

1. **`cos` is the only writer of PDF bytes.** PDFium is never used to save or modify documents. Its content regeneration is lossy (ADR-0002).
2. **The UI process never parses PDF data.** All parsing and rendering happens in `vellora-engine`. The UI receives pixels (shared memory) and plain data (IPC).
3. **The original file is never mutated while open.** Edits are ChangeSets layered on top (ADR-0009). Saves write to a temp file, fsync, then atomically rename.
4. **Untouched content stays byte-identical.** Incremental saves append; they never rewrite existing bytes. Content edits splice operator byte ranges; they don't re-serialise whole streams.
5. **Untrusted input never panics and never allocates without bound.** In `cos`, `inspect` and every crate that touches PDF bytes:
   - no `unwrap`/`expect`/`panic!`/unchecked indexing on input-derived data
   - every decoder and recursion goes through the limits in `cos::limits`
   - return typed errors (`thiserror`)
6. **`unsafe` is forbidden** (workspace lint) except in `vellora-render` and `vellora-engine-client` (FFI). Every `unsafe` block there needs a `// SAFETY:` comment.
7. **No network access from the engine.** No telemetry, ever. No new network feature without an ADR.
8. **PDF JavaScript is never executed.** Launch actions are never executed. Attachments are never opened.
9. **Dependencies:** every new crate must pass `deny.toml` (permissive licenses only; Qt is the sole LGPL exception, dynamically linked). Prefer the standard library and a few well-maintained crates. Justify each new dependency in the commit body. GPL, AGPL and LGPL code must never be copied into the repo.
10. **No speculative architecture.** Create the crates `content`, `doc`, `ops` and `diff` only when the milestone that needs them starts. Add no plugin system, generic "engine trait" or abstraction layer that no current task needs.

## Code conventions

- **Rust:** edition 2024; `thiserror` for library errors; `anyhow` only in binaries (`cli`, `engine`); `tracing` for logs (never log document content at info level or above); `#[must_use]` on builders and results where useful.
- **Naming:** crate dirs are `crates/<name>`, package names `vellora-<name>`, library names `vellora_<name>`.
- **Public API:** doc comments on all public items. Each crate's `lib.rs` header states its responsibility and boundaries. Keep that up to date.
- **Tests:** unit tests next to the code; cross-crate and invariant tests in `tests/`; property tests with `proptest` for parser and writer round-trips; fuzz targets in `fuzz/`.
- **C++:** C++20, Qt 6.8 LTS Widgets, `.clang-format` in repo root; no PDF parsing; talk to Rust only through `engine-client`'s cxx bridge.
- **Comments:** explain *why* and PDF-spec references (e.g. `// ISO 32000-2 §7.5.6: incremental updates`), not what.

## Commits

Conventional Commits, e.g. `feat(cos): parse cross-reference streams`.
- Scopes: crate or area names.
- Sign off with DCO (`git commit -s`).
- The maintainer is the sole author: no `Co-Authored-By` trailers and no mention of AI or tooling in commit messages, PR titles or PR bodies.
- Never force-push, rewrite published history, or bypass hooks.

## Where things are

| Path | What |
|---|---|
| `docs/PLAN.md` | Founding plan (A–P): competitive analysis, engine decision, architecture, roadmap |
| `docs/adr/` | Binding decisions. Read before changing anything structural. |
| `docs/milestones/` | Task breakdowns. **Start here.** |
| `docs/architecture/` | Overview, process model, data model, security, performance targets, dependency policy, testing |
| `docs/dev/` | Setup, coding standards, testing how-to |
| `crates/` | Rust workspace members |
| `app/` | C++/Qt shell (created in M0) |
| `fuzz/`, `bench/`, `tests/` | Fuzz targets, benchmarks, cross-crate tests and corpus manifests |
| `third_party/` | Pinned binary dependency manifests (PDFium) |
