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

- PDFium prebuilt binaries: `cargo xtask pdfium fetch`. Pinned and checksum-verified via `third_party/pdfium.lock`.

## Required from M0 task 22 (desktop app)

| Tool | Version | Notes |
|---|---|---|
| CMake | ≥ 3.28 | |
| Qt | 6.8 LTS | Qt Online Installer or `aqtinstall`. Modules: qtbase only for M0. |
| C++ compiler | MSVC 2022 / GCC ≥ 12 / Clang ≥ 15 | C++20 |
| clang-format | ≥ 17 | Style in `.clang-format` |

Build the app:

```sh
cmake -S app -B build -DCMAKE_PREFIX_PATH=<Qt install>/6.8.x/<kit>
cmake --build build
```

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
