# 0003. Rust core, C++/Qt 6 Widgets shell, `cxx` bridge

- **Status:** Accepted
- **Date:** 2026-10-07
- **Deciders:** @jhjh1214

## Context

We need memory safety for the code we write that parses and rewrites hostile input. We also need a toolkit for a dense, professional, accessible desktop UI on Windows, Linux and macOS.

## Decision

- **Rust** for all core code: `cos`, `inspect`, `ipc`, `engine`, `engine-client`, `cli`, and later `content`, `doc`, `ops`, `diff`.
- **C++20 + Qt 6.8 LTS Widgets** for the desktop shell. The canvas is a `QRhiWidget` compositing GPU tiles. The shell never parses PDF data.
- The shell links `vellora-engine-client` (a Rust staticlib) through **`cxx`**. We don't use `cxx-qt`: it's pre-1.0 (v0.10 in 2026), and we don't need Rust-defined QObjects. No Qt types cross the bridge.
- Build: a Cargo workspace, plus CMake for `app/`, using Corrosion to build the Rust staticlib from CMake.

## Alternatives considered

| Option | Why not |
|---|---|
| All C++ | No memory safety in our own hostile-input code |
| All Rust (Slint / egui / iced) | Not yet at Qt's level for docking, native dialogs, dense UIs and screen-reader maturity |
| Tauri (webview) | Extra copies for tiles; tends toward "web dashboard" UX; WebKitGTK inconsistency |
| GTK 4 | Weak on Windows and macOS |
| Flutter | Non-native feel; desktop text and accessibility maturity |
| Qt Quick / QML | Weaker for dense tool UIs; may host the canvas later if needed |

## Consequences

- Two languages raise the bar for contributors. This is mitigated by a narrow, RPC-shaped bridge and documented setup.
- The trust boundary (engine vs UI) coincides with the language boundary.
- Qt must stay dynamically linked (LGPL; ADR-0007).
