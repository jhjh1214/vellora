# app/ — C++/Qt 6 desktop shell

Created in **M0 task 22b** ([`docs/milestones/M0.md`](../docs/milestones/M0.md)).

- C++20, Qt 6.8 LTS **Widgets**, CMake ≥ 3.28, with Corrosion building the Rust `vellora-engine-client` staticlib.
- **Never parses PDF data.** It talks to the engine only through the `engine-client` cxx bridge (ADR-0003, ADR-0004).
- Every user action is a registered command (menus, toolbars, shortcuts and the command palette derive from the registry).

Layout (items marked *later* arrive in the sub-tasks of M0 task 22):

```
app/
├─ CMakeLists.txt     # Qt 6.8 + Corrosion; builds the shell, stages vellora-engine and PDFium beside it
├─ src/
│  ├─ main.cpp
│  ├─ MainWindow.{h,cpp}        # menu bar, File -> Open, status bar
│  ├─ bridge/EngineSession.*    # QObject over the cxx bridge: polled events -> signals, tiles
│  ├─ canvas/
│  │  ├─ PageLayout.*           # page column geometry (pure)
│  │  ├─ CanvasController.*     # what is on screen, which tiles to ask for / cancel, zoom anchoring
│  │  ├─ CanvasWidget.*         # QRhiWidget: draws the controller's frame (shaders/ -> qsb)
│  │  └─ CanvasView.*           # scroll area + wheel/keys around the widget
│  └─ commands/                 # later (22d): command registry, palette, keymap
└─ tests/             # Qt Test suites; they run the real engine (cargo xtask pdfium fetch first)
```

Build and test: see [`docs/dev/setup.md`](../docs/dev/setup.md).
