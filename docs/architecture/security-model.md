# Security model

**Assumption:** every PDF is potentially hostile. Parser bugs will exist, in PDFium, its codecs, and our code. The architecture limits what a successful exploit gains and how much a malicious file can consume.

## Trust boundaries

```
 user + OS  ──trusted──  UI process  ══pipes/shm══  engine process (untrusted, sandboxed)  ──▶ PDFium, codecs
```

- The engine gets **no paths, no network, no new handles**. The UI passes it exactly the file handle(s) it needs.
- The UI never parses PDF bytes. It treats engine replies as untrusted data: size-capped and validated.

## Threats and mitigations

| Threat | Mitigation |
|---|---|
| Memory corruption in PDFium / FreeType / OpenJPEG / libjpeg | Engine isolation + resource limits (M0) → per-OS sandbox (progressive: Windows AppContainer + restricted token + Job object; Linux seccomp-bpf + Landlock + namespaces; macOS sandbox profile). Watchdog restart. Monthly PDFium updates. |
| Bugs in our parser | Rust with `unsafe` forbidden; no panics on input; continuous fuzzing (cargo-fuzz in CI and nightly → OSS-Fuzz once public) |
| Decompression bombs / memory exhaustion | `cos::limits`: per-stream and total decode caps, ratio cap, image dimension caps. Process memory cap. Streaming decode. |
| CPU exhaustion (deep nesting, loops, giant content streams) | Depth limits, reference-cycle detection, job deadlines + cancellation, watchdog kill |
| Malicious fonts / images | Decoded only inside the engine (PDFium) or by memory-safe Rust crates |
| JavaScript | PDFium built **without V8**. Form calculations later use native AF* function implementations. JS is reported, never run. |
| Launch / GoToR / ImportData / SubmitForm / URI actions | Launch never executes. URIs need confirmation showing the full URL. SubmitForm is off by default. Remote GoTo prompts. |
| Embedded files, path traversal | Never auto-opened or executed. Save only via a dialog. Filenames sanitised (separators, reserved names, NTFS ADS, control characters). |
| External references (remote content, OCSP/TSA) | No network for document content. Revocation and timestamp requests (Phase 9) are explicit or opt-in and go through the UI process, never the engine. |
| Signature spoofing (USF, ISA, SWA, shadow attacks) | Strict ByteRange rules; independent re-parse of the signed revision; render signed vs current revision; DocMDP/FieldMDP classification of later changes; "valid but modified after signing" shown distinctly |
| Visual-signature confusion | Appearance-only signatures are labelled; cryptographic status is shown separately |
| Redaction leaks | Mandatory full rewrite + GC. Scrub metadata, outline, structure tree (Alt/ActualText), form values, thumbnails, OCR layers and attachments. Post-apply verification with independent extractors. |
| Differential parsing (`cos` vs PDFium disagree) | Open-time cross-check → *Repaired* mode forces a normalised rewrite. Signatures are validated only through `cos`. |
| Supply chain | `cargo-deny` (licenses, advisories, sources), reviewed dependency additions, PDFium pinned by SHA-256, CycloneDX SBOM per release, signed releases, reproducible builds as a goal |
| Document passwords | The password goes from the UI's prompt to the engine in `Open` (UTF-8, at most 256 bytes) and nowhere else. It is never logged at any level (`Debug` is redacted; `crates/engine/tests/password.rs` runs the engine at `trace` and searches its log), never written to disk and never put in a setting or recent-files list. Every buffer the code controls is wiped when dropped (`zeroize`): the message, the frame buffers, the forms the engine tries (as typed, SASLprep, Latin-1). The client keeps an accepted password in memory only so that a crashed engine can reopen the document without asking again; a refused one is dropped at once. Not covered: copies the OS keeps (pipe buffers, swap) and PDFium's own copy inside the engine. |
| Future plugins | Out-of-process only, same sandbox, broker-mediated capabilities. No in-process native plugins. |

## Privacy

- **No telemetry.** No automatic crash upload. The update check is opt-in.
- Crash dumps are written locally. The user decides whether to attach them to an issue.
- Logs never contain document text at `info` level or above.

## Sandbox status tracker

| Platform | Process isolation | Resource limits | Handle-only FS access | OS sandbox |
|---|---|---|---|---|
| Windows | M0 | M0 (Job object) | M0 | **M1 task 4** (`AppContainer`, no capabilities; [ADR-0017](../adr/0017-engine-sandbox-on-windows.md)) |
| Linux | M0 | M0 (rlimit) | M0 | **M1 task 5** (Landlock + seccomp deny list + sealed tile region; [ADR-0018](../adr/0018-engine-sandbox-on-linux.md)) |
| macOS | M0 | M0 (CPU only; memory not enforced) | M0 | before macOS release |

On Linux the engine sandboxes itself before the renderer thread starts: Landlock leaves only fonts, libraries, the PDFium directory and `/proc/self` readable, and a seccomp deny list refuses programs, processes, sockets and reaching other processes ([ADR-0018](../adr/0018-engine-sandbox-on-linux.md)). On Windows the engine is started by `sandbox.rs` in an `AppContainer` with no capabilities (no network, no user files), with only the document, the tile region and its standard streams inherited, and is put under its job **before it runs** (ADR-0017). It can read the system fonts and, besides the inherited handles, only the engine executable and the PDFium library, to which the launcher gives the container read access file by file.

Resource limits are applied by `vellora-engine-client` at spawn (`limits.rs`): Windows job object (memory, kill-on-close, no child processes, optional CPU time), Linux `RLIMIT_AS` and `RLIMIT_CPU`, macOS `RLIMIT_CPU`. A tile past its hard deadline makes the engine abort itself; the client sees the pipe close and reports a typed crash.

The client enforces three deadlines from outside, so a wedged engine cannot hang the UI (M1 task 2, `vellora-engine-client`): the engine must say `Hello` within 10 s of starting and answer `Open` within 30 s of its `Hello` (both configurable in `ClientConfig`; the engine is killed and the UI gets a typed `EngineTimeout`, no restart); and while tiles are in flight some answer must arrive within twice the hard tile deadline of the last one, or the engine is killed and restarted like after a crash.

### Known gaps in the engine boundary

| Gap | Effect | Planned fix |
|---|---|---|
| The engine holds a writable handle to the shared tile region, so a compromised engine on macOS can shrink the file | The UI faults (SIGBUS) when it reads a lost page: a denial of service of the UI process, no data exposure or corruption. Not possible on Windows. **Linux: closed in M1 task 5**: the region is a `memfd` sealed against shrinking and growing | `shm_open` on macOS (evaluate with the macOS sandbox), see ADR-0015 |
| macOS ignores `RLIMIT_AS` and `RLIMIT_DATA`, so the engine's memory is not capped there | A memory bomb can take the machine's memory until the OS intervenes; only the in-code limits (`cos::limits`) and the hard deadline apply | Find a macOS-specific memory cap (to be evaluated with the macOS sandbox profile) |
| The engine may start child processes on macOS (the Windows job and the Linux seccomp filter forbid it) | A compromised engine could run programs as the user | The sandbox profile (macOS) with the per-OS sandbox. Linux: closed in M1 task 5 (ADR-0018) |
| The Linux seccomp filter is a deny list, not an allow list | A system call nobody thought of stays available (for example `prctl` options, `memfd_create`, `setuid` on a file system the engine can no longer reach) | Trace real sessions and tighten (backlog) |
| The Windows job was applied just after the process started, not at creation | The engine ran for a few milliseconds outside its limits; it read no untrusted data until the UI sent `Open` | Closed in M1 task 4: the process is created suspended, put in its job, then resumed (ADR-0017) |
| An `AppContainer` can still create and bind sockets | No data leaves: connections and datagrams, even to the loopback, are blocked by the platform (tested). Creating a socket is not itself prevented | Accepted; nothing to fix on Windows |
| No process mitigation policies on the Windows engine (ACG, CFG-only images, win32k lockdown) | A memory-corruption exploit has fewer obstacles inside the container than it could have | Evaluate with PDFium's GDI font mapping (backlog) |
| No CPU-time limit by default | A render that spins is ended by the hard deadline (default 10 s), not by the OS. A hang outside a tile (in `Open`, or a wedged process) is ended by the client's deadlines (see above) | Closed in M1 task 2; an OS CPU limit stays optional |
| The engine maps the document, so a file truncated by another process kills the engine (not the UI) | **Windows: closed.** The document is opened with `FILE_SHARE_READ` only, so no other process can write, rename or delete it while it is open (M1 task 2). **Linux and macOS:** only a shared advisory `flock` (stops cooperating writers); other writers still get through. The client restarts the engine, compares size, mtime and file identity of the file and its path at each restart and tells the user the file changed | Not preventable without mandatory locks; the detection is the mitigation. A copy-on-open mode is a possible later hardening |
