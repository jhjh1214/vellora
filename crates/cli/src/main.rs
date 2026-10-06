//! # vellora — command-line interface
//!
//! **Responsibility:** user-facing CLI over the same core crates the desktop
//! app uses. Commands map one-to-one onto core operations, so GUI, CLI and
//! future pipelines share one vocabulary.
//!
//! - Every command supports `--json`.
//! - Exit codes are meaningful: 0 ok, 1 operation failed, 2 usage error.
//! - Pure-Rust commands (inspect, page operations) run in-process. Commands
//!   that need rendering spawn `vellora-engine`.
//!
//! **Status:** skeleton (`--version` / `--help` only). M0 task 23 adds
//! argument parsing and `vellora inspect`.

use std::process::ExitCode;

const USAGE: &str = "\
Usage: vellora <COMMAND> [OPTIONS]

Commands are added during milestone M0 (first: `inspect`).

Options:
  -h, --help       Print help
  -V, --version    Print version
";

fn main() -> ExitCode {
    match std::env::args().nth(1).as_deref() {
        Some("--version" | "-V") => {
            println!("vellora {}", env!("CARGO_PKG_VERSION"));
            ExitCode::SUCCESS
        }
        Some("--help" | "-h") => {
            print!("{USAGE}");
            ExitCode::SUCCESS
        }
        Some(other) => {
            eprintln!("vellora: unknown command '{other}'\n");
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
        None => {
            eprint!("{USAGE}");
            ExitCode::from(2)
        }
    }
}
