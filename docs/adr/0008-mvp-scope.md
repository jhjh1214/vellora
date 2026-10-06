# 0008. MVP scope

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

There is one core developer. The MVP must be small enough to finish, yet show at once why Vellora exists: fast, faithful, safe, with review and compare that no open-source tool offers.

## Decision

The MVP is **v0.5** (end of roadmap Phase 5) and contains:
1. A viewer: tiled async rendering, thumbnails, outline, links, zoom modes, encrypted files.
2. Text selection and search (case, whole word, regex, streaming).
3. Organize mode: multi-select ranges, reorder, rotate, delete, duplicate, insert blank, import, extract, merge, split.
4. Standard annotations with threaded replies and status, with our own appearance streams.
5. Object editing tiers 1–2: add text and images; move, resize or delete images and paths; delete text runs.
6. Incremental save by default, Save Clean, the save-impact bar, undo/redo, atomic writes.
7. Inspector (read-only) and a security report.
8. Text compare: page alignment, word diff, change categories including number changes, change navigator, sidecar review state.
9. CLI: `inspect`, `security`, `merge`, `split`, `extract`, `rotate`, `delete-pages`, `compare`, `render`, all with `--json`.
10. Platforms: Windows and Linux (Flatpak) at the MVP; macOS at MVP+1.

**Not in the MVP:** in-place text editing, redaction, sanitisation, forms, signatures, OCR, visual and object diff, optimisation, preflight, accessibility tooling, conversion, pipelines, plugins.

## Consequences

- In-place text editing (the hardest feature) comes straight after the MVP, built on the content-surgery infrastructure the MVP proves.
- Performance targets: see [performance-targets.md](../architecture/performance-targets.md).
