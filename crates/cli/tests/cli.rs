//! End-to-end tests for the `vellora` binary.

use std::process::Command;

fn vellora() -> Command {
    Command::new(env!("CARGO_BIN_EXE_vellora"))
}

#[test]
fn version_flag_prints_package_version() {
    let out = vellora().arg("--version").output().expect("run vellora");
    assert!(out.status.success());
    let stdout = String::from_utf8(out.stdout).expect("utf-8 output");
    assert_eq!(stdout.trim(), format!("vellora {}", env!("CARGO_PKG_VERSION")));
}

#[test]
fn unknown_command_is_a_usage_error() {
    let out = vellora().arg("no-such-command").output().expect("run vellora");
    assert_eq!(out.status.code(), Some(2));
    let stderr = String::from_utf8(out.stderr).expect("utf-8 output");
    assert!(stderr.contains("unknown command 'no-such-command'"));
}

#[test]
fn no_arguments_is_a_usage_error() {
    let out = vellora().output().expect("run vellora");
    assert_eq!(out.status.code(), Some(2));
}
