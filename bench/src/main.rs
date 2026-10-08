//! `vellora-bench`: synthetic large documents and the M0 performance harness.
//!
//! Normally run through `cargo xtask bench`, which builds this and the engine in release mode.
//!
//! ```text
//! vellora-bench generate [--dir <dir>] [--only <name>]
//! vellora-bench run [--dir <dir>] [--only <name>] [--engine <exe>] [--pdfium <lib>] [--runs <n>]
//! ```
//!
//! `run` generates a missing document first. Documents live in `bench/data/` (git-ignored).
//! Exit codes: 0 success, 1 a measurement failed, 2 usage error.
//!
//! This is dev tooling: a boxed error instead of `thiserror`, and it reads only files it wrote.

mod generate;
mod measure;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use generate::Kind;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage:
  vellora-bench generate [--dir <dir>] [--only <name>]
  vellora-bench run [--dir <dir>] [--only <name>] [--engine <exe>] [--pdfium <lib>] [--runs <n>]
names: text-10k, images-500mb";

#[derive(Default)]
struct Options {
    dir: Option<PathBuf>,
    only: Option<Kind>,
    engine: Option<PathBuf>,
    pdfium: Option<PathBuf>,
    runs: Option<usize>,
}

fn parse(mut rest: &[String]) -> Option<Options> {
    let mut options = Options::default();
    while let [flag, value, tail @ ..] = rest {
        match flag.as_str() {
            "--dir" => options.dir = Some(PathBuf::from(value)),
            "--only" => options.only = Some(Kind::parse(value)?),
            "--engine" => options.engine = Some(PathBuf::from(value)),
            "--pdfium" => options.pdfium = Some(PathBuf::from(value)),
            "--runs" => options.runs = Some(value.parse().ok().filter(|&n| n > 0)?),
            _ => return None,
        }
        rest = tail;
    }
    rest.is_empty().then_some(options)
}

fn data_dir(options: &Options) -> PathBuf {
    options
        .dir
        .clone()
        .unwrap_or_else(|| Path::new(env!("CARGO_MANIFEST_DIR")).join("data"))
}

fn kinds(options: &Options) -> Vec<Kind> {
    options
        .only
        .map_or_else(|| Kind::ALL.to_vec(), |kind| vec![kind])
}

/// Makes sure the document exists, generating it if not.
fn ensure(dir: &Path, kind: Kind) -> Result<PathBuf> {
    std::fs::create_dir_all(dir)?;
    let path = dir.join(kind.file_name());
    if path.is_file() {
        return Ok(path);
    }
    eprintln!("generating {} ...", path.display());
    let started = std::time::Instant::now();
    let size = generate::generate(kind, &path)?;
    eprintln!("  {size} bytes in {:.1} s", started.elapsed().as_secs_f64());
    Ok(path)
}

fn run_command(command: &str, options: &Options) -> Result<()> {
    let dir = data_dir(options);
    if command == "generate" {
        for kind in kinds(options) {
            let path = dir.join(kind.file_name());
            if path.exists() {
                std::fs::remove_file(&path)?;
            }
            ensure(&dir, kind)?;
        }
        return Ok(());
    }
    let setup = measure::Setup::locate(options.engine.clone(), options.pdfium.clone())?;
    let runs = options.runs.unwrap_or(3);
    let mut results = Vec::new();
    for kind in kinds(options) {
        let path = ensure(&dir, kind)?;
        results.push(measure::measure(&setup, kind, &path, runs)?);
    }
    measure::print_table(&results, runs);
    Ok(())
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    let Some(options) = parse(rest).filter(|_| matches!(command.as_str(), "generate" | "run"))
    else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    match run_command(command, &options) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("error: {error}");
            ExitCode::from(1)
        }
    }
}
