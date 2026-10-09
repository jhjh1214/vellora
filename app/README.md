# app/ — C++/Qt 6 desktop shell

Created in **M0 task 22b** ([`docs/milestones/M0.md`](../docs/milestones/M0.md)).

- C++20, Qt 6.8 LTS **Widgets**, CMake ≥ 3.28, with Corrosion building the Rust `vellora-engine-client` staticlib.
- **Never parses PDF data.** It talks to the engine only through the `engine-client` cxx bridge (ADR-0003, ADR-0004).
- Every user action is a registered command (menus, toolbars, shortcuts and the command palette derive from the registry).

Layout:

```
app/
├─ CMakeLists.txt     # Qt 6.8 + Corrosion; builds the shell, stages vellora-engine and PDFium beside it
├─ src/
│  ├─ main.cpp
│  ├─ MainWindow.{h,cpp}        # menu bar, tabs, File -> Open / Open Recent, status bar
│  ├─ diagnostics/CrashReports.*, CrashDialog.*  # crash dumps (monitor process), the next-start dialog
│  ├─ diagnostics/Logging.*          # Qt messages -> the application log (rotating files)
│  ├─ AboutDialog.*, LicensesDialog.*   # Help -> About, third-party licenses (embedded THIRD_PARTY_LICENSES)
│  ├─ DocumentTab.{h,cpp}       # one document: engine session, canvas, repair bar, password prompt
│  ├─ SingleInstance.{h,cpp}    # a second launch forwards its files to the running instance
│  ├─ settings/AppSettings.*    # recent files, geometry, per-document view state (QSettings)
│  ├─ bridge/EngineSession.*    # QObject over the cxx bridge: polled events -> signals, tiles
│  ├─ canvas/
│  │  ├─ PageLayout.*           # page rows (one or two pages a row, rotation) and their geometry (pure)
│  │  ├─ CanvasController.*     # what is on screen, which tiles to ask for / cancel, zoom anchoring
│  │  ├─ CanvasWidget.*         # QRhiWidget: draws the controller's frame (shaders/ -> qsb)
│  │  └─ CanvasView.*           # scroll area + wheel/keys around the widget
│  ├─ commands/                 # CommandRegistry (every action), CommandPalette (Ctrl+Shift+P)
│  └─ diagnostics/             # Application (times event delivery), UiWatchdog (8 ms frame budget), DiagnosticsScript
└─ tests/             # Qt Test suites; they run the real engine (cargo xtask pdfium fetch first)
```

Build and test: see [`docs/dev/setup.md`](../docs/dev/setup.md).

## Frame budget and the diagnostics script

Our code gets 8 ms of every 16.7 ms frame. `diagnostics/UiWatchdog` counts what goes over it, in two measurements: the time `vellora::Application::notify` spends delivering one event (so handlers, timers, queued slots; paint and update requests are left out because they contain the window's vsynced present, which is not ours) and the time of every `CanvasWidget::render()`. Debug builds log every violation (`vellora.watchdog`); `VELLORA_UI_WATCHDOG=1|0` forces it. A test that wants the timing uses `VELLORA_TEST_MAIN` (`tests/VelloraTestMain.h`) instead of `QTEST_MAIN`.

`DiagnosticsScript` scrolls one page per step and zooms in and out on the way (default: 2,000 pages, 20 zooms). `tst_canvas_render` runs it over a 10,000-page document and asserts zero over-budget samples. The app runs the same script with the hidden flag `vellora --diagnostics-script scroll-zoom <file.pdf>`, prints the report and exits 0.

## macOS and the canvas test

`tst_canvas_render` stays disabled on the macOS CI runner (set `VELLORA_TEST_GPU=1` to run it). Finding (M1 task 1, PR run with a temporary probe, five runs on `macos-15`): it is not a deadlock of the event loop. Stack samples of the four runs still going after 5 minutes show the UI thread busy, inside Qt's Metal present (`QRhiMetal::endOffscreenFrame` waiting for the command buffer), copying into GPU buffers and releasing them through Apple's paravirtualised GPU driver. The first run finished in 305 s (Windows, local: 62 s): 2,000 frames, 17 over 8 ms, worst `render()` 32.9 ms, worst handler 22.6 ms (Windows D3D11: worst 4.6 and 2.9 ms). So the earlier intermittent "hangs" are timeouts of a very slow virtual GPU, and the frame-time check cannot pass there. macOS frame times are measured on real hardware for M6.

The same holds for `tst_thumbnails_frame_time` (M1 task 11), which needs no GPU at all (the offscreen platform, repaints with `QPainter`) and is disabled on macOS CI too (set `VELLORA_TEST_FRAME_TIME=1` to run it). Evidence, three runs of the fling test on `macos-15` (PR #49): handlers worst 7.5, 4.3 and 20.5 ms, with 0, 0 and 2 over 8 ms, and sidebar repaints worst 33.2, 27.5 and 10.5 ms, with 2, 1 and 2 over 8 ms. The same test on Windows (local, three runs): handlers worst 0.5 ms, repaints worst 0.9 to 1.1 ms; on the Linux runner it passed every time. The functional tests of the sidebar (`tst_thumbnails`) run everywhere, including the check that a fling makes no requests until the list stops.