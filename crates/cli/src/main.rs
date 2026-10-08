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
//! **Status:** `vellora inspect` (M0 task 23); the other commands arrive with the
//! milestones that need them. The output formats are documented in `docs/user/cli.md`.

mod inspect;

use std::path::PathBuf;
use std::process::ExitCode;

use clap::{Parser, Subcommand};

/// Exit code for an operation that failed (unreadable file, not a PDF, wrong password).
const EXIT_FAILED: u8 = 1;

#[derive(Parser)]
#[command(name = "vellora", version, about = "Vellora PDF command-line tools")]
#[command(arg_required_else_help = true)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Show what a PDF file contains: version, pages, encryption, repairs and risky features.
    Inspect {
        /// The PDF file.
        file: PathBuf,
        /// Print the report as JSON (schema in docs/user/cli.md).
        #[arg(long)]
        json: bool,
        /// Password for an encrypted file (user or owner). Visible to other users of this
        /// machine in the process list.
        #[arg(long, value_name = "PASSWORD")]
        password: Option<String>,
    },
}

fn main() -> ExitCode {
    // clap exits with 0 for --help and --version and with 2 for a usage error.
    let cli = Cli::parse();
    match cli.command {
        Command::Inspect {
            file,
            json,
            password,
        } => match inspect::run(&file, json, password) {
            Ok(()) => ExitCode::SUCCESS,
            Err(message) => {
                eprintln!("vellora: {message}");
                ExitCode::from(EXIT_FAILED)
            }
        },
    }
}
