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
//! **Status:** `inspect` (M0 task 23), `security`, `render` and `text` (M1 task 19); the other
//! commands arrive with the milestones that need them. The output formats are documented in
//! `docs/user/cli.md`.

mod engine;
mod inspect;
mod pagespec;
mod render;
mod security;
mod text;

use std::io::Write as _;
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
    /// Say whether a PDF file has features that make it risky to open (scripts, programs it can
    /// start, embedded files, ...). Exit code 0 if none, 3 if some.
    Security {
        /// The PDF file.
        file: PathBuf,
        /// Print the assessment as JSON (schema in docs/user/cli.md).
        #[arg(long)]
        json: bool,
        /// Password for an encrypted file (user or owner).
        #[arg(long, value_name = "PASSWORD")]
        password: Option<String>,
    },
    /// Draw pages as PNG files with the engine.
    Render {
        /// The PDF file.
        file: PathBuf,
        /// Pages to draw, from 1: for example 1-5,8,10- (default: the first page).
        #[arg(long, value_name = "PAGES")]
        pages: Option<String>,
        /// Resolution in dots per inch.
        #[arg(long, default_value_t = 150, value_name = "N")]
        dpi: u32,
        /// A folder (files are named page-N.png) or, for one page, a .png file.
        #[arg(short, long, value_name = "PATH")]
        output: PathBuf,
        /// Password for an encrypted file (user or owner).
        #[arg(long, value_name = "PASSWORD")]
        password: Option<String>,
    },
    /// Print the text of pages in reading order, read by the engine. Pages are separated by a
    /// form feed.
    Text {
        /// The PDF file.
        file: PathBuf,
        /// Pages to read, from 1: for example 1-5,8,10- (default: all).
        #[arg(long, value_name = "PAGES")]
        pages: Option<String>,
        /// Password for an encrypted file (user or owner).
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
            Err(message) => failed(&message),
        },
        Command::Security {
            file,
            json,
            password,
        } => match security::assess(&file, password) {
            Ok(verdict) => {
                if json {
                    match serde_json::to_string_pretty(&verdict.json) {
                        Ok(text) => println!("{text}"),
                        Err(error) => return failed(&error.to_string()),
                    }
                } else {
                    print!("{}", verdict.text);
                }
                if verdict.risky {
                    ExitCode::from(security::EXIT_RISKY)
                } else if verdict.incomplete {
                    // Nothing was found, but not everything could be read: a security check does
                    // not answer "fine" to a file it could not read in full.
                    failed(
                        "the scan was incomplete, so the absence of risky features is not certain",
                    )
                } else {
                    ExitCode::SUCCESS
                }
            }
            Err(message) => failed(&message),
        },
        Command::Render {
            file,
            pages,
            dpi,
            output,
            password,
        } => match render::run(&file, pages.as_deref(), dpi, &output, password.as_deref()) {
            Ok(files) => {
                for path in files {
                    println!("{}", path.display());
                }
                ExitCode::SUCCESS
            }
            Err(message) => failed(&message),
        },
        Command::Text {
            file,
            pages,
            password,
        } => {
            let mut stdout = std::io::stdout().lock();
            match text::run(&file, pages.as_deref(), password.as_deref(), &mut stdout) {
                Ok(()) => ExitCode::SUCCESS,
                Err(message) => {
                    let _ = stdout.flush();
                    failed(&message)
                }
            }
        }
    }
}

fn failed(message: &str) -> ExitCode {
    eprintln!("vellora: {message}");
    ExitCode::from(EXIT_FAILED)
}
