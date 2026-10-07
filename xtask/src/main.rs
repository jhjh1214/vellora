//! `cargo xtask`: developer chores for the Vellora workspace.
//!
//! Commands:
//! - `corpus fetch [--manifest <file>] [--dir <dir>]`: download and verify the test corpus
//!   listed in `tests/corpus/manifest.toml` into `tests/corpus-data/` (git-ignored).
//!
//! - `pdfium fetch [--lock <file>] [--dir <dir>] [--platform <key>]`: download the PDFium binaries
//!   pinned in `third_party/pdfium.lock`, verify their SHA-256 and extract them into
//!   `third_party/pdfium/<platform>/` (git-ignored).
//!
//! Exit codes: 0 success, 1 a command failed, 2 usage error.
//!
//! This is dev tooling, not a library: it uses a boxed error instead of `thiserror`, and it
//! never parses PDF bytes (it only hashes them).

mod corpus;
mod manifest;
mod pdfium;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage:
  cargo xtask corpus fetch [--manifest <file>] [--dir <dir>]
  cargo xtask pdfium fetch [--lock <file>] [--dir <dir>] [--platform <key>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["corpus", "fetch", rest @ ..] => run_corpus_fetch(rest),
        ["pdfium", "fetch", rest @ ..] => run_pdfium_fetch(rest),
        _ => usage(),
    }
}

fn usage() -> ExitCode {
    eprintln!("{USAGE}");
    ExitCode::from(2)
}

fn workspace_root() -> &'static Path {
    // CARGO_MANIFEST_DIR is <root>/xtask.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| Path::new("."))
}

fn run_corpus_fetch(mut rest: &[&str]) -> ExitCode {
    let mut manifest: PathBuf = workspace_root().join("tests/corpus/manifest.toml");
    let mut dir: PathBuf = workspace_root().join("tests/corpus-data");
    while let [flag, value, tail @ ..] = rest {
        match *flag {
            "--manifest" => manifest = PathBuf::from(value),
            "--dir" => dir = PathBuf::from(value),
            _ => return usage(),
        }
        rest = tail;
    }
    if !rest.is_empty() {
        return usage();
    }
    match corpus::fetch(&manifest, &dir) {
        Ok(0) => ExitCode::SUCCESS,
        Ok(_) => ExitCode::from(1),
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_pdfium_fetch(mut rest: &[&str]) -> ExitCode {
    let mut lock: PathBuf = workspace_root().join("third_party/pdfium.lock");
    let mut dir: PathBuf = pdfium::default_root(workspace_root());
    let mut platform: Option<&str> = None;
    while let [flag, value, tail @ ..] = rest {
        match *flag {
            "--lock" => lock = PathBuf::from(value),
            "--dir" => dir = PathBuf::from(value),
            "--platform" => platform = Some(value),
            _ => return usage(),
        }
        rest = tail;
    }
    if !rest.is_empty() {
        return usage();
    }
    match pdfium::fetch(&lock, &dir, platform) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}
