# 0015. A dedicated crate for OS primitives, and the engine launch contract

- **Status:** Accepted
- **Date:** 2026-10-08
- **Deciders:** @jhjh1214

## Context

The engine host (M0 task 17) needs three things that cannot be written in safe Rust:

1. **Mapping the document** (`memmap2::Mmap::map` is `unsafe`). ADR-0013 measured 1.7 ms and 5 MB to open a 524 MB file mapped, against 176 ms and 505 MB read into memory. Task 3 had already left the question open of where a mapped `ByteSource` may live, because `cos` forbids `unsafe`.
2. **Mapping the shared tile region** that the UI and the engine both read and write (ADR-0005).
3. **Adopting inherited handles** (`from_raw_fd`, `from_raw_handle`): ADR-0004 says the engine receives handles, never paths.

Invariant 6 allowed `unsafe` only in `vellora-render` and `vellora-engine-client`. Putting these three in the engine host would make the crate that also runs the session logic, the IPC loop and the cross-check allow `unsafe`; putting them in `engine-client` would pull a UI-side crate into the engine.

## Decision

- Add **`vellora-shm`**, a small crate that holds exactly these primitives and nothing else: `MappedFile` (read-only mapping, implements `cos::ByteSource`), `TileRegion` and `SlotGeometry` (the shared region), and `share_with_child` / `stop_sharing` / `adopt` (handle passing). It has no PDF knowledge and no protocol.
- `vellora-shm` sets `unsafe_code = "deny"` and `undocumented_unsafe_blocks = "deny"`; the three modules that need `unsafe` opt in with `#[allow(unsafe_code)]` and every block has a `// SAFETY:` comment. Invariant 6 now names it as the third crate allowed to do so. `vellora-engine` and `cos` stay `forbid`.
- Both the engine and, later, the engine client depend on it, so the two ends of the region and of handle passing are one implementation.

**Launch contract** (`vellora_engine::args` is the single definition; `LaunchArgs::to_args` builds the command line, `parse` reads it):

- IPC runs over the engine's **standard input and output** (ADR-0005 allows either that or pipes passed at spawn). Standard error carries logs, never document content at `info` level or above.
- The UI opens the document and creates the tile region, marks both **inheritable** (`share_with_child`), spawns the engine with `--file-handle`, `--region-handle`, `--region-slots`, `--region-slot-bytes` and optionally `--max-document-bytes`, then calls `stop_sharing` so later children do not inherit them.
- The engine **validates every number it adopts** (not a standard stream, an open handle, a regular file) and refuses a region file shorter than the geometry.
- `Open { handle_token }` must carry the same number as `--file-handle`; anything else is `InvalidRequest`.
- A failure to start after the arguments are parsed is reported over the protocol (`Hello`, then `Error`) before the process exits, so the UI can show the reason.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| `unsafe` allowed inside `vellora-engine` | Least work | The crate with the session logic and the request loop would allow `unsafe`; each review of it would have to look for it |
| Put it in `engine-client` | Matches "FFI crate" wording | The engine would depend on the UI-side crate and on its future `cxx`/staticlib build |
| No shared memory in M0 (positioned file writes) | No `unsafe` at all | Changes the process model of `process-model.md`; the UI would read tiles through file I/O; throws away what task 21 and the QRhi upload path assume |
| Read the document with `FileSource` instead of mapping | No `unsafe` | 100x slower open and the whole file resident for a 500 MB document; the M0 10k-page and 500 MB acceptance numbers depend on mapping |
| Pass paths and let the engine open them | Trivial | Gives the untrusted process filesystem reach (ADR-0004) |
| Named OS shared memory (`shm_open`, named file mapping) | In-memory backing | Per-OS code in three APIs and a name another process could open; see below |

## Consequences

- Positive: the whole `unsafe` surface of the engine side is about 150 lines in three modules of one crate, each with unit tests, and `vellora-engine` is entirely safe code.
- Positive: `MappedFile` closes task 3's open question: it implements `cos::ByteSource` and is also what PDFium reads, so `cos` and PDFium share one mapping.
- Negative: a file that shrinks while mapped ends the engine with SIGBUS or an access violation (documented on `MappedFile`). It is the same outcome as any engine crash (the client restarts it); the parent holds the file open for reading and should deny writers where the OS allows (invariant 3).
- Negative: the tile region is backed by an **anonymous temporary file**, not by memory alone. On a machine whose temporary directory is on a disk, the OS may write finished tiles back. `TileRegion::create` hides the backing, so `memfd` (Linux), `shm_open` (macOS) or a pagefile-backed section (Windows) can replace it once profiling shows the cost, with no change to either side.
- Negative: the engine's writable handle to the region also allows it to shrink the backing file on Linux and macOS, so a compromised engine can make the UI fault when it reads a lost page (a denial of service of the UI, not a memory-safety or data-exposure problem). It is listed in the security model; a sealed `memfd` region on Linux closes it.
- Negative: between `share_with_child` and `stop_sharing` another thread's child process also inherits the files. The engine client must spawn from one place, serialised (task 20).
- Follow-ups: the engine client (task 20) uses `TileRegion::create`, `share_with_child` and `LaunchArgs`; task 19 adds OS limits to the spawn; the Unix half of `handle.rs` is first exercised on Linux and macOS by CI (developed on Windows).

## References

- ADR-0004, ADR-0005, ADR-0013, ADR-0014
- `crates/shm`, `crates/engine/src/args.rs`
