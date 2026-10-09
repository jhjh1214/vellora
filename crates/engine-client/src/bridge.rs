//! The `cxx` bridge: the whole of what the Qt shell can say to the engine client (ADR-0003).
//!
//! The surface is deliberately small and plain: integers, floats, strings and shared structs, no
//! Qt types, no callbacks. The shell drains [`EngineEvent`]s with `poll_events` from a timer (see
//! the event model in [`crate::client`]); functions that can fail throw a C++ exception carrying
//! the error text.
//!
//! The generated header is `vellora-engine-client/src/bridge.rs.h` (the build script runs
//! `cxx-build` on this file), and the Rust side links as a static library next to the shell.
//!
//! # Tiles
//!
//! Tiles go through the client's cache (task 22a), so C++ never handles slots. A tile is
//! `tile_pixels()` square (`BGRx`, 4 bytes a pixel) at a *bucketed* scale: `bucket_scale(s)` is the
//! scale tiles are really rendered at, and `request_tile` / `read_tile` take the page, a scale
//! (bucketed internally) and the tile's column and row. The loop is: `request_tile`; on `Ready`
//! call `read_tile`; on `Requested` wait for `TileReady` (match `request`), then `read_tile`.
//! `out` of `read_tile` must be exactly `slot_bytes()` long, of which the first
//! `tile_pixels() * tile_pixels() * 4` bytes are the tile.

// The bridge macro expands to `unsafe` glue (extern "C" shims and `no_mangle` exports); the
// hand-written code below contains none.
#![allow(unsafe_code)]

use std::env;
use std::path::{Path, PathBuf};

use tracing::Level;
use vellora_ipc::{ErrorKind, PageSize, Priority, RequestId};

use crate::cache::{DEFAULT_BUDGET_BYTES, ScaleBucket, TileCache, TileKey};
use crate::client::{Client, ClientConfig, ClientError, Event, Stage, TILE_PIXELS, TileLookup};
use crate::crash::{self, CrashError, CrashGuard};
use crate::document::Change;
use crate::logging::{self, LogConfig, LogError, LogGuard};

/// Environment variable that overrides where [`open`] looks for `vellora-engine`. For development
/// and tests; an installed shell finds the engine next to its own executable.
pub const ENGINE_ENV: &str = "VELLORA_ENGINE";

// `cxx` turns enum variants into associated constants without carrying their doc comments over, so
// `missing_docs` flags variants that are described below. The comments on each type say which
// fields an event kind uses; they are the reference for the C++ side.
#[allow(missing_docs)]
#[cxx::bridge(namespace = "vellora")]
mod ffi {
    /// A page size in points; `0 x 0` when unknown.
    #[derive(Clone, Copy, Debug, PartialEq)]
    struct PageExtent {
        width: f32,
        height: f32,
    }

    /// What an [`EngineEvent`] is about; the fields it uses are listed per kind.
    #[derive(Debug)]
    enum EventKind {
        /// `page_count`, `repairs` (sent again after each restart).
        Opened,
        /// `request`, `slot`.
        TileReady,
        /// `failure`, `message`; `request` if `has_request`.
        RequestFailed,
        /// `lost`, `will_restart`, `message`.
        EngineCrashed,
        /// Nothing else; `Opened` follows.
        EngineRestarted,
        /// `message`. Every later call fails.
        Failed,
        /// `timeout`, `message`. The engine did not answer in time and was killed. After a
        /// `Hello` or `Open` timeout every later call fails; after a `Tile` timeout an
        /// `EngineCrashed` follows and the engine restarts if it can.
        EngineTimeout,
        /// `file_replaced`. The file on disk differs from the one opened; reported before the
        /// engine restarts over it.
        DocumentChanged,
    }

    /// What an engine failed to do in time (the client's `Stage`).
    #[derive(Debug)]
    enum TimeoutStage {
        None,
        Hello,
        Open,
        Tile,
    }

    /// Broad class of a failure (the protocol's `ErrorKind`).
    #[derive(Debug)]
    enum FailureKind {
        None,
        VersionMismatch,
        InvalidRequest,
        OpenFailed,
        RenderFailed,
        Internal,
        /// The document needs a password (answer to opening it).
        PasswordRequired,
        /// The password given does not open the document.
        WrongPassword,
    }

    /// How serious a line from the Qt side is.
    #[derive(Debug)]
    enum LogLevel {
        Debug,
        Info,
        Warning,
        Error,
    }

    /// How urgently a tile is wanted.
    #[derive(Debug)]
    enum TilePriority {
        Visible,
        Prefetch,
        Thumbnail,
    }

    /// What `request_tile` did.
    #[derive(Debug)]
    enum TileState {
        /// Cached: call `read_tile`. Nothing was sent.
        Ready,
        /// Already requested; its `TileReady` will come.
        InFlight,
        /// A new request was sent; `request` is its id.
        Requested,
        /// All cache capacity is held by requests in flight; ask again later.
        Full,
    }

    /// The answer to `request_tile`.
    #[derive(Debug)]
    struct TileTicket {
        state: TileState,
        /// The request id, when `state` is `Requested`.
        request: u64,
    }

    /// One reason a document counts as repaired (the protocol's `Repair`).
    #[derive(Clone, Debug, PartialEq)]
    struct RepairNote {
        /// Stable kebab-case identifier.
        code: String,
        /// One line for the user.
        message: String,
    }

    /// One thing that happened. A flat struct, so C++ needs no variant type.
    #[derive(Debug)]
    struct EngineEvent {
        kind: EventKind,
        /// The request an answer belongs to (`TileReady`, and `RequestFailed` if `has_request`).
        request: u64,
        has_request: bool,
        slot: u32,
        page_count: u32,
        /// `Opened`: empty unless the document needed repair.
        repairs: Vec<RepairNote>,
        will_restart: bool,
        failure: FailureKind,
        timeout: TimeoutStage,
        /// `DocumentChanged`: a different file is at the path (rather than the open file having
        /// been modified in place).
        file_replaced: bool,
        message: String,
        /// Requests lost in a crash; ask again after `EngineRestarted`.
        lost: Vec<u64>,
    }

    extern "Rust" {
        /// Crash reporting (task 9c, ADR-0019); `stop` detaches the handler and ends the monitor.
        type CrashHandle;

        /// The folder crash reports go to (`VELLORA_CRASH_DIR` overrides it).
        fn default_crash_directory() -> String;

        /// Starts the crash monitor (`monitor_exe` run with `--crash-monitor`) and attaches the
        /// crash handler to this process. `info` (version, commit, system) goes beside every dump.
        /// Throws if the monitor cannot be started: the application then runs without dumps.
        fn install_crash_handler(
            directory: &str,
            monitor_exe: &str,
            info: &str,
        ) -> Result<Box<CrashHandle>>;

        /// Detaches the handler and ends the monitor. Safe to call twice.
        fn stop(self: &mut CrashHandle);

        /// The monitor: serves the application until it goes away or crashes, writing the dump
        /// into the folder it was told. Returns the process exit code. Called instead of starting
        /// the application, when it was started with `--crash-monitor <socket> <folder>`. It reads
        /// its own command line, because a Windows `argv` loses the characters outside the code
        /// page (user names!).
        fn run_crash_monitor() -> i32;

        /// The running log (task 9b); `stop` writes what is queued and ends it.
        type LogHandle;

        /// The folder the log goes to unless told otherwise (`VELLORA_LOG_DIR` overrides it).
        fn default_log_directory() -> String;

        /// Starts the application log in `directory` (the default folder if empty): rotating
        /// files that also receive the engine's log lines. Throws if it is already started or
        /// the folder cannot be used. The level is `info`, or what `VELLORA_LOG` says.
        fn start_logging(directory: &str) -> Result<Box<LogHandle>>;

        /// Waits until everything logged so far is in the files.
        fn flush(self: &LogHandle);

        /// Writes what is queued and stops logging. Safe to call twice.
        fn stop(self: &mut LogHandle);

        /// A line from `qDebug`, `qWarning` and the like. Never call this with document content
        /// or a password.
        fn log_message(level: LogLevel, message: &str);

        /// An open document and the engine process rendering it.
        type EngineClient;

        /// Opens the document at `path` and starts its engine (found through `VELLORA_ENGINE`,
        /// else next to the running executable). `Opened` arrives as an event.
        fn open(path: &str) -> Result<Box<EngineClient>>;

        /// Opens an encrypted document after a `RequestFailed` with `PasswordRequired` or
        /// `WrongPassword`; `Opened` or the same failure follows. The text is not logged or
        /// kept beyond what a restarted engine needs. Throws if the document is open, the engine
        /// is down or the password cannot be sent (NUL, over 256 bytes).
        fn submit_password(self: &EngineClient, password: &str) -> Result<()>;

        /// Number of pages; 0 until `Opened`.
        fn page_count(self: &EngineClient) -> u32;

        /// Size of a page in points; `0 x 0` before `Opened` and beyond the first 4096 pages.
        fn page_size(self: &EngineClient, page: u32) -> PageExtent;

        /// Side of a tile in pixels.
        fn tile_pixels() -> u32;

        /// The scale tiles are rendered at when `scale` is asked for; 0 for an invalid scale.
        fn bucket_scale(scale: f32) -> f32;

        /// Bytes of the buffer `read_tile` fills (one slot of the tile region).
        fn slot_bytes(self: &EngineClient) -> u32;

        /// Returns the tile (column `x`, row `y`) of `page` at `scale` from the cache, or asks the
        /// engine for it. Throws when the engine is down, the handle is closed or the tile is
        /// invalid.
        fn request_tile(
            self: &EngineClient,
            page: u32,
            scale: f32,
            x: u32,
            y: u32,
            priority: TilePriority,
        ) -> Result<TileTicket>;

        /// Copies a ready tile into `out` (exactly `slot_bytes()` long). `false` if it is not
        /// ready.
        fn read_tile(
            self: &EngineClient,
            page: u32,
            scale: f32,
            x: u32,
            y: u32,
            out: &mut [u8],
        ) -> Result<bool>;

        /// Drops the cached tiles of a page (it changed).
        fn invalidate_page(self: &EngineClient, page: u32);

        /// The operating-system id of the running engine process; 0 while none is running (for
        /// diagnostics and tests that stop the engine from outside).
        fn engine_id(self: &EngineClient) -> u32;

        /// Withdraws a request; it is never reported. `false` if it was not in flight.
        fn cancel(self: &EngineClient, request: u64) -> bool;

        /// Takes every queued event, oldest first. Never blocks.
        fn poll_events(self: &EngineClient) -> Vec<EngineEvent>;

        /// Ends the engine. Safe to call twice; the client is unusable afterwards.
        fn close(self: &mut EngineClient);
    }
}

pub use ffi::{
    EngineEvent, EventKind, FailureKind, LogLevel, PageExtent, RepairNote, TilePriority, TileState,
    TileTicket, TimeoutStage,
};

/// Crash reporting behind the bridge's opaque handle. `None` after `stop`.
#[derive(Debug)]
pub struct CrashHandle {
    guard: Option<CrashGuard>,
}

/// The default crash folder as text (the bridge's `default_crash_directory`).
#[must_use]
pub fn default_crash_directory() -> String {
    crash::default_crash_dir().to_string_lossy().into_owned()
}

/// Starts crash reporting; see the bridge's `install_crash_handler`.
///
/// # Errors
///
/// [`CrashError`], which the bridge turns into an exception.
pub fn install_crash_handler(
    directory: &str,
    monitor_exe: &str,
    info: &str,
) -> Result<Box<CrashHandle>, CrashError> {
    let dir = if directory.is_empty() {
        crash::default_crash_dir()
    } else {
        PathBuf::from(directory)
    };
    Ok(Box::new(CrashHandle {
        guard: Some(crash::install(&dir, Path::new(monitor_exe), info)?),
    }))
}

impl CrashHandle {
    /// Detaches the handler and ends the monitor.
    pub fn stop(&mut self) {
        self.guard = None;
    }
}

/// Runs the crash monitor with the process's own command line; see the bridge's
/// `run_crash_monitor`. Exit code 2 if the command line is not `--crash-monitor <socket> <folder>`.
#[must_use]
pub fn run_crash_monitor() -> i32 {
    let mut args = env::args_os().skip(1);
    match (args.next(), args.next(), args.next(), args.next()) {
        (Some(flag), Some(socket), Some(dir), None) if flag == crash::MONITOR_FLAG => {
            crash::run_monitor(Path::new(&socket), Path::new(&dir))
        }
        _ => 2,
    }
}

/// The running log behind the bridge's opaque handle. `None` after `stop`.
#[derive(Debug)]
pub struct LogHandle {
    guard: Option<LogGuard>,
}

/// The default log folder as text (the bridge's `default_log_directory`).
#[must_use]
pub fn default_log_directory() -> String {
    logging::default_log_dir().to_string_lossy().into_owned()
}

/// Starts the application log; see the bridge's `start_logging`.
///
/// # Errors
///
/// [`LogError`], which the bridge turns into an exception.
pub fn start_logging(directory: &str) -> Result<Box<LogHandle>, LogError> {
    let dir = if directory.is_empty() {
        logging::default_log_dir()
    } else {
        PathBuf::from(directory)
    };
    let mut config = LogConfig::new(dir);
    if let Some(level) = env::var("VELLORA_LOG")
        .ok()
        .and_then(|text| text.parse::<Level>().ok())
    {
        config.level = level;
    }
    Ok(Box::new(LogHandle {
        guard: Some(logging::init(&config)?),
    }))
}

impl LogHandle {
    /// Waits until everything logged so far is in the files.
    pub fn flush(&self) {
        if let Some(guard) = &self.guard {
            guard.flush();
        }
    }

    /// Writes what is queued and stops logging.
    pub fn stop(&mut self) {
        self.guard = None;
    }
}

/// A line from the Qt side, as a log event (the bridge's `log_message`).
pub fn log_message(level: LogLevel, message: &str) {
    logging::log_qt(
        match level {
            LogLevel::Error => Level::ERROR,
            LogLevel::Warning => Level::WARN,
            LogLevel::Info => Level::INFO,
            _ => Level::DEBUG,
        },
        message,
    );
}

/// The client behind the bridge's opaque handle. `None` after `close`.
#[derive(Debug)]
pub struct EngineClient {
    client: Option<Client>,
}

/// Opens `path` with the engine found by [`engine_executable`].
///
/// # Errors
///
/// [`ClientError`], which the bridge turns into an exception.
pub fn open(path: &str) -> Result<Box<EngineClient>, ClientError> {
    let geometry = TileCache::geometry_for_budget(DEFAULT_BUDGET_BYTES)?;
    let mut config = ClientConfig::new(engine_executable(), geometry);
    // The application records engine incidents; library users and tests choose for themselves.
    config.crash_dir = Some(crash::default_crash_dir());
    open_with(config, Path::new(path))
}

/// Opens `path` with an explicit configuration (what [`open`] does after choosing one).
///
/// # Errors
///
/// [`ClientError`] if the document cannot be opened or the engine cannot be started.
pub fn open_with(config: ClientConfig, path: &Path) -> Result<Box<EngineClient>, ClientError> {
    let client = Client::open(config, path)?;
    Ok(Box::new(EngineClient {
        client: Some(client),
    }))
}

/// Where the engine executable is: `$VELLORA_ENGINE`, else `vellora-engine` in the directory of
/// the running executable (the layout of an installed shell; ADR-0004).
#[must_use]
pub fn engine_executable() -> PathBuf {
    if let Some(path) = env::var_os(ENGINE_ENV) {
        return PathBuf::from(path);
    }
    let name = format!("vellora-engine{}", env::consts::EXE_SUFFIX);
    env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

impl EngineClient {
    /// The client this handle wraps, for Rust callers (tests).
    #[must_use]
    pub fn client(&self) -> Option<&Client> {
        self.client.as_ref()
    }

    /// Number of pages; 0 until the document is open.
    pub fn page_count(&self) -> u32 {
        self.client
            .as_ref()
            .and_then(Client::page_count)
            .unwrap_or(0)
    }

    /// Sends the password for the document; see [`Client::submit_password`].
    ///
    /// # Errors
    ///
    /// [`ClientError`] if the handle is closed, the document is open, the engine is down or the
    /// password cannot be sent.
    pub fn submit_password(&self, password: &str) -> Result<(), ClientError> {
        self.client
            .as_ref()
            .ok_or(ClientError::Closed)?
            .submit_password(password)
    }

    /// Size of a page in points; zero by zero when unknown.
    pub fn page_size(&self, page: u32) -> PageExtent {
        self.client
            .as_ref()
            .and_then(|client| client.page_size(page))
            .map_or(
                PageExtent {
                    width: 0.0,
                    height: 0.0,
                },
                |PageSize { width, height }| PageExtent { width, height },
            )
    }

    /// Size of the buffer [`read_tile`](Self::read_tile) fills; 0 once closed.
    pub fn slot_bytes(&self) -> u32 {
        self.client
            .as_ref()
            .map_or(0, |client| client.geometry().slot_bytes())
    }

    /// Asks for a tile; see [`Client::request_cached_tile`].
    ///
    /// # Errors
    ///
    /// [`ClientError`] when the engine is down, the handle is closed or the tile is invalid.
    pub fn request_tile(
        &self,
        page: u32,
        scale: f32,
        x: u32,
        y: u32,
        priority: TilePriority,
    ) -> Result<TileTicket, ClientError> {
        let client = self.client.as_ref().ok_or(ClientError::Closed)?;
        let key = tile_key(page, scale, x, y)?;
        Ok(match client.request_cached_tile(key, priority.into())? {
            TileLookup::Ready => ticket(TileState::Ready, 0),
            TileLookup::InFlight => ticket(TileState::InFlight, 0),
            TileLookup::Requested(id) => ticket(TileState::Requested, id.0),
            // `TileLookup` is non-exhaustive; an unknown outcome is the safe "try again later".
            _ => ticket(TileState::Full, 0),
        })
    }

    /// Copies a ready tile into `out`; see [`Client::read_tile`].
    ///
    /// # Errors
    ///
    /// [`ClientError`] if the handle is closed, the tile is invalid or `out` has the wrong length.
    pub fn read_tile(
        &self,
        page: u32,
        scale: f32,
        x: u32,
        y: u32,
        out: &mut [u8],
    ) -> Result<bool, ClientError> {
        let client = self.client.as_ref().ok_or(ClientError::Closed)?;
        client.read_tile(&tile_key(page, scale, x, y)?, out)
    }

    /// The id of the running engine process, or 0.
    pub fn engine_id(&self) -> u32 {
        self.client
            .as_ref()
            .and_then(Client::engine_id)
            .unwrap_or(0)
    }

    /// Drops the cached tiles of `page`.
    pub fn invalidate_page(&self, page: u32) {
        if let Some(client) = &self.client {
            client.invalidate_page(page);
        }
    }

    /// Withdraws a request; false if it was not in flight.
    pub fn cancel(&self, request: u64) -> bool {
        self.client
            .as_ref()
            .is_some_and(|client| client.cancel(RequestId(request)))
    }

    /// Takes every queued event, oldest first.
    pub fn poll_events(&self) -> Vec<EngineEvent> {
        self.client
            .as_ref()
            .map(Client::poll_events)
            .unwrap_or_default()
            .into_iter()
            .map(EngineEvent::from)
            .collect()
    }

    /// Ends the engine. Safe to call twice.
    pub fn close(&mut self) {
        if let Some(client) = self.client.take() {
            client.close();
        }
    }
}

fn ticket(state: TileState, request: u64) -> TileTicket {
    TileTicket { state, request }
}

fn tile_key(page: u32, scale: f32, x: u32, y: u32) -> Result<TileKey, ClientError> {
    let scale = ScaleBucket::from_scale(scale).ok_or(ClientError::InvalidTile)?;
    Ok(TileKey { page, scale, x, y })
}

/// Side of a tile in pixels (the bridge's `tile_pixels`).
#[must_use]
pub fn tile_pixels() -> u32 {
    TILE_PIXELS
}

/// The scale tiles are rendered at when `scale` is asked for (the bridge's `bucket_scale`).
#[must_use]
pub fn bucket_scale(scale: f32) -> f32 {
    ScaleBucket::from_scale(scale).map_or(0.0, ScaleBucket::scale)
}

impl From<TilePriority> for Priority {
    fn from(priority: TilePriority) -> Self {
        match priority {
            TilePriority::Prefetch => Self::Prefetch,
            TilePriority::Thumbnail => Self::Thumbnail,
            // Includes values C++ could smuggle into the shared enum: the most urgent class is
            // the one a bogus value hurts least (it only reorders the caller's own tiles).
            _ => Self::Visible,
        }
    }
}

impl From<ErrorKind> for FailureKind {
    fn from(kind: ErrorKind) -> Self {
        match kind {
            ErrorKind::VersionMismatch => Self::VersionMismatch,
            ErrorKind::InvalidRequest => Self::InvalidRequest,
            ErrorKind::OpenFailed => Self::OpenFailed,
            ErrorKind::RenderFailed => Self::RenderFailed,
            ErrorKind::Internal => Self::Internal,
            ErrorKind::PasswordRequired => Self::PasswordRequired,
            ErrorKind::WrongPassword => Self::WrongPassword,
        }
    }
}

impl EngineEvent {
    fn empty(kind: EventKind) -> Self {
        Self {
            kind,
            request: 0,
            has_request: false,
            slot: 0,
            page_count: 0,
            repairs: Vec::new(),
            will_restart: false,
            failure: FailureKind::None,
            timeout: TimeoutStage::None,
            file_replaced: false,
            message: String::new(),
            lost: Vec::new(),
        }
    }
}

impl From<Event> for EngineEvent {
    fn from(event: Event) -> Self {
        match event {
            Event::Opened {
                page_count,
                repairs,
                ..
            } => Self {
                page_count,
                repairs: repairs
                    .into_iter()
                    .map(|r| RepairNote {
                        code: r.code,
                        message: r.message,
                    })
                    .collect(),
                ..Self::empty(EventKind::Opened)
            },
            Event::TileReady { request, slot } => Self {
                request: request.0,
                has_request: true,
                slot: slot.0,
                ..Self::empty(EventKind::TileReady)
            },
            Event::RequestFailed {
                request,
                kind,
                message,
            } => Self {
                request: request.map_or(0, |id| id.0),
                has_request: request.is_some(),
                failure: kind.into(),
                message,
                ..Self::empty(EventKind::RequestFailed)
            },
            Event::EngineCrashed {
                crash,
                lost,
                will_restart,
            } => Self {
                will_restart,
                message: crash.to_string(),
                lost: lost.into_iter().map(|id| id.0).collect(),
                ..Self::empty(EventKind::EngineCrashed)
            },
            Event::EngineRestarted => Self::empty(EventKind::EngineRestarted),
            Event::Failed { reason } => Self {
                message: reason,
                ..Self::empty(EventKind::Failed)
            },
            Event::EngineTimeout { stage, waited } => Self {
                timeout: match stage {
                    Stage::Hello => TimeoutStage::Hello,
                    Stage::Open => TimeoutStage::Open,
                    // `Stage` is non-exhaustive; the stalled-tile case is the one that restarts.
                    _ => TimeoutStage::Tile,
                },
                message: format!("no answer for {} s", waited.as_secs()),
                ..Self::empty(EventKind::EngineTimeout)
            },
            Event::DocumentChanged { change } => Self {
                file_replaced: change == Change::Replaced,
                ..Self::empty(EventKind::DocumentChanged)
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use vellora_ipc::{Repair, RequestId, SlotId};

    use super::*;
    use crate::process::{Crash, Termination};

    #[test]
    fn the_header_is_generated_in_the_build() {
        // `build.rs` points at the header `cxx-build` wrote for this file.
        let header = fs::read_to_string(env!("VELLORA_BRIDGE_HEADER"))
            .expect("the build generates the C++ header");
        for expected in [
            "namespace vellora",
            "struct EngineEvent",
            "struct RepairNote",
            "enum class EventKind",
            "struct EngineClient final : public ::rust::Opaque",
            "::rust::Box<::vellora::EngineClient> open(::rust::Str path)",
            "request_tile(",
            "read_tile(",
            "submit_password(::rust::Str password)",
            "engine_id()",
            "struct TileTicket",
            "enum class TileState",
            "poll_events()",
            "void close()",
        ] {
            assert!(header.contains(expected), "header lacks `{expected}`");
        }
    }

    #[test]
    fn events_convert_field_by_field() {
        let tile = EngineEvent::from(Event::TileReady {
            request: RequestId(7),
            slot: SlotId(3),
        });
        assert_eq!(
            (tile.kind, tile.request, tile.has_request, tile.slot),
            (EventKind::TileReady, 7, true, 3)
        );

        let failed = EngineEvent::from(Event::RequestFailed {
            request: None,
            kind: ErrorKind::OpenFailed,
            message: "no pages".into(),
        });
        assert_eq!(
            (failed.kind, failed.has_request, failed.failure),
            (EventKind::RequestFailed, false, FailureKind::OpenFailed)
        );
        assert_eq!(failed.message, "no pages");

        for (kind, expected) in [
            (ErrorKind::PasswordRequired, FailureKind::PasswordRequired),
            (ErrorKind::WrongPassword, FailureKind::WrongPassword),
        ] {
            let refused = EngineEvent::from(Event::RequestFailed {
                request: None,
                kind,
                message: "password required".into(),
            });
            assert_eq!((refused.has_request, refused.failure), (false, expected));
        }

        let crashed = EngineEvent::from(Event::EngineCrashed {
            crash: Crash {
                termination: Termination::Failure(3),
                killed_by_client: false,
            },
            lost: vec![RequestId(4), RequestId(9)],
            will_restart: true,
        });
        assert_eq!(crashed.kind, EventKind::EngineCrashed);
        assert!(crashed.will_restart);
        assert_eq!(crashed.lost, [4, 9]);
        assert!(
            crashed.message.contains("Failure(3)"),
            "{}",
            crashed.message
        );

        let opened = EngineEvent::from(Event::Opened {
            page_count: 10_000,
            page_sizes: Vec::new(),
            repairs: vec![Repair {
                code: "xref-unreadable".into(),
                message: "cross-reference unreadable".into(),
            }],
        });
        assert_eq!(opened.page_count, 10_000);
        assert_eq!(
            opened.repairs,
            [RepairNote {
                code: "xref-unreadable".into(),
                message: "cross-reference unreadable".into(),
            }]
        );
    }

    #[test]
    fn a_closed_handle_answers_without_failing() {
        let mut handle = EngineClient { client: None };
        assert_eq!(handle.page_count(), 0);
        assert_eq!(
            handle.page_size(0),
            PageExtent {
                width: 0.0,
                height: 0.0
            }
        );
        assert!(handle.poll_events().is_empty());
        assert!(!handle.cancel(1));
        assert!(matches!(
            handle.request_tile(0, 1.0, 0, 0, TilePriority::Visible),
            Err(ClientError::Closed)
        ));
        assert!(matches!(
            handle.read_tile(0, 1.0, 0, 0, &mut []),
            Err(ClientError::Closed)
        ));
        assert!(matches!(
            handle.submit_password("anything"),
            Err(ClientError::Closed)
        ));
        assert_eq!(handle.slot_bytes(), 0);
        assert_eq!(handle.engine_id(), 0);
        handle.invalidate_page(0);
        handle.close();
        handle.close();
    }

    #[test]
    fn the_engine_is_found_next_to_the_executable_by_default() {
        // Only the shape is checked: the environment override is not set from a test.
        let path = engine_executable();
        assert!(
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("vellora-engine"))
                || env::var_os(ENGINE_ENV).is_some(),
            "{}",
            path.display()
        );
    }
}
