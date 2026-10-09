//! `THIRD_PARTY_LICENSES` lists every Rust crate that ends up in a Vellora binary.
//!
//! The file is generated (see `third_party/README.md`) and shown in the About dialog, so a crate
//! that was added to `Cargo.lock` and is missing from it is a licence notice the product does not
//! give. This test asks `cargo tree` for what is really linked (the normal dependencies of every
//! workspace member, on every platform, with the features that are switched on) and fails when a
//! package is not in the file. Build tools and development dependencies are not distributed, so
//! they need no notice (the generated file lists the build dependencies of workspace members
//! anyway).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeSet;
use std::path::Path;
use std::process::Command;

fn root() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// `name version` of every package from a registry in the output of `cargo tree`.
fn shipped_packages() -> BTreeSet<String> {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let output = Command::new(cargo)
        .args([
            "tree",
            "--workspace",
            "--edges",
            "normal",
            "--target",
            "all",
            "--prefix",
            "none",
            "--format",
            "{p}",
            "--locked",
            "--offline",
        ])
        .current_dir(root())
        .output()
        .expect("cargo tree runs");
    assert!(
        output.status.success(),
        "cargo tree failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let mut packages = BTreeSet::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        let mut parts = line.split_whitespace();
        let (Some(name), Some(version)) = (parts.next(), parts.next()) else {
            continue;
        };
        // A workspace member is shown with its path, which is not a registry package.
        let is_member = parts
            .next()
            .is_some_and(|rest| rest.starts_with('(') && rest != "(proc-macro)" && rest != "(*)");
        if !is_member {
            packages.insert(format!("{name} {}", version.trim_start_matches('v')));
        }
    }
    packages
}

#[test]
fn every_shipped_crate_is_in_third_party_licenses() {
    let file = std::fs::read_to_string(root().join("THIRD_PARTY_LICENSES"))
        .expect("THIRD_PARTY_LICENSES exists");
    let shipped = shipped_packages();
    assert!(
        shipped.len() > 50,
        "suspiciously few packages: {}",
        shipped.len()
    );
    let missing: Vec<&String> = shipped
        .iter()
        .filter(|package| !file.contains(&format!("  - {package}")))
        .collect();
    assert!(
        missing.is_empty(),
        "THIRD_PARTY_LICENSES is out of date: it lacks {missing:?}. Regenerate it as described in \
         third_party/README.md."
    );
    // The binary components are named too.
    for component in ["PDFium", "Qt"] {
        assert!(file.contains(component), "{component} is not mentioned");
    }
}
