//! A machine that has never run the engine: the sandbox must register its `AppContainer` profile
//! itself (M1 task 4, ADR-0017). Without a registered profile `CreateProcess` fails with "file not
//! found", which passed unnoticed locally because an earlier run had left the profile behind.
//!
//! One test in a binary of its own, because the engine client registers the profile once per
//! process and a sibling test could have done that already. Windows only.

#![cfg(windows)]
// Test code: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    unsafe_code,
    clippy::undocumented_unsafe_blocks
)]

use std::env;
use std::io::BufReader;
use std::path::Path;

use vellora_engine_client::{EngineProcess, SpawnConfig};
use vellora_ipc::{PROTOCOL_VERSION, Response, read_frame};
use vellora_shm::{SlotGeometry, TileRegion};
use windows_sys::Win32::Security::Isolation::DeleteAppContainerProfile;

/// The name the client registers; the test removes it to get the state of a fresh machine.
const CONTAINER_NAME: &str = "Vellora.Engine";

#[test]
fn an_engine_starts_when_its_container_profile_does_not_exist_yet() {
    let name: Vec<u16> = CONTAINER_NAME.encode_utf16().chain([0]).collect();
    // Succeeds if the profile existed and fails harmlessly if it did not.
    unsafe { DeleteAppContainerProfile(name.as_ptr()) };

    let exe = env::current_exe().unwrap();
    let engine = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("examples")
        .join("fake_engine.exe");
    assert!(
        engine.is_file(),
        "{} is missing: run `cargo test -p vellora-engine-client` (it builds the examples)",
        engine.display()
    );
    let geometry = SlotGeometry::new(1, 4096).unwrap();
    let document = tempfile::tempfile().unwrap();
    let (_region, region_file) = TileRegion::create(geometry).unwrap();

    let mut process = EngineProcess::spawn(&SpawnConfig::new(
        &engine,
        &document,
        &region_file,
        geometry,
    ))
    .expect("the sandbox must create the container profile it needs");
    let mut output = BufReader::new(process.take_stdout().unwrap());
    let hello: Option<Response> = read_frame(&mut output).unwrap();
    assert_eq!(
        hello,
        Some(Response::Hello {
            protocol_version: PROTOCOL_VERSION
        })
    );
}
