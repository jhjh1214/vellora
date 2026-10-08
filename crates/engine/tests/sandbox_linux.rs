//! The engine's Linux sandbox (M1 task 5, ADR-0018): what a compromised engine cannot do.
//!
//! `examples/sandbox_probe.rs` tries each forbidden thing before and after `sandbox::apply` and
//! reports both. A refusal counts only if the same action worked before, so a refusal cannot be an
//! accident of the environment. Linux only. The kernel must have Landlock (the report says so); a
//! kernel without it fails this test instead of passing it quietly.

#![cfg(target_os = "linux")]
// Test code: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::env;
use std::path::Path;
use std::process::Command;

/// `EPERM` and `EACCES`.
const REFUSED: [&str; 2] = ["err(1)", "err(13)"];

fn run_probe() -> (String, HashMap<String, (String, String)>) {
    let exe = env::current_exe().unwrap();
    let probe = exe
        .parent()
        .and_then(Path::parent)
        .unwrap()
        .join("examples")
        .join("sandbox_probe");
    assert!(
        probe.is_file(),
        "{} is missing: run `cargo test -p vellora-engine` (it builds the examples)",
        probe.display()
    );
    let output = Command::new(&probe).output().unwrap();
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(output.status.success(), "the probe failed:\n{text}");
    let mut probes = HashMap::new();
    for line in text.lines() {
        if let Some((name, rest)) = line.split_once(": before=")
            && let Some((before, after)) = rest.split_once(" after=")
        {
            probes.insert(name.to_owned(), (before.to_owned(), after.to_owned()));
        }
    }
    (text, probes)
}

#[test]
fn a_sandboxed_engine_cannot_reach_what_a_compromised_one_would_want() {
    let (text, probes) = run_probe();
    eprintln!("{text}");
    assert!(
        text.contains("landlock: Ok(Full)"),
        "this kernel does not enforce Landlock in full:\n{text}"
    );
    assert!(text.contains("seccomp: Ok(())"), "{text}");

    let result = |name: &str| {
        probes
            .get(name)
            .unwrap_or_else(|| panic!("no line for {name}:\n{text}"))
    };

    // What the engine needs keeps working.
    for name in [
        "read the library directory",
        "read its own /proc entry",
        "start a thread",
    ] {
        assert_eq!(result(name), &("ok".to_owned(), "ok".to_owned()), "{name}");
    }

    // Everything else worked before and is refused after.
    for name in [
        "read a file outside the allowed set",
        "create a file",
        "read the parent's /proc entry",
        "a socket",
        "start a program",
    ] {
        let (before, after) = result(name);
        assert_eq!(before, "ok", "control: {name} must work unsandboxed");
        assert!(
            REFUSED.contains(&after.as_str()),
            "{name} must be refused, got {after}"
        );
    }
}
