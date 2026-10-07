//! Generates the C++ header and glue for the `cxx` bridge (`src/bridge.rs`) and compiles the glue
//! into the library, so the Qt shell links one static library and includes one header.

use std::env;
use std::error::Error;
use std::fs;
use std::path::{Path, PathBuf};

fn main() -> Result<(), Box<dyn Error>> {
    cxx_build::bridge("src/bridge.rs")
        .std("c++20")
        .compile("vellora_engine_client_bridge");

    // Where `cxx-build` writes the header; the bridge's tests check that it exists and what it
    // declares.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("cargo did not set OUT_DIR")?);
    let header = out_dir.join("cxxbridge/include/vellora-engine-client/src/bridge.rs.h");
    println!("cargo:rustc-env=VELLORA_BRIDGE_HEADER={}", header.display());
    publish_headers(&out_dir)?;
    println!("cargo:rerun-if-changed=src/bridge.rs");
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}

/// Environment variable naming the directory the headers are published to (the `CMake` build of the
/// Qt shell sets it, because cargo's own output directory contains a hash).
const HEADER_DIR_ENV: &str = "VELLORA_CXXBRIDGE_DIR";

/// Copies the generated header and `rust/cxx.h` to `$VELLORA_CXXBRIDGE_DIR/include`, or else to
/// `<profile dir>/cxxbridge/include`. A file is rewritten only when its content changes, so C++ is
/// not rebuilt for nothing.
fn publish_headers(out_dir: &Path) -> Result<(), Box<dyn Error>> {
    println!("cargo:rerun-if-env-changed={HEADER_DIR_ENV}");
    let base = match env::var_os(HEADER_DIR_ENV) {
        Some(dir) => PathBuf::from(dir),
        // OUT_DIR is `<profile dir>/build/<package>-<hash>/out`.
        None => out_dir
            .ancestors()
            .nth(3)
            .ok_or("OUT_DIR is not inside a cargo profile directory")?
            .join("cxxbridge"),
    };
    let from = out_dir.join("cxxbridge/include");
    let to = base.join("include");
    for relative in ["vellora-engine-client/src/bridge.rs.h", "rust/cxx.h"] {
        let bytes = fs::read(from.join(relative))?;
        let target = to.join(relative);
        if fs::read(&target).ok().as_deref() == Some(bytes.as_slice()) {
            continue;
        }
        if let Some(dir) = target.parent() {
            fs::create_dir_all(dir)?;
        }
        fs::write(&target, bytes)?;
    }
    Ok(())
}
