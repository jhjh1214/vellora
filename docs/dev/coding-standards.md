# Coding standards

The hard rules are in [`CLAUDE.md`](../../CLAUDE.md). This page adds the day-to-day conventions.

## Rust

- **Edition 2024.** Workspace lints are mandatory (`[lints] workspace = true`). Only the FFI crates (`render`, `engine-client`) define their own lint table, to allow scoped `unsafe`.
- **Errors:** libraries define error enums with `thiserror`, carrying context such as byte offset, object ref or page index. Binaries may use `anyhow` at the top level. Never use `Box<dyn Error>` in library APIs.
- **Untrusted input:**
  - no `unwrap`/`expect`/`panic!`/`unreachable!` on data derived from a PDF
  - use `.get()` instead of indexing
  - checked arithmetic for sizes and offsets (`checked_add`, `try_from`)
  - all decoding goes through `cos::limits`
- **Allocation:** never pre-allocate from an input-declared length without capping it (`Vec::with_capacity(min(declared, limit))`).
- **Logging:** `tracing`. `debug`/`trace` may include object refs and offsets. `info` and above never include document text or metadata values.
- **API design:** small public surfaces. `pub(crate)` by default. Newtypes for ids (`ObjRef`, `PageIndex`). No boolean parameters where an enum reads better.
- **Docs:** every public item has a doc comment. Reference the spec, e.g. `ISO 32000-2:2020 §7.5.6`.
- **Tests:** name tests after behaviour (`incremental_save_preserves_original_prefix`). One behaviour per test. Property tests for parsers and serialisers.
- **Concurrency:** prefer message passing and owned data. PDFium access only through `vellora-render`'s thread.

## C++ (app/)

- C++20, Qt 6.8 Widgets. Format with `.clang-format`. Run `clang-tidy` with the repo config (added in M0 task 22).
- No PDF parsing. Everything document-related goes through the `engine-client` bridge.
- Qt parent/child ownership for widgets; `std::unique_ptr` otherwise. No raw `new` without an owner.
- Every user action is a registered **command** (id, title, shortcut, handler). Don't wire handlers directly to menu items.
- Accessibility: every interactive widget has an accessible name. The canvas implements `QAccessibleInterface` (Phase 1).

## Commits and PRs

Conventional Commits with DCO sign-off; see [CONTRIBUTING.md](../../CONTRIBUTING.md). One logical change per PR. Update docs and ADRs in the same PR.
