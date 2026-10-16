//! `vellora-bench`: synthetic large documents and the M0 performance harness.
//!
//! Normally run through `cargo xtask bench`, which builds this and the engine in release mode.
//!
//! ```text
//! vellora-bench generate [--dir <dir>] [--only <name>]
//! vellora-bench run [--dir <dir>] [--only <name>] [--engine <exe>] [--pdfium <lib>] [--runs <n>]
//!                   [--json <file>]
//! vellora-bench compare <previous.json> <current.json> [--max-regression <percent>]
//! ```
//!
//! `run` generates a missing document first. Documents live in `bench/data/` (git-ignored).
//! `run --json` also writes every number to a file; `compare` is the performance gate: it exits 1
//! when a tracked metric of the second file is worse than the first by more than the limit
//! (default 10 %). Exit codes: 0 success, 1 a measurement failed or the gate failed, 2 usage error.
//!
//! This is dev tooling: a boxed error instead of `thiserror`, and it reads only files it wrote.

mod generate;
mod measure;
mod report;
mod scenarios;

use std::error::Error;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

use generate::Kind;

pub(crate) type Result<T> = std::result::Result<T, Box<dyn Error>>;

const USAGE: &str = "usage:
  vellora-bench generate [--dir <dir>] [--only <name>]
  vellora-bench run [--dir <dir>] [--only <name>] [--engine <exe>] [--pdfium <lib>] [--runs <n>] [--json <file>]
  vellora-bench compare <previous.json> <current.json> [--max-regression <percent>]
names: text-10k, images-500mb";

#[derive(Default)]
struct Options {
    dir: Option<PathBuf>,
    only: Option<Kind>,
    engine: Option<PathBuf>,
    pdfium: Option<PathBuf>,
    runs: Option<usize>,
    json: Option<PathBuf>,
}

fn parse(mut rest: &[String]) -> Option<Options> {
    let mut options = Options::default();
    while let [flag, value, tail @ ..] = rest {
        match flag.as_str() {
            "--dir" => options.dir = Some(PathBuf::from(value)),
            "--only" => options.only = Some(Kind::parse(value)?),
            "--engine" => options.engine = Some(PathBuf::from(value)),
            "--pdfium" => options.pdfium = Some(PathBuf::from(value)),
            "--json" => options.json = Some(PathBuf::from(value)),
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
    if let Some(json) = &options.json {
        let mut report = report::Report::new(runs);
        measure::record(&results, &mut report);
        report.write(json)?;
        eprintln!("wrote {}", json.display());
    }
    Ok(())
}

/// The gate: prints every tracked metric's change and fails if one is too much worse.
fn compare_command(args: &[String]) -> ExitCode {
    let (paths, limit) = match args {
        [previous, current] => ([previous, current], 10.0),
        [previous, current, flag, value] if flag == "--max-regression" => {
            match value.parse::<f64>() {
                Ok(percent) if percent > 0.0 => ([previous, current], percent),
                _ => {
                    eprintln!("{USAGE}");
                    return ExitCode::from(2);
                }
            }
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::from(2);
        }
    };
    let read = |path: &String| report::Report::read(Path::new(path));
    let (previous, current) = match (read(paths[0]), read(paths[1])) {
        (Ok(previous), Ok(current)) => (previous, current),
        (Err(error), _) | (_, Err(error)) => {
            eprintln!("error: {error}");
            return ExitCode::from(1);
        }
    };
    if previous.os != current.os || previous.arch != current.arch {
        eprintln!(
            "error: the files are from different machines ({}/{} and {}/{})",
            previous.os, previous.arch, current.os, current.arch
        );
        return ExitCode::from(1);
    }
    let changes = report::compare(&previous, &current, limit / 100.0);
    println!(
        "{:<52} {:>12} {:>12} {:>8}",
        "metric", "previous", "current", "worse"
    );
    for c in &changes {
        println!(
            "{:<52} {:>12.1} {:>12.1} {:>+7.1}% {}{}",
            c.name,
            c.previous,
            c.current,
            c.worse_by * 100.0,
            c.unit,
            if c.failed { "  FAIL" } else { "" }
        );
    }
    let failed = changes.iter().filter(|c| c.failed).count();
    if failed > 0 {
        eprintln!("error: {failed} metric(s) are worse by more than {limit} %");
        return ExitCode::from(1);
    }
    println!(
        "ok: {} tracked metrics, none worse by more than {limit} %",
        changes.len()
    );
    ExitCode::SUCCESS
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some((command, rest)) = args.split_first() else {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    };
    if command == "compare" {
        return compare_command(rest);
    }
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
