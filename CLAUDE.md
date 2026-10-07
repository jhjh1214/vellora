# CLAUDE.md — Vellora implementation rules

These rules apply to every implementation session, human or AI. They are derived from the approved plan ([`docs/PLAN.md`](docs/PLAN.md)) and the ADRs ([`docs/adr/`](docs/adr/)). **If a task seems to require breaking a rule, stop and ask the maintainer. Don't redesign silently.**

## What Vellora is

A local-first, open-source (MPL-2.0) PDF workstation:
- a Rust core
- a C++/Qt 6 Widgets desktop shell
- PDFium as a read-only renderer, running in a sandboxed engine process

## Session protocol: one task per session

Each implementation session does **exactly one milestone task**, then ends. The milestone file is the only memory between sessions, so keep it accurate.

`main` is protected: no direct pushes, no force-pushes, PRs only, squash-merge only, and every CI check must pass. Every task therefore lands as **one pull request**.

**Start (keep reading minimal):**
1. `git switch main && git pull --ff-only`, then `git status`. The tree must be clean. If not, stop and report.
2. Open **`docs/milestones/M0.md`**. Read the header, then find the **first unchecked task** whose dependencies are all checked. Read that task and the `Note:` lines under the tasks it depends on.
3. Read only the docs and ADRs that task links to, plus the source files it touches. Don't read the whole `docs/` tree or `docs/PLAN.md` unless the task requires it.
4. Create the task branch: `git switch -c m0/task-NN-short-name` (e.g. `m0/task-06-classic-xref`).

**Work:**

5. Implement within the task's scope. Don't start the next task. Don't add features no task asks for.
6. Run the task's **acceptance checks** and the standard checks below. Read your own diff.

**Finish:**

7. Tick the task (`- [x]`) in the milestone file. Under it, add `Note:` lines with anything the next session needs: decisions made, deviations, gotchas, commands that differ from the plan. Keep each note to a single line.
8. Commit with sign-off (`git commit -s`; task code + milestone tick together), Conventional Commits, e.g. `feat(cos): parse classic xref tables (M0 task 6)`.
9. Push the branch and open the PR:
   - `gh pr create --title "<same Conventional Commit subject>" --body "<what/why, how verified, save-fidelity impact>"`
   - The PR title becomes the squash commit subject on `main`, so it must be a valid Conventional Commit.
10. Watch CI with `gh pr checks --watch`.
    - Fix failures on the same branch.
    - If the session prompt says to merge: once all checks are green, run `gh pr merge --squash --delete-branch`, then `git switch main && git pull --ff-only`.
    - Otherwise leave the PR open for the maintainer.
11. Report the real CI result, the PR URL, what you ran and what you couldn't run.

**Stop and ask instead of continuing** if:
- the task is ambiguous or contradicts an ADR
- it needs a decision the task doesn't make
- it turns out much bigger than described (propose a split into e.g. 7a/7b in the milestone file)
- CI is red for reasons outside your task

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
6. **`unsafe` is forbidden** (workspace lint) except in `vellora-render`, `vellora-engine-client` (FFI) and `vellora-shm` (memory mapping and handle passing, ADR-0015). Every `unsafe` block there needs a `// SAFETY:` comment. `cos` and `vellora-engine` stay `forbid`.
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

Conventional Commits, e.g. `feat(cos): parse cross-reference streams (M0 task 7)`.
- Scopes: crate or area names.
- Sign off with DCO (`git commit -s`).
- The maintainer is the sole author: no `Co-Authored-By` trailers and no mention of AI or tooling in commit messages, PR titles or PR bodies.
- Never force-push, rewrite published history, bypass hooks, or try to bypass branch protection.

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
