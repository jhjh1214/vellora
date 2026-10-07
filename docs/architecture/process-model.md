# Process model

## Processes

| Process | Language | Trust | Owns |
|---|---|---|---|
| **UI** (`vellora` desktop app) | C++/Qt + Rust client staticlib | Trusted. Never parses PDF bytes. | Windows, user file access (dialogs), OS key stores (signing, later), network (opt-in features only), crash-dump writing |
| **Engine** (`vellora-engine`), one per open document | Rust + PDFium | **Untrusted**: assume compromise is possible | Document session, parsing, rendering, analysis, serialisation of new revisions |
| Render helpers (later) | Rust + PDFium | Untrusted | Extra read-only PDFium instances for parallel tiles |
| OCR worker (Phase 10) | Rust + Tesseract | Untrusted | OCR jobs |
| CLI (`vellora`) | Rust | Runs pure-Rust core in-process; spawns an engine for rendering | — |

## Lifecycle

1. The user opens a file. The UI opens a **read handle** (the engine never gets a path) and spawns `vellora-engine` with:
   - pipes for IPC (the engine's standard input and output)
   - the file handle, inherited and named by number
   - a shared-memory region for tiles, inherited and named by number, with its slot geometry
   - limits: memory cap, CPU deadline defaults

   The exact command line is defined in `vellora_engine::args` ([ADR-0015](../adr/0015-os-primitives-crate-and-engine-launch-contract.md)).
2. The engine and client exchange a **protocol-version handshake**. A mismatch is a fatal, user-visible error.
3. The engine parses lazily (trailer and xref only), loads the current revision into PDFium, cross-checks the page tree, and replies with page count, page sizes and repair status.
4. The UI requests tiles for visible pages. The engine schedules them (visible > prefetch > thumbnails), renders them into shared-memory slots and replies with slot descriptors. Superseded requests are **cancelled**.
5. **Crash or timeout:** the client detects EOF or a missed deadline, shows a recoverable error, and respawns the engine. The document reopens at the same position; pending in-memory edits are replayed from the client-side journal (from Phase 2).
6. **Save:** the engine serialises output (an incremental section or a full rewrite) into a buffer or stream, and sends it to the UI. The UI writes to a temp file next to the target, fsyncs, and renames atomically. The engine never writes files.

## Threading inside the engine

- One **PDFium thread** (PDFium isn't thread-safe). All `vellora-render` calls are marshalled onto it.
- A small worker pool for pure-Rust work (parsing, decoding, analysis), with cooperative cancellation tokens.
- The IPC reader and writer run on their own thread(s). No blocking work happens on them.

## Resource limits (defence in depth)

| Layer | Mechanism |
|---|---|
| In-code | `cos::limits`: max decoded stream size, total decode budget, decompression-ratio cap, object nesting depth, xref entry cap, image dimension caps |
| Per job | Deadlines with cancellation; the watchdog kills the engine if a PDFium call exceeds its hard deadline |
| Per process | **Windows:** Job object (memory limit, kill-on-close, no child processes). **Linux:** `setrlimit` (+ cgroup v2 when available). **macOS:** `setrlimit` |
| Sandbox (progressive) | **Windows:** AppContainer / restricted token. **Linux:** seccomp-bpf + Landlock + namespaces. **macOS:** sandbox profile. See [security-model.md](security-model.md). |

M0 delivers process isolation, handle passing, resource limits and watchdog restart. Full per-OS sandbox hardening lands progressively (tracked in the roadmap); its absence is documented, not hidden.

## Shared-memory tiles

- A fixed-size region is split into slots (e.g. 256×256 or 512×512 RGBA tiles at device scale). The client owns slot allocation and LRU eviction; the engine writes only into the slot it is assigned per request.
- IPC carries `{request_id, page, scale, rect, slot}`. Pixels never cross the pipe.
- The UI uploads finished slots to GPU textures (QRhi) and frees or recycles slots.

## Why processes, not threads

- PDFium's lack of thread safety and its C++ attack surface are both contained by process boundaries.
- A crash costs tiles, not the session.
- The memory-safety boundary (Rust vs C++) lines up with the trust boundary.
