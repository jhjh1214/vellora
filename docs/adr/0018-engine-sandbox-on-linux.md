# 0018. Engine sandbox on Linux

- **Status:** Accepted
- **Date:** 2026-10-09
- **Deciders:** @jhjh1214

## Context

[ADR-0004](0004-process-model-and-sandboxing.md) promises seccomp-bpf and Landlock for the Linux engine, and [ADR-0017](0017-engine-sandbox-on-windows.md) did the Windows half. Through M0 the Linux engine ran as an ordinary child with `RLIMIT_AS` and `RLIMIT_CPU`. It could read every file the user can, open sockets, start programs and signal other processes, and, through its writable handle to the tile region, shrink the shared file and fault the UI.

What the engine genuinely needs: the document and the tile region (inherited descriptors), its standard streams, the PDFium library, the system fonts, and threads.

## Decision

**A sealed tile region.** On Linux `TileRegion::create` backs the region with a `memfd` (`memfd_create`, `MFD_CLOEXEC | MFD_ALLOW_SEALING`) sealed with `F_SEAL_SHRINK | F_SEAL_GROW | F_SEAL_SEAL` once it is sized. The engine's writable handle can no longer resize it, which closes the gap listed in ADR-0015. If the kernel refuses `memfd_create`, the old temporary file is used. macOS keeps the gap.

**The engine sandboxes itself** (`crates/engine/src/sandbox.rs`, called from `main` before the PDFium thread starts and before any document byte is read). Landlock and seccomp bind the calling thread and the threads it creates afterwards, so it has to come before the renderer thread. It sets:

- **Landlock**, ABI 3 handled in best-effort mode: every file-system right is handled and only reading is granted back, under `/usr/share/fonts`, `/usr/local/share/fonts`, `/usr/share/X11/fonts`, `/etc/fonts`, `/usr/lib`, `/usr/lib64`, `/lib`, `/lib64`, the directory of the PDFium library, and the process's own `/proc/self` (not `/proc`: the UI's entry there is readable by the same user and holds its environment). The engine cannot open, create, rename or delete anything else. Already open descriptors keep working.
- **No new privileges** (set by both mechanisms).
- **seccomp-bpf**, a deny list answering `EPERM`: `execve`/`execveat`, `fork`/`vfork` (x86-64), `clone` without `CLONE_THREAD`, the socket family (`socket`, `socketpair`, `connect`, `bind`, `listen`, `accept`, `accept4`), `ptrace`, `process_vm_*`, `pidfd_*`, `tkill`, queued signals, `kill`/`tgkill` to any pid but its own (`abort` still works), `mount`/namespace calls, `open_by_handle_at`, `bpf`, `perf_event_open`, keyrings, `userfaultfd`, kernel module and `kexec` calls, `reboot`, `io_uring_*`. `clone3` answers `ENOSYS`, which makes the C library fall back to the inspected `clone`. Threads keep working.
- **Degrade, never fail.** A kernel without Landlock, or with only part of it, or a failure to install a filter, is reported by `sandbox::apply` and logged at `warn` by the engine, which carries on.

Crates: `landlock` 0.4 (MIT OR Apache-2.0, the reference binding maintained with the kernel feature) and `seccompiler` 0.5 (Apache-2.0 OR BSD-3-Clause, from the Firecracker project, pure Rust, builds the BPF program). Both expose safe APIs, so `vellora-engine` stays free of `unsafe`; `libc` supplies the system-call numbers. `extrasafe` was not chosen: it bundles opinionated profiles on top of the same two mechanisms and we need a specific list. All pass `cargo deny`.

### Deviations from the task text

- **A deny list, not an allow list.** The task asked for an allow list of system calls. Without a Linux machine to trace a real session on (the Linux code was written on Windows and first ran in CI), an allow list would have to be guessed, and a missing entry kills the engine with `SIGSYS` on someone else's glibc, kernel or PDFium build. The deny list closes what the task named (no program, no process, no socket, no way into other processes) and is tightened with traced data later (backlog).
- **Applied before PDFium loads, not after.** PDFium is loaded by its own thread, which is created after the sandbox. Instead of loading it first, the sandbox leaves its directory readable, along with the font directories. The cost is that those directories stay readable; the library file is there anyway.
- **"No `open` of new paths except via Landlock"** is just Landlock: seccomp does not look at paths.

### Measured outcome

CI (Ubuntu runner, kernel with Landlock): `crates/engine/tests/sandbox_linux.rs` runs the probe `examples/sandbox_probe.rs`, which tries each action before and after `sandbox::apply` on a thread created after it. It requires `landlock: Full` and the filter installed, then for `/etc/passwd`, creating a file, the parent's `/proc/<pid>/environ`, a UDP socket and starting `/bin/true`: works before, `EPERM` or `EACCES` after; reading `/usr/lib`, its own `/proc/self` and starting a thread: works both times. The existing engine, client and Qt test suites run against the self-sandboxed engine on Linux and pass, which exercises PDFium loading and rendering under the sandbox. A unit test checks the seals on the tile region.

**Not measured:** the corpus gate and the system-font rendering comparison were only run on Windows (the corpus gate is not part of CI until M1 task 20), so on Linux PDFium's behaviour on real documents under the sandbox is covered by the golden-page tests only. The `kill`/`tgkill` rules are untested (the probe cannot send signals without `unsafe`).

## Alternatives considered

| Option | Pros | Cons |
|---|---|---|
| Allow-list seccomp | Smallest surface | Needs traced data for PDFium on each libc; a miss is a crash (see above) |
| Namespaces (user, mount, net) via `unshare` | Strong, hides the file system entirely | Unprivileged user namespaces are disabled on several distributions and in Flatpak; Landlock needs nothing |
| A seccomp filter on `open`/`openat` paths | None | Cannot look at paths |
| Apply the sandbox in the UI at spawn (`pre_exec`) | Applies before the first instruction | The engine could not load PDFium afterwards, and the UI would need `unsafe` for it |
| `extrasafe` | Convenient | See above |

## Consequences

- Positive: a compromised engine on Linux cannot read the user's files, start programs, make network connections, or reach the UI process, and cannot shrink the shared region.
- Negative: on a kernel without Landlock (before 5.13, or the module disabled) only the seccomp half applies, with a warning. Distribution kernels differ.
- Negative: font directories outside the listed ones are not readable, so PDFium would fall back to its built-in fonts for them.
- Follow-ups (`docs/backlog.md`): tighten the deny list into an allow list from traced sessions; add the Linux corpus gate and a font comparison to nightly; check the Flatpak build in task 23 (filters stack inside its sandbox, and `/usr` is the runtime).

## References

- ADR-0004, ADR-0015, ADR-0017
- `crates/engine/src/sandbox.rs`, `crates/engine/examples/sandbox_probe.rs`, `crates/engine/tests/sandbox_linux.rs`, `crates/shm/src/region.rs`
- Linux documentation: *Landlock*, *seccomp*, `memfd_create(2)`, `fcntl(2)` file seals
