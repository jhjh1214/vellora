# 0004. One sandboxed engine process per document

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

PDFs are hostile input. PDFium and its codecs are C/C++ with a history of CVEs. PDFium isn't thread-safe. A renderer crash must not cost the user their session.

## Decision

- Each open document gets its own **`vellora-engine` process**. It holds the document session and the only PDFium instance in that process, with PDFium calls on a single thread.
- The engine receives **handles, not paths**. It has no network, and it never writes files: saves are returned as bytes, and the UI writes them atomically.
- **M0** ships: process isolation, handle passing, in-code limits (`cos::limits`), OS resource limits (Windows Job object; `setrlimit` on Linux and macOS), per-job deadlines, and a watchdog with auto-restart.
- **Progressive hardening:**
  - Windows: AppContainer / restricted token
  - Linux: seccomp-bpf + Landlock + namespaces
  - macOS: sandbox profile

  Evaluate existing crates before writing our own. Progress is tracked in the [security model](../architecture/security-model.md).
- The CLI runs pure-Rust commands in-process and spawns the engine for anything that needs PDFium.

## Alternatives considered

| Option | Why not |
|---|---|
| In-process with a global lock | A PDFium crash kills the app; no privilege separation |
| Threads + one PDFium per thread | PDFium has global state; not supported |
| Chromium's sandbox library | Heavy, C++, a large integration cost for a solo project |

## Consequences

- An IPC layer and shared-memory tiles are needed from day one (ADR-0005).
- Each commit re-opens the revision in the engine. Latency is measured in M0.
- Memory per document includes a process overhead (a few MB).
