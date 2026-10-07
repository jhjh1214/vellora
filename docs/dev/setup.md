# Development setup

## Required (all milestones)

| Tool | Version | Notes |
|---|---|---|
| Rust | pinned in `rust-toolchain.toml` | Install via [rustup](https://rustup.rs). The pinned toolchain installs automatically. |
| Git | recent | Configure `user.name`/`user.email`; commit with `-s` (DCO) |
| cargo-deny | latest | `cargo install cargo-deny --locked` |
| curl | any recent | Used by `cargo xtask corpus fetch`. Preinstalled on Windows 10+, macOS and most Linux distributions |
| qpdf | ≥ 11 | Test oracle (`qpdf --check`). Windows: `winget install QPDF.QPDF`; Debian/Ubuntu: `apt install qpdf`; macOS: `brew install qpdf` |

**Windows:** install the *Visual Studio Build Tools* with the "Desktop development with C++" workload (the MSVC toolchain). Rust's `x86_64-pc-windows-msvc` target is the supported one.

## Required from M0 task 14 (rendering)

- PDFium prebuilt binaries: `cargo xtask pdfium fetch`. Pinned and checksum-verified via `third_party/pdfium.lock`, extracted to `third_party/pdfium/<platform>/` (git-ignored); re-running verifies and is a no-op. Needs `curl` and `tar` (both ship with Windows 10+, macOS and Linux). Supported hosts: Windows x64, Linux x64, macOS arm64 and x64.
- Run it before `cargo test --workspace`: the `vellora-render` tests and the `vellora-engine` end-to-end tests (which run the real engine executable) load the real library and **fail**, they do not skip, if it is missing. Building and clippy do not need it (PDFium is loaded at run time, ADR-0014). `VELLORA_PDFIUM_LIB` points the tests (and the engine) at another build.

## Required from M0 task 20 (engine client)

- A C++ compiler with C++20 (MSVC 2022, GCC ≥ 12 or Clang ≥ 15): `vellora-engine-client` builds the glue of its `cxx` bridge in `build.rs`, so every `cargo build`/`clippy`/`test` of the workspace needs one. Qt, CMake and the shell are not needed yet.
- The generated header is `vellora-engine-client/src/bridge.rs.h` under `target/<profile>/build/vellora-engine-client-*/out/cxxbridge/include/`.
- The bridge finds the engine through `VELLORA_ENGINE`, else `vellora-engine` next to the running executable.

## Required from M0 task 22 (desktop app)

| Tool | Version | Notes |
|---|---|---|
| CMake | ≥ 3.28 | Corrosion (pinned tag) is fetched by CMake at configure time, so the first configure needs network access |
| Ninja | any | Used as the generator in CI; any generator works except that Windows needs the MSVC environment |
| Qt | 6.8 LTS | qtbase (Widgets, Test) plus the **Qt Shader Tools** module (`qsb` compiles the canvas shaders). Qt Online Installer, or project-local with `uvx --from aqtinstall aqt install-qt windows desktop 6.8.3 win64_msvc2022_64 -m qtshadertools --archives qtbase -O .qt` (`.qt/` is git-ignored) |
| C++ compiler | MSVC 2022 or newer / GCC ≥ 12 / Clang ≥ 15 | C++20 |
| clang-format | 23.1.3 (CI pins it) | `uvx clang-format@23.1.3`, or `pip install clang-format==23.1.3`; style in `.clang-format` |

Build and test the app (the shell stages `vellora-engine` and PDFium next to itself, so run `cargo xtask pdfium fetch` first):

```sh
cmake -S app -B build -G Ninja -DCMAKE_BUILD_TYPE=Debug -DCMAKE_PREFIX_PATH=<Qt install>/6.8.3/<kit>
cmake --build build
ctest --test-dir build --output-on-failure   # Qt Test suites against the real engine
build/vellora <file.pdf>                      # run the shell (build/vellora.exe on Windows)
```

- **Windows:** run these from a shell where the MSVC environment is loaded (`vcvars64.bat`, or the "x64 Native Tools" prompt), and put the Qt `bin` directory on `PATH` so the tests find the Qt DLLs. All MSVC configurations, Debug included, use the release C runtime and release Qt libraries, because Rust links the release runtime and the two cannot be mixed.
- **Tests and the GPU:** most suites run on the `offscreen` platform (set by CTest), where `QRhiWidget` cannot render. `tst_canvas_render` is the one that draws: it needs a platform plugin that can run QRhi, so it runs on the real platform on Windows and macOS (D3D11/Metal, a software or virtual GPU is enough) and under `xvfb-run` with Mesa's software OpenGL on Linux (`xvfb`, `libgl1-mesa-dri`, `libxcb-cursor0` and the other xcb libraries; without `xvfb-run` the test is not registered). `VELLORA_RHI=null|opengl|vulkan|d3d11|d3d12|metal` forces a graphics backend.
- **Linux (headless):** Qt needs `libgl1-mesa-dev`, `libxkbcommon-x11-0`, `libegl1` and `libfontconfig1`.
- The cxx bridge header (`bridge.rs.h`, `rust/cxx.h`) is written by `vellora-engine-client`'s `build.rs` to `$VELLORA_CXXBRIDGE_DIR/include` (CMake sets it to `build/cxxbridge`), else to `target/<profile>/cxxbridge/include`.
- Check formatting with `clang-format --dry-run -Werror $(find app -name '*.cpp' -o -name '*.h')`.

## Useful commands

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --all
cargo deny check
cargo xtask corpus fetch      # from M0 task 1
cargo xtask pdfium fetch      # from M0 task 14
cargo xtask bench             # from M0 task 24
cargo +nightly fuzz run lexer # from M0 task 13 (cd fuzz)
```
