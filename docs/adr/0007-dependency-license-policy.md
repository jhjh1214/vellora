# 0007. Dependency license policy enforced by cargo-deny

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

Accidentally depending on a copyleft or non-commercial library would compromise distribution under MPL-2.0 (ADR-0001).

## Decision

- Linked dependencies must be permissively licensed (allow list in `deny.toml`).
- **Qt 6** is the only LGPL exception. It's dynamically linked, LGPL modules only, and never linked statically.
- GPL, AGPL and LGPL source code is never copied into the repository.
- AGPL/GPL tools (e.g. Ghostscript) may only be invoked as optional, user-installed external executables.
- `cargo deny check` runs in CI on every PR. A new non-Rust dependency requires an ADR.

## Consequences

Details and the list of adopted dependencies: [dependency-policy.md](../architecture/dependency-policy.md).
