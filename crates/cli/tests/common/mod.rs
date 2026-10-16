//! Helpers shared by the end-to-end tests of the `vellora` binary.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

pub(crate) fn root(relative: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join(relative)
}

pub(crate) fn stdout(out: &Output) -> String {
    String::from_utf8(out.stdout.clone()).expect("utf-8 output")
}

pub(crate) fn stderr(out: &Output) -> String {
    String::from_utf8(out.stderr.clone()).expect("utf-8 output")
}

pub(crate) fn plain() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vellora"))
}

/// `vellora` with the engine and PDFium it needs for rendering and text.
pub(crate) fn with_engine() -> Command {
    let mut command = plain();
    let exe = PathBuf::from(env!("CARGO_BIN_EXE_vellora"));
    let name = if cfg!(windows) {
        "vellora-engine.exe"
    } else {
        "vellora-engine"
    };
    let engine = exe.with_file_name(name);
    assert!(
        engine.is_file(),
        "the engine is not built at {}: run `cargo build -p vellora-engine`",
        engine.display()
    );
    command.env("VELLORA_ENGINE", engine);
    let pdfium = std::env::var_os("VELLORA_PDFIUM_LIB").map_or_else(
        || {
            let (platform, file) = if cfg!(windows) {
                ("win-x64", "bin/pdfium.dll")
            } else if cfg!(target_os = "macos") {
                (
                    if cfg!(target_arch = "aarch64") {
                        "mac-arm64"
                    } else {
                        "mac-x64"
                    },
                    "lib/libpdfium.dylib",
                )
            } else {
                ("linux-x64", "lib/libpdfium.so")
            };
            root("../../third_party/pdfium").join(platform).join(file)
        },
        PathBuf::from,
    );
    assert!(
        pdfium.is_file(),
        "PDFium not found at {}: run `cargo xtask pdfium fetch`",
        pdfium.display()
    );
    command.env("VELLORA_PDFIUM_LIB", pdfium);
    command
}

pub(crate) fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buffer = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buffer).unwrap();
    assert_eq!(info.color_type, png::ColorType::Rgb);
    buffer.truncate(info.buffer_size());
    (info.width, info.height, buffer)
}
