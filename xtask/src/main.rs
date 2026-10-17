//! `cargo xtask`: developer chores for the Vellora workspace.
//!
//! Commands:
//! - `corpus fetch [--manifest <file>] [--dir <dir>]`: download and verify the test corpus
//!   listed in `tests/corpus/manifest.toml` into `tests/corpus-data/` (git-ignored).
//!
//! - `goldens fetch [--manifest <file>] [--dir <dir>]` and `goldens update [--tag <tag>]`: the golden
//!   images of the rendering regression suite: download and verify them into `tests/golden-data/`
//!   (git-ignored), or render them again and write a new bundle and `tests/goldens/manifest.toml`.
//!
//! - `pdfium fetch [--lock <file>] [--dir <dir>] [--platform <key>]`: download the PDFium binaries
//!   pinned in `third_party/pdfium.lock`, verify their SHA-256 and extract them into
//!   `third_party/pdfium/<platform>/` (git-ignored).
//!
//! - `bench [generate|run] [args...]`: build the engine and the benchmark harness in release mode
//!   and run `vellora-bench` (default command `run`; see `bench/src/main.rs` for its flags).
//!   Documents are generated into `bench/data/` (git-ignored) on first use.
//!
//! Exit codes: 0 success, 1 a command failed, 2 usage error.
//!
//! This is dev tooling, not a library: it uses a boxed error instead of `thiserror`, and it
//! never parses PDF bytes (it only hashes them).

mod corpus;
mod goldens;
mod manifest;
mod pdfium;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage:
  cargo xtask corpus fetch [--manifest <file>] [--dir <dir>]
  cargo xtask goldens fetch [--manifest <file>] [--dir <dir>]
  cargo xtask goldens update [--tag <tag>]
  cargo xtask pdfium fetch [--lock <file>] [--dir <dir>] [--platform <key>]
  cargo xtask bench [generate|run] [--dir <dir>] [--only <name>] [--runs <n>]";

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["corpus", "fetch", rest @ ..] => run_corpus_fetch(rest),
        ["goldens", "fetch", rest @ ..] => run_goldens_fetch(rest),
        ["goldens", "update", rest @ ..] => run_goldens_update(rest),
        ["pdfium", "fetch", rest @ ..] => run_pdfium_fetch(rest),
        ["bench", rest @ ..] => run_bench(rest),
        _ => usage(),
    }
}

/// Builds the engine and the harness in release mode (they must sit side by side in
/// `target/release`) and runs the harness with the remaining arguments.
fn run_bench(rest: &[&str]) -> ExitCode {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let build = Command::new(&cargo)
        .args([
            "build",
            "--release",
            "--locked",
            "-p",
            "vellora-engine",
            "-p",
            "vellora-bench",
        ])
        .current_dir(workspace_root())
        .status();
    if !build.is_ok_and(|status| status.success()) {
        eprintln!("error: the release build failed");
        return ExitCode::from(1);
    }
    let (command, flags) = match rest {
        [first, tail @ ..] if !first.starts_with("--") => (*first, tail),
        _ => ("run", rest),
    };
    let exe = workspace_root()
        .join("target/release")
        .join(format!("vellora-bench{}", std::env::consts::EXE_SUFFIX));
    match Command::new(exe).arg(command).args(flags).status() {
        Ok(status) => ExitCode::from(u8::try_from(status.code().unwrap_or(1)).unwrap_or(1)),
        Err(e) => {
            eprintln!("error: cannot run the harness: {e}");
            ExitCode::from(1)
        }
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

fn run_goldens_fetch(mut rest: &[&str]) -> ExitCode {
    let mut manifest: PathBuf = workspace_root().join("tests/goldens/manifest.toml");
    let mut dir: PathBuf = workspace_root().join("tests/golden-data");
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
    match goldens::fetch(&manifest, &dir) {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::from(1)
        }
    }
}

fn run_goldens_update(rest: &[&str]) -> ExitCode {
    let tag = match rest {
        [] => None,
        ["--tag", tag] => Some(*tag),
        _ => return usage(),
    };
    match goldens::update(workspace_root(), tag) {
        Ok(()) => ExitCode::SUCCESS,
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
