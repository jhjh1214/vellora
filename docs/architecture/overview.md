# Architecture overview

Vellora is a desktop PDF workstation made of a **C++/Qt 6 shell** and a **Rust core**. The core runs in a **sandboxed engine process per document**. This page is the map; details live in the linked documents and the [ADRs](../adr/).

## Design principles

1. **Faithful:** we own the write path. Untouched bytes stay byte-identical, and saves explain their impact. → [data-model.md](data-model.md)
2. **Safe:** hostile input is parsed only in a resource-limited, sandboxed process, by memory-safe code wherever we wrote it. → [security-model.md](security-model.md)
3. **Fast at scale:** lazy loading, tiled rendering, nothing blocks the UI thread. → [performance-targets.md](performance-targets.md)
4. **Boring engineering:** few crates, few dependencies, no speculative abstraction. Boundaries are added when a phase needs them.

## Two engines, one authority

| | `vellora-cos` (ours, Rust) | PDFium (Google, BSD/Apache) |
|---|---|---|
| Role | **Authority on document structure**; the **only writer** | Read-only rasteriser, glyph geometry, form interaction |
| Why | Faithful incremental updates, byte-span preservation, real redaction and signatures require owning the write path | Best real-world rendering compatibility; continuously fuzzed by Chromium |
| Never | — | Used to save or modify documents (its content regeneration is lossy) |

PDFium always renders the **current revision** that `cos` produces: the original bytes plus our pending incremental sections, served from memory. On open, the two parsers' views are cross-checked. If they disagree, the document is marked *Repaired*, and saving it requires a normalising full rewrite. → [ADR-0002](../adr/0002-engine-split.md)

## Process model

```
┌──────────── UI process (C++/Qt 6 Widgets) ─────────────┐
│ windows · modes · canvas (QRhiWidget) · command registry│
│            ↕ cxx (in-process, narrow API)               │
│ vellora-engine-client (Rust): IPC, tile cache           │
└──────────────▲──────────────────────────────────────────┘
               │ pipes (postcard) + shared memory (tiles) + passed handles
┌──────────────┴──── vellora-engine (Rust), one per document, sandboxed ─┐
│ session: cos object store + revisions + history                        │
│ scheduler (priority, cancel, deadlines) · inspect · (later: content,   │
│ doc, ops, diff) · vellora-render → PDFium (single thread)              │
└─────────────────────────────────────────────────────────────────────────┘
CLI: links core crates in-process; spawns vellora-engine when it needs rendering.
```

→ [process-model.md](process-model.md), [ADR-0004](../adr/0004-process-model-and-sandboxing.md), [ADR-0005](../adr/0005-ipc-protocol.md)

## Crates

| Crate | Responsibility | Exists from |
|---|---|---|
| `cos` | Lexer, parser, xref (table/stream/hybrid/repair), lazy object store, filters, decryption, writers, limits | M0 |
| `inspect` | Summary, feature and security analysis, later size attribution | M0 |
| `render` | Safe PDFium wrapper; engine only | M0 |
| `ipc` | Protocol types and framing | M0 |
| `engine` | Engine host binary | M0 |
| `engine-client` | UI-side client staticlib + cxx bridge | M0 |
| `cli` | `vellora` binary | M0 |
| `content` | Content-stream tokeniser with byte spans, interpreter, page objects with provenance, patches | Phase 4 |
| `doc` | Typed views over COS, transactions → ChangeSets, history, save planner | Phase 2 |
| `ops` | Serialisable operations shared by GUI commands, CLI and pipelines | Phase 2 |
| `diff` | Page alignment, text diff, change model | Phase 5 |

Dependency direction: `cli`, `engine` → `ops` → `doc` → `content` → `cos`. Also `inspect`/`diff` → `doc`/`cos`, `engine` → `render`, and `engine`/`engine-client` → `ipc`. No cycles. The UI depends only on `engine-client`.

## UI shell (summary)

- Qt 6.8 LTS **Widgets** (dense professional UI, mature accessibility on all three OSes). The canvas is a custom `QRhiWidget` compositing GPU tiles.
- A **command registry** drives menus, toolbars, shortcuts, the command palette and customisable keymaps. Command ids match `ops` names.
- **Modes** (View / Organize / Annotate / Edit / Review / Redact / Forms) scope the tools. *Organize* is a full-window page grid.
- The canvas exposes text, links, annotations and fields through a custom `QAccessibleInterface`.

## Where to read more

- Founding plan with full reasoning: [`../PLAN.md`](../PLAN.md)
- Roadmap: [`../roadmap.md`](../roadmap.md)
- Testing: [testing-strategy.md](testing-strategy.md)
- Dependencies and licenses: [dependency-policy.md](dependency-policy.md)
