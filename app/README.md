# app/ — C++/Qt 6 desktop shell

Created in **M0 task 22** ([`docs/milestones/M0.md`](../docs/milestones/M0.md)).

- C++20, Qt 6.8 LTS **Widgets**, CMake ≥ 3.28, with Corrosion building the Rust `vellora-engine-client` staticlib.
- **Never parses PDF data.** It talks to the engine only through the `engine-client` cxx bridge (ADR-0003, ADR-0004).
- Every user action is a registered command (menus, toolbars, shortcuts and the command palette derive from the registry).

Planned layout:

```
app/
├─ CMakeLists.txt
├─ src/
│  ├─ main.cpp
│  ├─ MainWindow.{h,cpp}
│  ├─ canvas/        # QRhiWidget tile compositor, page layout, scrolling
│  ├─ commands/      # command registry, palette, keymap
│  └─ bridge/        # thin adapters from engine-client (cxx) to Qt types/signals
├─ resources/
└─ tests/            # Qt Test
```
