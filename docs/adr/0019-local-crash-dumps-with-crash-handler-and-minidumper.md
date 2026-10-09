# 0019. Local crash dumps with `crash-handler` and `minidumper`

- **Status:** Accepted
- **Date:** 2026-10-09
- **Deciders:** @jhjh1214

## Context

[ADR-0012](0012-no-telemetry-local-crash-dumps.md) decides that crashes produce minidumps, written locally, that nothing is uploaded, and that the user chooses what to attach to a report. It leaves the tool open ("Breakpad/Crashpad-compatible tooling"). M1 task 9c has to pick one.

What it must do:

- Catch a crash of the **UI process** (C++ and Rust code in one process, so an access violation or a signal anywhere in it) and write a minidump. A crashed process is a bad place to write files, so the dump is written by **another process**.
- Work on Windows, Linux and macOS, from the CI we already have, with licenses `cargo deny` accepts ([ADR-0007](0007-dependency-license-policy.md)).
- Record what happens to the **engine** (a sandboxed child that cannot write files) with its last log lines.
- Upload nothing, ever.

## Options

**A. Crashpad.** Chromium's handler. Mature and BSD-licensed, but it is built with GN and its own toolchain, ships a separate `crashpad_handler` binary per platform and brings a database format of its own. It would add a large C++ build to every CI job and to the packaging of task 23, to produce files we only need to hand to a person.

**B. Breakpad.** The older library Crashpad grew out of. Same build problem, less maintained.

**C. `crash-handler` + `minidumper` (Rust, MIT/Apache-2.0, from Embark Studios).** `crash-handler` installs the platform's exception mechanism (Windows structured exceptions and the C runtime's abort and purecall hooks, POSIX signals on Linux, Mach exceptions on macOS), so it also catches crashes in the C++ and Qt code of the same process. `minidumper` is the other half: a small server in a **separate process** that receives the crash context over a local socket and writes the dump with `minidump-writer`. About 40 more crates, all permissive; `cargo deny` accepts them.

**D. Nothing in-process, rely on the operating system** (Windows Error Reporting, systemd-coredump, macOS crash reports). Not under our control, not uniform, and some of it uploads.

## Decision

Option C.

- The UI process installs the handler at start-up (`vellora_engine_client::crash`, reached through the `cxx` bridge) and starts the **monitor**: the same executable run as `vellora --crash-monitor <socket> <folder>`, handled in `main` before any GUI is created. The monitor owns no windows and exits when the UI disconnects. Shipping one executable keeps packaging simple, and keeps the dump code (and its dependencies) out of the engine process, which handles hostile input.
- A crash makes the handler ask the monitor for a dump and wait until it is written. Files go to `crashes/` inside the log folder (`VELLORA_CRASH_DIR` overrides): `crash-<time>-<pid>.dmp` and a text file beside it with the version, commit and operating system. At most the 20 newest reports are kept.
- An engine that ends unasked, or reports an internal error (a caught panic), is recorded **by the UI process**, because the engine cannot write files: `engine-crash-<time>-<pid>.txt` with what happened and the last 200 log lines the UI read from the engine's standard error ([task 9b](../milestones/M1.md)). At most one every ten seconds, so a crash loop does not flood the folder.
- On the next start the shell finds reports newer than the last time it asked and shows a dialog: **Open crash folder**, **Report on GitHub** (opens the project's issue form with the version, commit and operating system filled in and the *names* of the files; nothing is attached) and **Dismiss**. Whichever is chosen, it does not ask again for those reports.
- Nothing is uploaded: there is no code that sends a report anywhere, and the monitor has no network use.

### What a dump contains

The dumps are of the "normal" kind: the exception, the registers and stacks of every thread and the list of loaded modules, **not** the heap. Stacks can still hold fragments of what the program was doing (a file name, in rare cases text), so the dialog tells the user to look before attaching one, and the GitHub form never attaches anything by itself. A password lives on the heap or on a short-lived stack for a moment (task 7 wipes it after use), and the engine's log lines never contain one (tested).

### Test hooks

A deliberate crash is needed to test all this. `vellora --crash-test` (hidden, like `--diagnostics-script`) crashes the UI process after installing the handler. Debug builds of the engine honour `VELLORA_TEST_ENGINE_CRASH=panic|abort`; release builds do not contain that code.

## Consequences

- Roughly 40 more crates in the UI process, listed in `THIRD_PARTY_LICENSES` (the check of task 9a enforces it). `unsafe` is needed once: `CrashHandler::attach` is unsafe because its callback runs in a crashed process. It lives in one small module of `vellora-engine-client`, one of the crates where `unsafe` is allowed, with a `// SAFETY:` comment (CLAUDE.md invariant 6).
- On Linux the monitor must be allowed to inspect the crashed process (`PR_SET_PTRACER` for its pid; some hardened kernels still refuse, then the dump is missing and only the log shows the crash).
- If the monitor cannot be started, the application runs without crash dumps and says so in its log.
- A crash that kills the monitor too (a shutdown, the OOM killer) leaves nothing; that is the price of not using the operating system's own reporting.
- Reports are public the moment someone attaches them to an issue, so the contents above are stated in the dialog rather than hidden.