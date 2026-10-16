//! Starting the engine for the commands that need to render or read text. The engine is the
//! sandboxed `vellora-engine` process, the same one the desktop app uses; the CLI is its client and
//! never reads the document itself.
//!
//! Where things are: the engine is looked for next to the `vellora` executable (override with
//! `VELLORA_ENGINE`), and PDFium next to the engine (override with `VELLORA_PDFIUM_LIB`, which is
//! handed on to the engine).

use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vellora_engine_client::{Client, ClientConfig, Event};
use vellora_ipc::ErrorKind;
use vellora_shm::SlotGeometry;

/// Names the engine executable.
const ENGINE_ENV: &str = "VELLORA_ENGINE";
/// Where PDFium is, for the engine (the name `vellora-render` reads).
const PDFIUM_ENV: &str = "VELLORA_PDFIUM_LIB";

/// How long opening a document may take.
const OPEN_PATIENCE: Duration = Duration::from_secs(120);
/// How long one answer (a tile, a page of text) may take.
pub(crate) const ANSWER_PATIENCE: Duration = Duration::from_secs(120);

/// One slot holds a 512 x 512 tile of four bytes a pixel.
pub(crate) const SLOT_BYTES: u32 = 1 << 20;

fn engine_path() -> Result<PathBuf, String> {
    if let Some(path) = env::var_os(ENGINE_ENV) {
        return Ok(PathBuf::from(path));
    }
    let exe = env::current_exe().map_err(|error| format!("cannot find myself: {error}"))?;
    let name = if cfg!(windows) {
        "vellora-engine.exe"
    } else {
        "vellora-engine"
    };
    let beside = exe.with_file_name(name);
    if beside.is_file() {
        Ok(beside)
    } else {
        Err(format!(
            "the engine ({name}) is not next to {}; set {ENGINE_ENV} to its path",
            exe.display()
        ))
    }
}

/// Starts an engine over `file` and waits until the document is open. `password` is sent if the
/// document asks for one (once; a refused password ends the attempt).
pub(crate) fn open(file: &Path, password: Option<&str>) -> Result<Client, String> {
    let mut config = ClientConfig::new(
        engine_path()?,
        SlotGeometry::new(1, SLOT_BYTES).map_err(|error| error.to_string())?,
    );
    // One document, one run: no thumbnails or print bands, and a crash is a failure, not a retry.
    config.thumbnail_budget_bytes = 0;
    config.print_slots = 0;
    config.max_restarts = 0;
    if let Some(path) = env::var_os(PDFIUM_ENV) {
        config.env.push((OsString::from(PDFIUM_ENV), path));
    }
    let client = Client::open(config, file).map_err(|error| error.to_string())?;

    let mut tried = false;
    let deadline = Instant::now() + OPEN_PATIENCE;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("the engine did not open the document in time")?;
        for event in client.wait_events(left.min(Duration::from_secs(1))) {
            match event {
                Event::Opened { .. } => return Ok(client),
                Event::RequestFailed {
                    request: None,
                    kind: ErrorKind::PasswordRequired | ErrorKind::WrongPassword,
                    ..
                } => {
                    let Some(password) = password.filter(|_| !tried) else {
                        return Err(if tried {
                            "the password is not correct".to_owned()
                        } else {
                            "the document is encrypted: pass --password".to_owned()
                        });
                    };
                    tried = true;
                    client
                        .submit_password(password)
                        .map_err(|error| error.to_string())?;
                }
                Event::RequestFailed {
                    request: None,
                    message,
                    ..
                } => return Err(message),
                other => {
                    if let Some(reason) = fatal(&other) {
                        return Err(reason);
                    }
                }
            }
        }
    }
}

/// Why `event` means the engine is gone or stuck, if it does.
pub(crate) fn fatal(event: &Event) -> Option<String> {
    match event {
        Event::Failed { reason } => Some(reason.clone()),
        Event::EngineCrashed { crash, .. } => Some(format!("the engine stopped ({crash:?})")),
        Event::EngineTimeout { stage, waited } => Some(format!(
            "the engine did not answer ({stage:?}, after {} s)",
            waited.as_secs()
        )),
        _ => None,
    }
}

/// Waits for an event for which `answer` returns `Some`, up to [`ANSWER_PATIENCE`].
pub(crate) fn wait<T>(
    client: &Client,
    mut answer: impl FnMut(&Event) -> Option<Result<T, String>>,
) -> Result<T, String> {
    let deadline = Instant::now() + ANSWER_PATIENCE;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("the engine did not answer in time")?;
        for event in client.wait_events(left.min(Duration::from_secs(1))) {
            if let Some(result) = answer(&event) {
                return result;
            }
            if let Some(reason) = fatal(&event) {
                return Err(reason);
            }
        }
    }
}
