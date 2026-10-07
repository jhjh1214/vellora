//! Generates the C++ header and glue for the `cxx` bridge (`src/bridge.rs`) and compiles the glue
//! into the library, so the Qt shell links one static library and includes one header.

use std::env;
use std::error::Error;
use std::path::PathBuf;

fn main() -> Result<(), Box<dyn Error>> {
    cxx_build::bridge("src/bridge.rs")
        .std("c++20")
        .compile("vellora_engine_client_bridge");

    // Where `cxx-build` writes the header; the bridge's tests check that it exists and what it
    // declares.
    let out_dir = PathBuf::from(env::var_os("OUT_DIR").ok_or("cargo did not set OUT_DIR")?);
    let header = out_dir.join("cxxbridge/include/vellora-engine-client/src/bridge.rs.h");
    println!("cargo:rustc-env=VELLORA_BRIDGE_HEADER={}", header.display());
    println!("cargo:rerun-if-changed=src/bridge.rs");
    println!("cargo:rerun-if-changed=build.rs");
    Ok(())
}
