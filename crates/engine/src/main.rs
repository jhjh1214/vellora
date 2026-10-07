//! `vellora-engine` executable: parses the launch arguments, adopts the inherited handles,
//! starts PDFium and serves the UI on standard input and output. See the library crate for the
//! design.

use std::io::{self, BufWriter};
use std::process::ExitCode;

use anyhow::Context as _;
use vellora_engine::{Engine, Invocation, LaunchArgs, args};
use vellora_ipc::{ErrorKind, PROTOCOL_VERSION, Response, write_frame};
use vellora_render::{Pdfium, Renderer};
use vellora_shm::{TileRegion, adopt};

/// Environment variable that sets the log level (`error`, `warn`, `info`, `debug`, `trace`).
const LOG_ENV: &str = "VELLORA_LOG";

fn main() -> ExitCode {
    let Ok(arguments) = std::env::args_os()
        .skip(1)
        .map(std::ffi::OsString::into_string)
        .collect::<Result<Vec<_>, _>>()
    else {
        eprintln!("vellora-engine: arguments must be valid UTF-8");
        return ExitCode::from(2);
    };

    let launch = match args::parse(arguments) {
        Ok(Invocation::Version) => {
            println!("vellora-engine {}", env!("CARGO_PKG_VERSION"));
            return ExitCode::SUCCESS;
        }
        Ok(Invocation::Serve(launch)) => launch,
        Err(error) => {
            eprintln!("vellora-engine: {error}");
            return ExitCode::from(2);
        }
    };

    init_logging();

    let mut engine = match start(&launch) {
        Ok(engine) => engine,
        Err(error) => {
            // The UI is already waiting on our standard output: tell it why there will be no
            // session, in the protocol it understands, then fail.
            let message = format!("the engine could not start: {error:#}");
            tracing::error!("{message}");
            report_startup_failure(&message);
            return ExitCode::FAILURE;
        }
    };

    let stdin = io::stdin().lock();
    // Not `.lock()`ed: the writer is shared with the render worker thread, which needs `Send`.
    let stdout = BufWriter::new(io::stdout());
    match engine.serve(stdin, stdout) {
        Ok(exit) => {
            tracing::info!(?exit, "session ended");
            ExitCode::SUCCESS
        }
        Err(error) => {
            tracing::error!(%error, "session failed");
            ExitCode::FAILURE
        }
    }
}

/// Adopts the inherited files and starts the renderer.
fn start(launch: &LaunchArgs) -> anyhow::Result<Engine> {
    let file = adopt(launch.file).context("cannot use the document handle")?;
    let region_file = adopt(launch.region).context("cannot use the tile region handle")?;
    let region = TileRegion::from_file(&region_file, launch.geometry)
        .context("cannot map the tile region")?;
    let library = Pdfium::locate().context("cannot find PDFium")?;
    let renderer = Renderer::start(library).context("cannot start PDFium")?;
    let mut engine = Engine::new(
        renderer,
        file,
        launch.file,
        region,
        launch.max_document_bytes,
    );
    engine.set_deadlines(launch.deadlines);
    // A PDFium call cannot be interrupted, so a tile past its hard deadline ends the process;
    // the UI sees the pipe close and restarts the engine (ADR-0004). `abort`, not `exit`: the
    // stuck thread may hold locks that exit-time cleanup would wait for.
    engine.set_hard_deadline_hook(|req_id| {
        tracing::error!(
            req_id = req_id.0,
            "a tile exceeded its hard deadline: aborting the engine"
        );
        std::process::abort();
    });
    Ok(engine)
}

/// Best effort: if the pipe is already gone there is nobody to tell.
fn report_startup_failure(message: &str) {
    let mut out = BufWriter::new(io::stdout().lock());
    let hello = Response::Hello {
        protocol_version: PROTOCOL_VERSION,
    };
    let error = Response::Error {
        req_id: None,
        kind: ErrorKind::Internal,
        message: message.chars().take(1024).collect(),
    };
    let _ = write_frame(&mut out, &hello).and_then(|()| write_frame(&mut out, &error));
}

/// Logs go to standard error only; standard output belongs to the protocol.
fn init_logging() {
    let level = std::env::var(LOG_ENV)
        .ok()
        .and_then(|text| text.parse::<tracing::Level>().ok())
        .unwrap_or(tracing::Level::WARN);
    // A second initialisation (impossible here) would only mean logging is already set up.
    let _ = tracing_subscriber::fmt()
        .with_max_level(level)
        .with_writer(io::stderr)
        .try_init();
}
