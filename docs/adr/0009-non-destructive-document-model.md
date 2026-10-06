# 0009. Non-destructive document model and explicit save modes

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

Users must never unexpectedly lose fidelity. Signatures survive only incremental updates. Incremental updates leave deleted content recoverable, which matters for privacy.

## Decision

- The original file is an immutable `ByteSource`. Edits are **ChangeSets** layered on top. Views are computed over COS objects and cached by change epoch.
- Content edits splice operator byte ranges. New content goes into appended streams.
- **Incremental save** is the default. A **full rewrite** is used for redaction, sanitise, optimise, Save Clean and *Repaired* documents.
- A **save planner** produces a `SaveImpact` (mode, reasons, signature effects, recoverable content, lossy steps), which the UI and CLI show before writing.
- **Write protocol:** temp file in the same directory → fsync → self-check re-parse → atomic rename. The source is never overwritten in place.

## Consequences

- Undo/redo, "show original" and diff-against-original come almost for free.
- Details and the fidelity matrix: [data-model.md](../architecture/data-model.md).
