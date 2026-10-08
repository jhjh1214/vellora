# 0017. Engine sandbox on Windows

- **Status:** Accepted
- **Date:** 2026-10-09
- **Deciders:** @jhjh1214

## Context

[ADR-0004](0004-process-model-and-sandboxing.md) promises a progressively hardened engine process: on Windows an AppContainer or a restricted token. Through M0 the engine ran as a normal child of the UI with a job object (memory cap, no child processes, kill-on-close) applied *after* it had started, and it could still read everything the user can, use the network and enumerate the registry. M1 task 4 closes that. The engine is the process that runs PDFium and the image codecs on hostile input, so a successful exploit there should gain as little as possible.

What the engine genuinely needs: the executable and the PDFium library, the document and the tile region (inherited handles), its standard input and output, and the system fonts (PDFium maps non-embedded fonts to them).

## Decision

`vellora-engine-client` starts the engine with `CreateProcessW` and builds the sandbox itself (`crates/engine-client/src/sandbox.rs`):

- **AppContainer with no capabilities.** The token has an AppContainer SID and no capability SIDs: no network (the loopback is blocked too), no access to the user's files, registry or other processes' objects. The SID is *derived* from the fixed name `Vellora.Engine` (`DeriveAppContainerSidFromAppContainerName`); **no profile is registered**, so nothing is written to the registry or to `%LOCALAPPDATA%\Packages`. Measured: with the profile deleted, a derived SID still starts the process.
- **Read access for the container to exactly two files**: the engine executable and the PDFium library (`$VELLORA_PDFIUM_LIB`, else `pdfium.dll` next to the engine). The grant is one ACL entry for the container SID, read and execute, on the file only (not on any directory), added once per file and process. Best effort: where the files are already readable by `ALL APPLICATION PACKAGES` (an installation under `Program Files`) the user may not change the ACL and does not need to. If the container really cannot read them, process creation fails and the error says so. The container needs nothing on the parent directories (checked: `NT AUTHORITY\Authenticated Users`-only ancestors in a user profile are traversed).
- **An explicit handle list** (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`): only the document, the tile region, and the three standard streams are inherited, whatever else the UI holds that is inheritable. This also narrows the share/spawn window of [ADR-0015](0015-os-primitives-crate-and-engine-launch-contract.md) on Windows.
- **Created suspended, jailed, then resumed.** The job object is assigned before the first instruction runs, which closes the "job applied just after start" gap. The job gets what it had (memory cap, kill-on-close, no child processes, die on unhandled exception) plus UI restrictions: no clipboard, global atoms, display settings, desktop switching, logoff, or other processes' USER handles.
- **A minimal environment.** The engine receives only `SystemRoot`, `SystemDrive`, `windir`, `OS`, `PROCESSOR_ARCHITECTURE`, `NUMBER_OF_PROCESSORS`, `RUST_BACKTRACE`, `LOCALAPPDATA` and `VELLORA_*`, plus what the caller passes. `LOCALAPPDATA` is not a courtesy: `CreateProcess` fails with `ERROR_ENVVAR_NOT_FOUND` for an AppContainer without it. The working directory is `%SystemRoot%\System32`.
- **Standard error** is a copy of the UI's if that is a file or a pipe, else the null device (a console cannot be handed to a container).
- **Fail closed.** If the sandbox cannot be built, the engine does not start (`SpawnError::Sandbox`). There is no switch to run it unsandboxed.

Resource limits are unchanged in meaning and live in `limits.rs`; `confine` now takes a process handle so the sandbox can call it on the suspended process.

### Measured outcome

Windows 11 (build 26200), x86-64, debug build, PDFium from `cargo xtask pdfium fetch`.

| Check | Result |
|---|---|
| PDFium finds the system fonts | A page of text in the non-embedded `SegoeUI` renders **pixel for pixel the same** sandboxed and bare; a font that exists nowhere renders differently, so the test can tell (`crates/engine/tests/sandbox.rs`) |
| Inherited handles (document, region, pipes) | Work; the document is read through its handle |
| A file in the user's profile | `ERROR_ACCESS_DENIED` |
| TCP connect to a listener on `127.0.0.1` | Refused (times out); the listener never sees a connection |
| UDP datagram to a receiver on `127.0.0.1` | Never delivered |
| Starting a child process | Refused (`ERROR_NOT_ENOUGH_QUOTA` from the job) |
| Token | AppContainer, 0 capabilities |
| A handle that was inheritable but not listed | Not inherited |
| Corpus v0 through the sandboxed engine (`crates/engine/tests/corpus.rs`) | 257 of 265 non-password files opened (97.0%), 256 rendered page 1, 0 crashes or hangs, 0 in the 54 malformed files: identical to the M0 run |
| Cost | Spawn to `Hello` of a do-nothing engine: median 19 ms sandboxed, 6 ms bare (35 runs) |

**What an AppContainer does not do:** *creating* or *binding* a socket still succeeds (`UdpSocket::bind` on the loopback works); only traffic is blocked, by the network isolation of the platform. M1 task 4 asked for a test that the engine "cannot create a socket"; the achievable and meaningful property is that it cannot connect or send, and that is what the test asserts. The control in that test is the same connect and send from the unsandboxed test process.

**Strict handle checks.** Observed while writing the probe: using a handle number that is not valid inside the container ends the process with `STATUS_INVALID_HANDLE` (an exception) instead of returning an error. The engine only uses handles it was given and validated (`vellora_shm::adopt`), but code that probes handle numbers would crash it.

### The fallback

The task allowed a restricted token with a low integrity level if PDFium's font enumeration or the inherited handles failed in the container. Neither did, so **the fallback is not implemented**.

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Restricted token + low integrity level | Needs no ACL change; works for any install location | Weaker: the process keeps network access and can still read what low integrity may read; no isolation of the registry. Kept as the documented fallback if a future PDFium needs something the container denies |
| Registering an AppContainer profile (`CreateAppContainerProfile`) | The container gets its own folders and registry mapping | Writes outside our files for no benefit the engine uses; also needs deleting on uninstall. Measured not to be required |
| Chromium's sandbox library | Mature, many mitigations | Heavy C++ integration for a solo project (ADR-0004); would own the process launch |
| Windows Sandbox / Hyper-V | Strongest | Per-document VM: far too slow and not available on Home editions |
| A `ProcessMitigationPolicy` set (CFG, ACG, image-load policies) on top | Cheap extra hardening | Not part of this task; see follow-ups |

## Consequences

- Positive: a compromised engine cannot read the user's files, reach the network, or start programs, and no instruction of it runs outside the job.
- Positive: nothing persists outside the two ACL entries; uninstalling deletes the files and with them the entries.
- Negative: the container needs read access to the engine and the PDFium library. An installer must put them where `ALL APPLICATION PACKAGES` can read them (the default under `Program Files`); a per-user install relies on the launcher's file-level grant.
- Negative: about 13 ms more per engine start (not per tile).
- Negative: creating a socket is still possible (see above).
- Follow-ups (`docs/backlog.md`): process mitigation policies (win32k lockdown has not been tried with PDFium's font mapping), a Low-integrity-level token on top of the container's, and the Linux and macOS counterparts (M1 task 5, M6).

## References

- ADR-0004, ADR-0015
- `crates/engine-client/src/sandbox.rs`, `crates/engine-client/tests/sandbox.rs`, `crates/engine/tests/sandbox.rs`
- Microsoft Learn: *Implementing an AppContainer*, `PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES`, `PROC_THREAD_ATTRIBUTE_HANDLE_LIST`
