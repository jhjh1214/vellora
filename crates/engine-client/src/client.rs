//! The protocol client: one document, one engine process at a time, restarted after a crash.
//!
//! # Event model: a polled queue
//!
//! The Qt event loop owns the UI thread, so nothing here calls back into the caller. Requests
//! return at once; everything the engine says (and everything that happens to the engine) is
//! queued as an [`Event`] that the UI drains with [`Client::poll_events`] from a timer or after a
//! wake-up of its own. Rust code that has nothing else to do can block in
//! [`Client::wait_events`]. A queue needs no thread-safe callback, no Qt type on this side and no
//! re-entrancy rules, which is what makes the `cxx` bridge small (ADR-0003).
//!
//! # Threads
//!
//! One reader thread per engine process decodes its responses and queues events. When a reader
//! sees the pipe close without a [`Client::close`], it reaps the process ([`Crash`]), reports
//! [`Event::EngineCrashed`] and, within the restart budget, starts a new engine over the same
//! document and region and opens it again ([`Event::EngineRestarted`], then a second
//! [`Event::Opened`]). Requests are written from the caller's thread under a lock of their own, so
//! a slow pipe write never stops the reader from draining the engine's answers.
//!
//! # What a crash costs the caller
//!
//! Requests in flight when the engine dies are gone; [`Event::EngineCrashed::lost`] lists them so
//! the UI can ask again once it sees [`Event::EngineRestarted`]. A request made while the engine
//! is down fails with [`ClientError::EngineUnavailable`]. Tile slots may hold garbage after a
//! crash: a slot is valid only between a request's [`Event::TileReady`] and its reuse.
//!
//! # The tile cache
//!
//! The client owns a [`TileCache`] over the shared region. [`Client::request_cached_tile`] answers
//! a cache hit at once ([`TileLookup::Ready`]) and otherwise reserves a slot and asks the engine;
//! the reader thread completes the entry when `TileReady` arrives and drops it when the request is
//! cancelled, fails or is lost in a crash. [`Client::read_tile`] copies a ready tile. The cache and
//! the slot hand-out are one critical section with the request, so a slot is never reused while
//! the engine may still write it for a live request. Do not mix this with [`Client::request_tile`]
//! on the same client: that call lets the caller pick slots the cache knows nothing about.
//!
//! Thumbnails ([`Client::request_thumbnail`]) have a second cache with its own budget and its own
//! slots at the end of the region, so a fling through the sidebar cannot evict the tiles of the
//! page being read, and the engine renders them last ([`Priority::Thumbnail`]).
//!
//! # The client's own deadlines
//!
//! The engine aborts itself when a tile passes its hard deadline, but a wedged process may never
//! get there, and nothing inside the engine covers start-up. A watchdog thread therefore
//! enforces three deadlines from the outside (all reported as [`Event::EngineTimeout`]):
//!
//! - [`Stage::Hello`]: the engine must say `Hello` within [`ClientConfig::hello_timeout`] of being
//!   started.
//! - [`Stage::Open`]: it must answer `Open` within [`ClientConfig::open_timeout`] of its `Hello`.
//!   Both are final: the engine is killed and the client [`Event::Failed`]s without a restart,
//!   because a start-up that hangs once will hang again on the same document.
//! - [`Stage::Tile`]: while requests are in flight, *some* answer must arrive within twice the
//!   engine's hard tile deadline. Otherwise the engine is killed, which the client handles like
//!   any crash (the requests are listed as lost and the engine restarts within its budget). The
//!   clock runs from the last answer, not from each request, because a request can wait in the
//!   engine's queue behind others for longer than any one tile may take.
//!
//! # Changes to the file
//!
//! The document is opened so that other processes cannot write to it where the OS allows
//! (see [`crate::document`]). At each engine restart the client also compares the file and its
//! path with what it last saw and reports [`Event::DocumentChanged`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use vellora_engine::{DEFAULT_MAX_DOCUMENT_BYTES, Deadlines};
use vellora_ipc::{
    ErrorKind, PROTOCOL_VERSION, PageSize, Password, Priority, Repair, Request, RequestId,
    Response, SlotId, TileRect, check_version, read_frame, write_frame,
};
use vellora_shm::{SlotGeometry, TileRegion};

use crate::cache::{
    DEFAULT_BUDGET_BYTES, DEFAULT_THUMBNAIL_BUDGET_BYTES, ReserveError, ScaleBucket, TileCache,
    TileKey,
};
use crate::document::{self, Change, Seen};
use crate::limits::ResourceLimits;
use crate::logging::LineRing;
use crate::process::{Crash, EngineProcess, SpawnConfig, SpawnError};

/// Engines restarted after crashes in a row, with no tile delivered in between, before the client
/// gives up with [`Event::EngineCrashed`] `will_restart: false`.
pub const DEFAULT_MAX_RESTARTS: u32 = 3;

/// Side of a cached tile in device pixels. A tile is `TILE_PIXELS` square at 4 bytes a pixel, which
/// is exactly one [`crate::cache::SLOT_BYTES`] slot. Tiles on the right and bottom edges of a page
/// extend past it (white there), so every tile costs the same.
pub const TILE_PIXELS: u32 = 512;

/// Bytes of one cached tile.
const TILE_BYTES: u64 = TILE_PIXELS as u64 * TILE_PIXELS as u64 * 4;

/// Default time from starting an engine to its `Hello`.
pub const DEFAULT_HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// Default time from the engine's `Hello` to its answer to `Open`.
pub const DEFAULT_OPEN_TIMEOUT: Duration = Duration::from_secs(30);

/// The least time between two engine incident reports.
const INCIDENT_SPACING: Duration = Duration::from_secs(10);

/// How long a crashed engine's last log lines are waited for before the report is written.
const LOG_DRAIN_PATIENCE: Duration = Duration::from_millis(500);

/// The environment variable that sets the engine's log level (`error` ... `trace`).
const LOG_LEVEL_ENV: &str = "VELLORA_LOG";

/// How many times the engine's hard tile deadline the client waits for any answer before it kills
/// an engine that has requests in flight.
const STALL_FACTOR: u32 = 2;

/// How long a reader waits for a process whose pipe closed to finish exiting before killing it.
const CRASH_GRACE: Duration = Duration::from_secs(2);

/// How long [`Client::close`] waits for the engine to leave after `Close` before killing it.
const CLOSE_GRACE: Duration = Duration::from_secs(2);

/// Everything a client needs to start engines for one document.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// The `vellora-engine` executable.
    pub engine: PathBuf,
    /// How the shared tile region is cut into slots.
    pub geometry: SlotGeometry,
    /// Documents larger than this are refused by the engine before it maps them.
    pub max_document_bytes: u64,
    /// Soft and hard time per tile; the engine aborts itself after the hard one.
    pub deadlines: Deadlines,
    /// OS limits for the engine; `None` uses [`ResourceLimits::for_mapped`] over the document and
    /// the region.
    pub limits: Option<ResourceLimits>,
    /// Extra environment variables for the engine (for example where to find PDFium).
    pub env: Vec<(OsString, OsString)>,
    /// How many crashes in a row (no tile delivered in between) are answered with a restart.
    pub max_restarts: u32,
    /// Bytes of finished and pending tiles the cache may hold.
    pub cache_budget_bytes: u64,
    /// Bytes of finished and pending thumbnails the thumbnail cache may hold. It has slots of its
    /// own, added to the region after `geometry`'s, so thumbnails never take room from tiles;
    /// 0 turns thumbnails off.
    pub thumbnail_budget_bytes: u64,
    /// How long a new engine may take to say `Hello` before it is killed ([`Stage::Hello`]).
    pub hello_timeout: Duration,
    /// How long the engine may take to answer `Open` after its `Hello` before it is killed
    /// ([`Stage::Open`]).
    pub open_timeout: Duration,
    /// Where to record engine incidents (it ended unasked, or reported an internal error) with
    /// the last lines it printed ([`crate::crash::write_engine_report`]); `None` records nothing.
    pub crash_dir: Option<PathBuf>,
}

impl ClientConfig {
    /// The usual configuration for the engine at `engine` with the given tile region.
    #[must_use]
    pub fn new(engine: impl Into<PathBuf>, geometry: SlotGeometry) -> Self {
        Self {
            engine: engine.into(),
            geometry,
            max_document_bytes: DEFAULT_MAX_DOCUMENT_BYTES,
            deadlines: Deadlines::default(),
            limits: None,
            env: Vec::new(),
            max_restarts: DEFAULT_MAX_RESTARTS,
            cache_budget_bytes: DEFAULT_BUDGET_BYTES,
            thumbnail_budget_bytes: DEFAULT_THUMBNAIL_BUDGET_BYTES,
            hello_timeout: DEFAULT_HELLO_TIMEOUT,
            open_timeout: DEFAULT_OPEN_TIMEOUT,
            crash_dir: None,
        }
    }
}

/// What the engine failed to do in time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Stage {
    /// Say `Hello` after being started.
    Hello,
    /// Answer `Open`.
    Open,
    /// Answer any of the requests in flight.
    Tile,
}

/// Why a client call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The document file could not be opened.
    #[error("cannot open the document: {0}")]
    Document(#[source] io::Error),
    /// The shared tile region could not be created or read.
    #[error("tile region: {0}")]
    Region(#[from] vellora_shm::Error),
    /// The engine process could not be started.
    #[error(transparent)]
    Spawn(#[from] SpawnError),
    /// A thread for the engine's pipe could not be started.
    #[error("cannot start the reader thread: {0}")]
    Thread(#[source] io::Error),
    /// A request could not be sent (it was refused by validation, or the pipe is broken).
    #[error("cannot send the request: {0}")]
    Protocol(#[from] vellora_ipc::Error),
    /// No engine is running: it crashed and is restarting, or the client gave up.
    #[error("the engine is not running")]
    EngineUnavailable,
    /// The client was closed.
    #[error("the client is closed")]
    Closed,
    /// A password was given for a document that is already open.
    #[error("the document is already open")]
    AlreadyOpen,
    /// The tile cannot exist: a slot of the region is smaller than a tile.
    #[error("invalid tile")]
    InvalidTile,
    /// The cache refused the tile for a reason other than being full.
    #[error("tile cache: {0}")]
    Cache(#[from] ReserveError),
}

/// What [`Client::request_cached_tile`] did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum TileLookup {
    /// The tile is cached; read it with [`Client::read_tile`]. Nothing was sent.
    Ready,
    /// A request for this tile is already in flight; its [`Event::TileReady`] will come.
    InFlight,
    /// A new request was sent; the id is the one its answer carries.
    Requested(RequestId),
    /// Every slot is held by requests in flight. Ask again after some finish.
    Full,
}

/// A tile to render.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TileRequest {
    /// Zero-based page index.
    pub page: u32,
    /// Device pixels per point, in (0, 64].
    pub scale: f32,
    /// The part of the scaled page to render.
    pub rect: TileRect,
    /// The slot of the region the pixels go to. The caller must not read it before the matching
    /// [`Event::TileReady`], and must not reuse it while a request for it is outstanding.
    pub slot: SlotId,
    /// How urgently the tile is wanted.
    pub priority: Priority,
}

/// Something that happened, in the order it happened.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Event {
    /// The document is open. Sent again after every restart; the page count is unchanged unless
    /// the file changed on disk.
    Opened {
        /// Total number of pages.
        page_count: u32,
        /// Sizes of the first pages (the protocol sends at most 4096 at open).
        page_sizes: Vec<PageSize>,
        /// Why the document needed repair; empty if it did not (at most 32 entries).
        repairs: Vec<Repair>,
    },
    /// A tile is in its slot and may be read with [`Client::read_slot`].
    TileReady {
        /// The request it answers.
        request: RequestId,
        /// The slot that now holds the pixels.
        slot: SlotId,
    },
    /// A request failed, or the engine reported a problem of its own (`request` is `None`).
    RequestFailed {
        /// The failed request.
        request: Option<RequestId>,
        /// Broad class of failure.
        kind: ErrorKind,
        /// Human-readable detail.
        message: String,
    },
    /// The engine ended without being asked to.
    EngineCrashed {
        /// How it ended.
        crash: Crash,
        /// Requests that were in flight and will never be answered, oldest first.
        lost: Vec<RequestId>,
        /// A new engine is being started; ask again for `lost` after [`Event::EngineRestarted`].
        will_restart: bool,
    },
    /// A new engine process is running; [`Event::Opened`] follows once it has the document.
    EngineRestarted,
    /// The engine did not answer in time and was killed. For [`Stage::Hello`] and [`Stage::Open`]
    /// the client gives up and no [`Event::Failed`] follows (this is the only report). For
    /// [`Stage::Tile`] an [`Event::EngineCrashed`] follows at once, and the engine restarts if its
    /// budget allows.
    EngineTimeout {
        /// What it failed to do.
        stage: Stage,
        /// How long the client waited: the configured timeout for `Hello` and `Open`, the time
        /// since the last answer for `Tile`.
        waited: Duration,
    },
    /// The file on disk is not what was opened. Reported when the engine restarts; the new engine
    /// maps whatever is there now, so the page count and the pages may differ from before.
    DocumentChanged {
        /// How it differs.
        change: Change,
    },
    /// The client cannot continue (protocol mismatch, or the engine cannot be started again).
    /// Every later request fails.
    Failed {
        /// What went wrong.
        reason: String,
    },
}

/// Where the engine process is in its life.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Running,
    Restarting,
    Failed,
    Closed,
}

/// What the client knows about the open document.
#[derive(Debug)]
struct OpenedInfo {
    page_count: u32,
    page_sizes: Vec<PageSize>,
    repairs: Vec<Repair>,
}

/// Bookkeeping that needs no process: which requests are in flight, and the restart budget.
#[derive(Debug)]
struct Ledger {
    pending: HashSet<RequestId>,
    next_request: u64,
    crashes_without_progress: u32,
    max_restarts: u32,
}

/// What to do with one response from the engine.
#[derive(Debug, PartialEq)]
enum Accepted {
    Event(Event),
    /// An answer to a request that was cancelled or never made.
    Dropped,
    /// The engine broke the protocol.
    Violation,
}

impl Ledger {
    fn new(max_restarts: u32) -> Self {
        Self {
            pending: HashSet::new(),
            next_request: 1,
            crashes_without_progress: 0,
            max_restarts,
        }
    }

    /// Registers a new request and returns its id.
    fn begin(&mut self) -> RequestId {
        let id = RequestId(self.next_request);
        self.next_request = self.next_request.wrapping_add(1);
        self.pending.insert(id);
        id
    }

    /// Forgets a request; `true` if it was in flight.
    fn forget(&mut self, id: RequestId) -> bool {
        self.pending.remove(&id)
    }

    fn accept(&mut self, response: Response) -> Accepted {
        match response {
            // The handshake is done by the time responses are delivered.
            Response::Hello { .. } => Accepted::Violation,
            Response::Opened {
                page_count,
                page_sizes,
                repairs,
            } => Accepted::Event(Event::Opened {
                page_count,
                page_sizes,
                repairs,
            }),
            Response::TileReady { req_id, slot } => {
                // A cancelled request may still be answered if the engine had finished it before
                // the `Cancel` arrived; the caller was told it is gone, so the answer is dropped.
                if self.pending.remove(&req_id) {
                    self.crashes_without_progress = 0;
                    Accepted::Event(Event::TileReady {
                        request: req_id,
                        slot,
                    })
                } else {
                    Accepted::Dropped
                }
            }
            Response::Error {
                req_id,
                kind,
                message,
            } => match req_id {
                Some(id) if !self.pending.remove(&id) => Accepted::Dropped,
                request => Accepted::Event(Event::RequestFailed {
                    request,
                    kind,
                    message,
                }),
            },
        }
    }

    /// The engine died: the requests that were in flight, and whether to start another.
    fn crashed(&mut self) -> (Vec<RequestId>, bool) {
        self.crashes_without_progress = self.crashes_without_progress.saturating_add(1);
        let mut lost: Vec<_> = self.pending.drain().collect();
        lost.sort_by_key(|id| id.0);
        (lost, self.crashes_without_progress <= self.max_restarts)
    }
}

#[derive(Debug)]
struct State {
    phase: Phase,
    /// Counts engine processes; a reader acts only while it is the current generation.
    generation: u64,
    events: VecDeque<Event>,
    ledger: Ledger,
    process: Option<EngineProcess>,
    /// The thread that reads the current engine's log lines; it ends when the engine does.
    log_reader: Option<thread::JoinHandle<()>>,
    opened: Option<OpenedInfo>,
    /// The password the user gave, while the engine is taking it or has accepted it. Kept so that
    /// an engine that is restarted after a crash can open the document again without asking; it is
    /// dropped (and so wiped) when the engine refuses it, and with the client. Never logged.
    password: Option<Password>,
    cache: TileCache,
    /// The thumbnails, in slots after the tile cache's.
    thumbnails: TileCache,
    /// The cache entry each in-flight cached request is for.
    tile_keys: HashMap<RequestId, (Pool, TileKey)>,
    /// While the current engine is starting: what it owes us next and when that is overdue.
    startup: Option<(Stage, Instant)>,
    /// When the current engine last answered, or got its first request in flight after being idle.
    activity: Instant,
    /// The watchdog has killed the current engine; its reader is about to report the crash.
    timed_out: bool,
    /// The file as last reported to the user.
    seen: Seen,
}

/// What the watchdog should do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Verdict {
    /// The client is finished; the watchdog ends.
    Done,
    /// Nothing is overdue; look again at the given time, or when woken if there is none.
    Wait(Option<Instant>),
    /// The engine owes an answer it has not given.
    Overdue(Stage),
}

impl State {
    /// Registers a new request. The stall clock starts when the engine goes from idle to busy.
    fn begin_request(&mut self, now: Instant) -> RequestId {
        if self.ledger.pending.is_empty() {
            self.activity = now;
        }
        self.ledger.begin()
    }

    /// Decides what the watchdog does at `now`; `hard` is the engine's hard tile deadline.
    fn verdict(&self, hard: Duration, now: Instant) -> Verdict {
        match self.phase {
            Phase::Failed | Phase::Closed => return Verdict::Done,
            // A new engine will start the clocks; a killed one is being replaced.
            Phase::Restarting => return Verdict::Wait(None),
            Phase::Running if self.timed_out => return Verdict::Wait(None),
            Phase::Running => {}
        }
        if let Some((stage, deadline)) = self.startup {
            // Tiles asked for during start-up wait for `Open`, which has its own deadline.
            return if now >= deadline {
                Verdict::Overdue(stage)
            } else {
                Verdict::Wait(Some(deadline))
            };
        }
        if self.ledger.pending.is_empty() {
            return Verdict::Wait(None);
        }
        // An absurd deadline (the tests' "never") overflows `Instant`: no limit then.
        let limit = hard
            .checked_mul(STALL_FACTOR)
            .and_then(|stall| self.activity.checked_add(stall));
        match limit {
            Some(limit) if now >= limit => Verdict::Overdue(Stage::Tile),
            limit => Verdict::Wait(limit),
        }
    }

    fn pool(&mut self, pool: Pool) -> &mut TileCache {
        match pool {
            Pool::Tiles => &mut self.cache,
            Pool::Thumbnails => &mut self.thumbnails,
        }
    }

    /// Updates the cache for what the engine just said about a request.
    fn settle_tile(&mut self, event: &Event) {
        match event {
            Event::TileReady { request, .. } => {
                if let Some((pool, key)) = self.tile_keys.remove(request) {
                    // `false`: invalidated while in flight, the slot is freed instead.
                    self.pool(pool).complete(&key);
                }
            }
            Event::RequestFailed {
                request: Some(request),
                ..
            } => {
                if let Some((pool, key)) = self.tile_keys.remove(request) {
                    self.pool(pool).abandon(&key);
                }
            }
            _ => {}
        }
    }
}

/// Which cache a request belongs to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Pool {
    Tiles,
    Thumbnails,
}

/// The write side of the current engine's pipe. Locked separately from [`State`]: a write can
/// block on a full pipe, and the reader needs `State` to drain the engine's answers.
#[derive(Debug)]
struct Sink(Option<BufWriter<File>>);

#[derive(Debug)]
struct Shared {
    config: ClientConfig,
    path: PathBuf,
    document: File,
    region_file: File,
    region: TileRegion,
    state: Mutex<State>,
    sink: Mutex<Sink>,
    /// The last lines the engines printed, for a crash report.
    engine_log: Arc<LineRing>,
    /// When an incident was last recorded, so that a crash loop does not flood the folder.
    last_incident: Mutex<Option<Instant>>,
    /// Signals events to [`Client::wait_events`].
    wake: Condvar,
    /// Signals the watchdog that a deadline may have changed.
    watch: Condvar,
}

impl Shared {
    /// Records that the engine ended unasked or reported an internal error, with the last lines it
    /// printed, if [`ClientConfig::crash_dir`] says where. At most one every [`INCIDENT_SPACING`].
    fn record_incident(&self, what: &str, details: &str) {
        let Some(dir) = &self.config.crash_dir else {
            return;
        };
        {
            let mut last = self
                .last_incident
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            let now = Instant::now();
            if last.is_some_and(|at| now.duration_since(at) < INCIDENT_SPACING) {
                return;
            }
            *last = Some(now);
        }
        match crate::crash::write_engine_report(dir, what, details, &self.engine_log.tail()) {
            Ok(path) => tracing::warn!(what, report = %path.display(), "engine incident recorded"),
            Err(error) => tracing::error!(%error, "cannot record the engine incident"),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        // A poisoned lock only means another thread panicked; the state stays consistent
        // because every critical section is a few plain assignments.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn sink(&self) -> MutexGuard<'_, Sink> {
        self.sink.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn push(&self, state: &mut State, event: Event) {
        state.events.push_back(event);
        self.wake.notify_all();
        // Most events change what the watchdog waits for (a phase, an answer, a restart).
        self.watch.notify_all();
    }
}

/// How a reader's pipe ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum End {
    /// The pipe closed or failed.
    Eof,
    /// The engine sent something that is not the protocol; it is killed.
    Violation,
}

/// A freshly started engine, not yet installed.
struct Launched {
    process: EngineProcess,
    log_reader: Option<thread::JoinHandle<()>>,
    writer: BufWriter<File>,
    stdout: File,
}

/// A handle on one open document and the engine rendering it.
///
/// Dropping the client closes it.
#[derive(Debug)]
pub struct Client {
    shared: Arc<Shared>,
}

impl Client {
    /// Opens the document at `path`: starts an engine over it and sends `Open`. The answer
    /// arrives later as [`Event::Opened`] (or [`Event::RequestFailed`] if the engine cannot read
    /// the file).
    ///
    /// # Errors
    ///
    /// [`ClientError`] if the file cannot be opened, the region cannot be created or the engine
    /// cannot be started.
    pub fn open(mut config: ClientConfig, path: &Path) -> Result<Self, ClientError> {
        let document = document::open_read_only(path).map_err(ClientError::Document)?;
        let seen = Seen::new(&document, path).map_err(ClientError::Document)?;
        // The thumbnails' slots follow the tile cache's in one region. The geometry the rest of
        // the client sees (and the engine is started with) is the whole region.
        let tile_slots = config.geometry.slot_count();
        let thumbnail_slots = thumbnail_slots(config.geometry, config.thumbnail_budget_bytes);
        if thumbnail_slots > 0 {
            config.geometry =
                SlotGeometry::new(tile_slots + thumbnail_slots, config.geometry.slot_bytes())?;
        }
        let (region, region_file) = TileRegion::create(config.geometry)?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                phase: Phase::Restarting,
                generation: 0,
                events: VecDeque::new(),
                ledger: Ledger::new(config.max_restarts),
                process: None,
                log_reader: None,
                opened: None,
                password: None,
                cache: TileCache::over_slots(
                    config.geometry,
                    0..tile_slots,
                    config.cache_budget_bytes,
                ),
                thumbnails: TileCache::over_slots(
                    config.geometry,
                    tile_slots..tile_slots + thumbnail_slots,
                    config.thumbnail_budget_bytes,
                ),
                tile_keys: HashMap::new(),
                startup: None,
                activity: Instant::now(),
                timed_out: false,
                seen,
            }),
            sink: Mutex::new(Sink(None)),
            engine_log: Arc::new(LineRing::new(crate::capture::TAIL_LINES)),
            last_incident: Mutex::new(None),
            wake: Condvar::new(),
            watch: Condvar::new(),
            config,
            path: path.to_owned(),
            document,
            region_file,
            region,
        });
        let launched = launch(&shared)?;
        install(&shared, launched, false)?;
        // From here on dropping the client closes it, which also ends the watchdog.
        let client = Self { shared };
        let watched = Arc::clone(&client.shared);
        thread::Builder::new()
            .name("vellora-engine-watchdog".into())
            .spawn(move || watch(&watched))
            .map_err(ClientError::Thread)?;
        Ok(client)
    }

    /// Opens an encrypted document with `password`, after the engine answered `Open` with
    /// [`ErrorKind::PasswordRequired`] or [`ErrorKind::WrongPassword`] (both arrive as
    /// [`Event::RequestFailed`] with no request). The answer is [`Event::Opened`], or the same
    /// failure again for a wrong password; the engine stays up between attempts.
    ///
    /// The client keeps the password in memory while the engine holds the document, so that an
    /// engine restarted after a crash opens it without asking the user again. It is wiped when the
    /// engine refuses it and when the client is dropped, and never written anywhere.
    ///
    /// # Errors
    ///
    /// [`ClientError::AlreadyOpen`] if the document is open, [`ClientError::Protocol`] if the
    /// password is not acceptable on the wire (it contains NUL, or is over 256 bytes),
    /// [`ClientError::EngineUnavailable`] while the engine is down, [`ClientError::Closed`].
    pub fn submit_password(&self, password: &str) -> Result<(), ClientError> {
        let request;
        {
            let mut state = self.shared.state();
            Self::check_running(&state)?;
            if state.opened.is_some() {
                return Err(ClientError::AlreadyOpen);
            }
            let Some(process) = state.process.as_ref() else {
                return Err(ClientError::EngineUnavailable);
            };
            request = Request::Open {
                handle_token: process.document_token().get(),
                password: Some(Password::new(password)),
            };
            // The watchdog times the answer like any other `Open`.
            state.startup = Some((
                Stage::Open,
                Instant::now() + self.shared.config.open_timeout,
            ));
            state.password = match &request {
                Request::Open { password, .. } => password.clone(),
                _ => None,
            };
        }
        self.shared.watch.notify_all();
        let sent = match self.shared.sink().0.as_mut() {
            Some(writer) => write_frame(writer, &request).map_err(ClientError::from),
            None => Err(ClientError::EngineUnavailable),
        };
        if sent.is_err() {
            let mut state = self.shared.state();
            state.password = None;
            // Nothing was asked, so there is nothing to time.
            state.startup = None;
        }
        sent
    }

    /// Asks for a tile. Returns the id its answer will carry.
    ///
    /// # Errors
    ///
    /// [`ClientError::EngineUnavailable`] while the engine is down, [`ClientError::Closed`] after
    /// [`close`](Self::close), [`ClientError::Protocol`] if the request is invalid (scale or tile
    /// size out of range) or the pipe broke.
    pub fn request_tile(&self, tile: &TileRequest) -> Result<RequestId, ClientError> {
        let id = {
            let mut state = self.shared.state();
            Self::check_running(&state)?;
            state.begin_request(Instant::now())
        };
        self.shared.watch.notify_all();
        let request = Request::RenderTile {
            req_id: id,
            page: tile.page,
            scale: tile.scale,
            rect: tile.rect,
            slot: tile.slot,
            priority: tile.priority,
        };
        let sent = match self.shared.sink().0.as_mut() {
            Some(writer) => write_frame(writer, &request).map_err(ClientError::from),
            None => Err(ClientError::EngineUnavailable),
        };
        if let Err(error) = sent {
            self.shared.state().ledger.forget(id);
            return Err(error);
        }
        Ok(id)
    }

    /// Returns the tile at `key` from the cache, or asks the engine for it. The tile is
    /// [`TILE_PIXELS`] square at the bucket's scale ([`crate::ScaleBucket::scale`]), `key.x` and
    /// `key.y` counting tiles from the page's top left.
    ///
    /// # Errors
    ///
    /// As [`request_tile`](Self::request_tile), plus [`ClientError::InvalidTile`] if a slot cannot
    /// hold a tile and [`ClientError::Cache`] if the budget cannot hold one.
    pub fn request_cached_tile(
        &self,
        key: TileKey,
        priority: Priority,
    ) -> Result<TileLookup, ClientError> {
        let (id, slot) = {
            let mut state = self.shared.state();
            Self::check_running(&state)?;
            if state.cache.get(&key).is_some() {
                return Ok(TileLookup::Ready);
            }
            if state.cache.contains(&key) {
                return Ok(TileLookup::InFlight);
            }
            if u64::from(self.shared.config.geometry.slot_bytes()) < TILE_BYTES {
                return Err(ClientError::InvalidTile);
            }
            let slot = match state.cache.reserve(key, TILE_BYTES) {
                Ok(slot) => slot,
                Err(ReserveError::Full) => return Ok(TileLookup::Full),
                Err(error) => return Err(error.into()),
            };
            let id = state.begin_request(Instant::now());
            state.tile_keys.insert(id, (Pool::Tiles, key));
            (id, slot)
        };
        self.shared.watch.notify_all();
        let request = Request::RenderTile {
            req_id: id,
            page: key.page,
            scale: key.scale.scale(),
            rect: TileRect {
                // Saturating: a position the protocol refuses is reported by validation below.
                x: key.x.saturating_mul(TILE_PIXELS),
                y: key.y.saturating_mul(TILE_PIXELS),
                width: TILE_PIXELS,
                height: TILE_PIXELS,
            },
            slot,
            priority,
        };
        let sent = match self.shared.sink().0.as_mut() {
            Some(writer) => write_frame(writer, &request).map_err(ClientError::from),
            None => Err(ClientError::EngineUnavailable),
        };
        if let Err(error) = sent {
            let mut state = self.shared.state();
            state.ledger.forget(id);
            if state.tile_keys.remove(&id).is_some() {
                state.cache.abandon(&key);
            }
            return Err(error);
        }
        Ok(TileLookup::Requested(id))
    }

    /// Returns the thumbnail of `page` at `scale` from the thumbnail cache, or asks the engine for
    /// it at [`Priority::Thumbnail`]. A thumbnail is the whole page rendered `width` x `height`
    /// pixels (each at most [`TILE_PIXELS`]) at the bucket's scale; the same page and bucket must
    /// always be asked for with the same size. Its cache has its own budget and slots
    /// ([`ClientConfig::thumbnail_budget_bytes`]), so thumbnails and tiles never evict each other.
    ///
    /// # Errors
    ///
    /// As [`request_cached_tile`](Self::request_cached_tile), and [`ClientError::InvalidTile`] for
    /// a size outside `1..=`[`TILE_PIXELS`] or when thumbnails are off.
    pub fn request_thumbnail(
        &self,
        page: u32,
        scale: ScaleBucket,
        width: u32,
        height: u32,
    ) -> Result<TileLookup, ClientError> {
        if !(1..=TILE_PIXELS).contains(&width) || !(1..=TILE_PIXELS).contains(&height) {
            return Err(ClientError::InvalidTile);
        }
        let key = TileKey {
            page,
            scale,
            x: 0,
            y: 0,
        };
        let (id, slot) = {
            let mut state = self.shared.state();
            Self::check_running(&state)?;
            if state.thumbnails.get(&key).is_some() {
                return Ok(TileLookup::Ready);
            }
            if state.thumbnails.contains(&key) {
                return Ok(TileLookup::InFlight);
            }
            let bytes = u64::from(width) * u64::from(height) * 4;
            let slot = match state.thumbnails.reserve(key, bytes) {
                Ok(slot) => slot,
                Err(ReserveError::Full) => return Ok(TileLookup::Full),
                Err(ReserveError::TooLarge(_)) => return Err(ClientError::InvalidTile),
                Err(error) => return Err(error.into()),
            };
            let id = state.begin_request(Instant::now());
            state.tile_keys.insert(id, (Pool::Thumbnails, key));
            (id, slot)
        };
        self.shared.watch.notify_all();
        let request = Request::RenderTile {
            req_id: id,
            page,
            scale: scale.scale(),
            rect: TileRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            slot,
            priority: Priority::Thumbnail,
        };
        let sent = match self.shared.sink().0.as_mut() {
            Some(writer) => write_frame(writer, &request).map_err(ClientError::from),
            None => Err(ClientError::EngineUnavailable),
        };
        if let Err(error) = sent {
            let mut state = self.shared.state();
            state.ledger.forget(id);
            if state.tile_keys.remove(&id).is_some() {
                state.thumbnails.abandon(&key);
            }
            return Err(error);
        }
        Ok(TileLookup::Requested(id))
    }

    /// Copies a ready thumbnail into `out`, which must be exactly `width * height * 4` bytes of
    /// the size it was requested with, and marks it most recently used. `false` if it is not
    /// ready.
    ///
    /// # Errors
    ///
    /// [`ClientError::Region`] for a buffer longer than a slot.
    pub fn read_thumbnail(
        &self,
        page: u32,
        scale: ScaleBucket,
        out: &mut [u8],
    ) -> Result<bool, ClientError> {
        let key = TileKey {
            page,
            scale,
            x: 0,
            y: 0,
        };
        // Held across the copy, as in `read_tile`.
        let mut state = self.shared.state();
        let Some(slot) = state.thumbnails.get(&key) else {
            return Ok(false);
        };
        self.shared.region.read_slot_prefix(slot.0, out)?;
        Ok(true)
    }

    /// Copies a ready tile into `out`, which must be exactly one slot long, and marks it most
    /// recently used. `false` if the tile is not ready (absent, in flight or invalidated).
    ///
    /// # Errors
    ///
    /// [`ClientError::Region`] for a buffer of the wrong length.
    pub fn read_tile(&self, key: &TileKey, out: &mut [u8]) -> Result<bool, ClientError> {
        // The lock is held across the copy: the slot can only be handed out again by a request,
        // which needs this lock, so the engine cannot overwrite it meanwhile.
        let mut state = self.shared.state();
        let Some(slot) = state.cache.get(key) else {
            return Ok(false);
        };
        self.shared.region.read_slot(slot.0, out)?;
        Ok(true)
    }

    /// Forgets the cached tiles of `page` (it changed): ready ones at once, in-flight ones when the
    /// engine answers. Returns how many ready tiles were dropped.
    pub fn invalidate_page(&self, page: u32) -> usize {
        let mut state = self.shared.state();
        state.cache.invalidate_page(page) + state.thumbnails.invalidate_page(page)
    }

    /// Number of tiles in the cache, in flight and ready.
    #[must_use]
    pub fn cached_tiles(&self) -> usize {
        self.shared.state().cache.len()
    }

    /// Number of thumbnails in the thumbnail cache, in flight and ready.
    #[must_use]
    pub fn cached_thumbnails(&self) -> usize {
        self.shared.state().thumbnails.len()
    }

    /// Withdraws a request. Queued work is dropped by the engine; a render already running
    /// finishes, but the caller never hears of it. Returns `false` if the request was not in
    /// flight (already answered, lost in a crash, or unknown), in which case nothing is sent.
    ///
    /// The engine may still write the request's slot until its next answer, so the slot is not
    /// safe to reuse at once.
    pub fn cancel(&self, request: RequestId) -> bool {
        {
            let mut state = self.shared.state();
            if !state.ledger.forget(request) {
                return false;
            }
            if let Some((pool, key)) = state.tile_keys.remove(&request) {
                state.pool(pool).abandon(&key);
            }
        }
        if let Some(writer) = self.shared.sink().0.as_mut() {
            // A broken pipe means the engine is gone, which cancels the work anyway.
            let _ = write_frame(writer, &Request::Cancel { req_id: request });
        }
        true
    }

    /// Takes every queued event, oldest first. Never blocks.
    pub fn poll_events(&self) -> Vec<Event> {
        self.shared.state().events.drain(..).collect()
    }

    /// Like [`poll_events`](Self::poll_events), but waits up to `timeout` for the first event.
    pub fn wait_events(&self, timeout: Duration) -> Vec<Event> {
        let deadline = Instant::now() + timeout;
        let mut state = self.shared.state();
        loop {
            if !state.events.is_empty() {
                return state.events.drain(..).collect();
            }
            let Some(left) = deadline.checked_duration_since(Instant::now()) else {
                return Vec::new();
            };
            state = self
                .shared
                .wake
                .wait_timeout(state, left)
                .unwrap_or_else(PoisonError::into_inner)
                .0;
        }
    }

    /// Copies a slot of the shared region into `out`, which must be exactly one slot long. Call
    /// it only after the matching [`Event::TileReady`]; the contents come from the engine and are
    /// not trusted.
    ///
    /// # Errors
    ///
    /// [`ClientError::Region`] for a slot outside the region or a buffer of the wrong length.
    pub fn read_slot(&self, slot: SlotId, out: &mut [u8]) -> Result<(), ClientError> {
        Ok(self.shared.region.read_slot(slot.0, out)?)
    }

    /// How the tile region is cut into slots.
    #[must_use]
    pub fn geometry(&self) -> SlotGeometry {
        self.shared.config.geometry
    }

    /// Number of pages, once the document is open.
    #[must_use]
    pub fn page_count(&self) -> Option<u32> {
        self.shared.state().opened.as_ref().map(|o| o.page_count)
    }

    /// Size of a page in points. `None` until the document is open, and for pages beyond the
    /// first chunk the protocol sends at open (4096).
    #[must_use]
    pub fn page_size(&self, page: u32) -> Option<PageSize> {
        let state = self.shared.state();
        let index = usize::try_from(page).ok()?;
        state.opened.as_ref()?.page_sizes.get(index).copied()
    }

    /// Why the engine had to repair the document; empty if it did not (or before it is open).
    #[must_use]
    pub fn repairs(&self) -> Vec<Repair> {
        self.shared
            .state()
            .opened
            .as_ref()
            .map_or_else(Vec::new, |o| o.repairs.clone())
    }

    /// Whether the engine had to repair the document.
    #[must_use]
    pub fn repaired(&self) -> bool {
        self.shared
            .state()
            .opened
            .as_ref()
            .is_some_and(|o| !o.repairs.is_empty())
    }

    /// The last 200 lines the engines of this client printed (oldest first, each marked with the
    /// engine's process id), for a crash report. Their text is the engine's: untrusted data.
    #[must_use]
    pub fn engine_log_tail(&self) -> Vec<String> {
        self.shared.engine_log.tail()
    }

    /// The operating-system id of the current engine process, if one is running.
    #[must_use]
    pub fn engine_id(&self) -> Option<u32> {
        self.shared.state().process.as_ref().map(EngineProcess::id)
    }

    /// Closes the document and ends the engine: asks it to leave and kills it if it does not
    /// within a couple of seconds. Idempotent; later calls fail with [`ClientError::Closed`].
    pub fn close(&self) {
        let process = {
            let mut state = self.shared.state();
            if state.phase == Phase::Closed {
                return;
            }
            state.phase = Phase::Closed;
            state.ledger.pending.clear();
            state.process.take()
        };
        let writer = self.shared.sink().0.take();
        if let Some(mut writer) = writer {
            // The engine also leaves when its input closes, so a failed `Close` changes nothing.
            let _ = write_frame(&mut writer, &Request::Close);
        }
        if let Some(mut process) = process {
            // Reaps a healthy engine; dropping it kills one that did not leave in time.
            let _ = process.wait_timeout(CLOSE_GRACE);
        }
        self.shared.wake.notify_all();
        self.shared.watch.notify_all();
    }

    fn check_running(state: &State) -> Result<(), ClientError> {
        match state.phase {
            Phase::Running => Ok(()),
            Phase::Restarting | Phase::Failed => Err(ClientError::EngineUnavailable),
            Phase::Closed => Err(ClientError::Closed),
        }
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        self.close();
    }
}

/// How many slots, added to a region of `geometry`, hold `budget_bytes` of thumbnails: as many
/// whole slots as the budget fills, as far as the region limit allows.
fn thumbnail_slots(geometry: SlotGeometry, budget_bytes: u64) -> u32 {
    let slot_bytes = u64::from(geometry.slot_bytes());
    let room = vellora_shm::MAX_REGION_BYTES / slot_bytes - u64::from(geometry.slot_count());
    // At most the region limit over the slot size, so it fits `u32`.
    u32::try_from((budget_bytes / slot_bytes).min(room)).unwrap_or(0)
}

/// Starts an engine over the document and region and sends `Hello` and `Open`.
fn launch(shared: &Shared) -> Result<Launched, ClientError> {
    let config = &shared.config;
    let mut spawn = SpawnConfig::new(
        &config.engine,
        &shared.document,
        &shared.region_file,
        config.geometry,
    );
    spawn.max_document_bytes = config.max_document_bytes;
    spawn.deadlines = config.deadlines;
    spawn.env.clone_from(&config.env);
    // The engine logs at `warn` unless told otherwise; the log files want its `info` lines (a
    // document was opened, the session ended) unless the user chose a level.
    let level_chosen = config.env.iter().any(|(name, _)| name == LOG_LEVEL_ENV)
        || std::env::var_os(LOG_LEVEL_ENV).is_some();
    if !level_chosen {
        spawn.env.push((LOG_LEVEL_ENV.into(), "info".into()));
    }
    if let Some(limits) = config.limits {
        spawn.limits = limits;
    }
    let mut process = EngineProcess::spawn(&spawn)?;
    let (Some(stdin), Some(stdout)) = (process.take_stdin(), process.take_stdout()) else {
        // Both pipes were requested at spawn; losing one means there is nothing to talk to.
        return Err(ClientError::EngineUnavailable);
    };
    // The engine's log lines: someone must read them, or its next log line would block.
    let log_reader = process.take_stderr().and_then(|stderr| {
        crate::capture::spawn_reader(process.id(), stderr, Arc::clone(&shared.engine_log))
    });
    tracing::info!(pid = process.id(), "engine started");
    let mut writer = BufWriter::new(stdin);
    // The engine says `Hello` first and reads ours afterwards, but a pipe holds both of our
    // messages, so there is no need to wait for its `Hello` before sending `Open`.
    write_frame(
        &mut writer,
        &Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )?;
    // A restarted engine gets the password the user gave, if one was accepted before the crash.
    let password = shared.state().password.clone();
    write_frame(
        &mut writer,
        &Request::Open {
            handle_token: process.document_token().get(),
            password,
        },
    )?;
    Ok(Launched {
        process,
        log_reader,
        writer,
        stdout,
    })
}

/// Makes `launched` the current engine and starts its reader. Dropping it instead (the client was
/// closed meanwhile) kills the process.
fn install(shared: &Arc<Shared>, launched: Launched, restarted: bool) -> Result<(), ClientError> {
    let Launched {
        process,
        log_reader,
        writer,
        stdout,
    } = launched;
    let mut state = shared.state();
    if state.phase == Phase::Closed {
        return Err(ClientError::Closed);
    }
    state.generation += 1;
    let generation = state.generation;
    let reader_shared = Arc::clone(shared);
    // The reader blocks on `state` until this critical section ends, so nothing it queues can
    // come before the `EngineRestarted` pushed below.
    thread::Builder::new()
        .name("vellora-engine-reader".into())
        .spawn(move || read_responses(&reader_shared, generation, stdout))
        .map_err(ClientError::Thread)?;
    state.process = Some(process);
    state.log_reader = log_reader;
    state.phase = Phase::Running;
    let now = Instant::now();
    state.startup = Some((Stage::Hello, now + shared.config.hello_timeout));
    state.activity = now;
    state.timed_out = false;
    shared.sink().0 = Some(writer);
    if restarted {
        shared.push(&mut state, Event::EngineRestarted);
    }
    shared.watch.notify_all();
    Ok(())
}

/// The reader thread of one engine process.
fn read_responses(shared: &Arc<Shared>, generation: u64, stdout: File) {
    let mut input = BufReader::new(stdout);

    match read_frame::<_, Response>(&mut input) {
        Ok(Some(Response::Hello { protocol_version })) => {
            if let Err(error) = check_version(protocol_version) {
                fail(shared, generation, &error.to_string());
                return;
            }
            let mut state = shared.state();
            if state.generation != generation || state.phase != Phase::Running {
                return;
            }
            let now = Instant::now();
            state.startup = Some((Stage::Open, now + shared.config.open_timeout));
            state.activity = now;
        }
        Ok(None) | Err(vellora_ipc::Error::Io(_)) => return ended(shared, generation, End::Eof),
        Ok(Some(_)) | Err(_) => return ended(shared, generation, End::Violation),
    }

    let end = loop {
        match read_frame::<_, Response>(&mut input) {
            Ok(Some(response)) => {
                let mut state = shared.state();
                if state.generation != generation || state.phase != Phase::Running {
                    return;
                }
                state.activity = Instant::now();
                let mut incident = None;
                match state.ledger.accept(response) {
                    Accepted::Event(event) => {
                        if let Event::RequestFailed {
                            kind: ErrorKind::Internal,
                            message,
                            ..
                        } = &event
                        {
                            // A panic the engine caught, or another failure of its own.
                            incident = Some(message.clone());
                        }
                        // `Open` is answered, if only with a refusal.
                        if matches!(
                            event,
                            Event::Opened { .. } | Event::RequestFailed { request: None, .. }
                        ) {
                            state.startup = None;
                        }
                        if let Event::RequestFailed {
                            request: None,
                            kind: ErrorKind::PasswordRequired | ErrorKind::WrongPassword,
                            ..
                        } = &event
                        {
                            // Refused: nothing to keep for a restart.
                            state.password = None;
                        }
                        if let Event::Opened {
                            page_count,
                            page_sizes,
                            repairs,
                        } = &event
                        {
                            state.opened = Some(OpenedInfo {
                                page_count: *page_count,
                                page_sizes: page_sizes.clone(),
                                repairs: repairs.clone(),
                            });
                        }
                        state.settle_tile(&event);
                        shared.push(&mut state, event);
                    }
                    Accepted::Dropped => {}
                    Accepted::Violation => break End::Violation,
                }
                drop(state);
                if let Some(message) = incident {
                    // The engine prints a panic's message just before it answers; give the log
                    // reader a moment to take it, without holding up this thread.
                    let shared = Arc::clone(shared);
                    let _ = thread::Builder::new()
                        .name("vellora-incident".into())
                        .spawn(move || {
                            thread::sleep(Duration::from_millis(150));
                            shared
                                .record_incident("the engine reported an internal error", &message);
                        });
                }
            }
            Ok(None) | Err(vellora_ipc::Error::Io(_)) => break End::Eof,
            Err(_) => break End::Violation,
        }
    };
    ended(shared, generation, end);
}

/// The engine of `generation` can not be used: report it and, if allowed, start another.
fn ended(shared: &Arc<Shared>, generation: u64, end: End) {
    let process = {
        let mut state = shared.state();
        if state.generation != generation || state.phase != Phase::Running {
            // Closed by the caller, or already handled.
            return;
        }
        state.phase = Phase::Restarting;
        (state.process.take(), state.log_reader.take())
    };
    let (process, log_reader) = process;
    let Some(mut process) = process else { return };
    if end == End::Violation {
        process.kill();
    }
    let crash = process.crash(CRASH_GRACE);
    tracing::warn!(?crash, "engine ended without being asked");
    drop(process);
    // The process is gone, so its log pipe ends: let the reader take the last lines (a panic
    // message, say) before the report copies them.
    if let Some(reader) = &log_reader {
        let deadline = Instant::now() + LOG_DRAIN_PATIENCE;
        while !reader.is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(5));
        }
    }
    shared.record_incident(
        "the engine ended without being asked",
        &format!("{crash:?}"),
    );
    // Dropped only now: a write blocked on the dying engine's pipe fails when the process is gone.
    shared.sink().0 = None;

    let will_restart = {
        let mut state = shared.state();
        if state.phase == Phase::Closed {
            return;
        }
        let (lost, will_restart) = state.ledger.crashed();
        // Their answers will never come; ready tiles stay valid (task 21).
        state.cache.abandon_pending();
        state.thumbnails.abandon_pending();
        state.tile_keys.clear();
        if !will_restart {
            state.phase = Phase::Failed;
        }
        shared.push(
            &mut state,
            Event::EngineCrashed {
                crash,
                lost,
                will_restart,
            },
        );
        will_restart
    };
    if !will_restart {
        return;
    }
    report_document_change(shared);
    let restarted = launch(shared).and_then(|launched| install(shared, launched, true));
    match restarted {
        Ok(()) | Err(ClientError::Closed) => {}
        Err(error) => give_up(shared, &error.to_string()),
    }
}

/// An engine that speaks another protocol version: nothing to restart into.
fn fail(shared: &Arc<Shared>, generation: u64, reason: &str) {
    let process = {
        let mut state = shared.state();
        if state.generation != generation || state.phase != Phase::Running {
            return;
        }
        state.process.take()
    };
    if let Some(mut process) = process {
        process.kill();
    }
    shared.sink().0 = None;
    give_up(shared, reason);
}

fn give_up(shared: &Shared, reason: &str) {
    let mut state = shared.state();
    if state.phase == Phase::Closed {
        return;
    }
    state.phase = Phase::Failed;
    shared.push(
        &mut state,
        Event::Failed {
            reason: reason.to_owned(),
        },
    );
}

/// Tells the caller if the file is not what it was when it was last looked at. Runs before an
/// engine is restarted, because the new engine maps whatever is on disk.
fn report_document_change(shared: &Shared) {
    let mut state = shared.state();
    if state.phase == Phase::Closed {
        return;
    }
    if let Some(change) = state.seen.check(&shared.document, &shared.path) {
        shared.push(&mut state, Event::DocumentChanged { change });
    }
}

/// The watchdog thread: kills an engine that has not met a deadline (see the module
/// documentation). It sleeps until the next deadline or until a state change wakes it.
fn watch(shared: &Shared) {
    let hard = shared.config.deadlines.hard;
    let mut state = shared.state();
    loop {
        let now = Instant::now();
        state = match state.verdict(hard, now) {
            Verdict::Done => return,
            Verdict::Wait(Some(until)) => {
                shared
                    .watch
                    .wait_timeout(state, until.saturating_duration_since(now))
                    .unwrap_or_else(PoisonError::into_inner)
                    .0
            }
            Verdict::Wait(None) => shared
                .watch
                .wait(state)
                .unwrap_or_else(PoisonError::into_inner),
            Verdict::Overdue(Stage::Tile) => {
                state.timed_out = true;
                let waited = now.saturating_duration_since(state.activity);
                shared.push(
                    &mut state,
                    Event::EngineTimeout {
                        stage: Stage::Tile,
                        waited,
                    },
                );
                // Killed under the lock so the reader cannot start a restart in between. Its pipe
                // then closes and `ended` reports the crash and restarts within the budget.
                if let Some(process) = state.process.as_mut() {
                    process.kill();
                }
                state
            }
            Verdict::Overdue(late) => {
                let waited = if late == Stage::Hello {
                    shared.config.hello_timeout
                } else {
                    shared.config.open_timeout
                };
                state.phase = Phase::Failed;
                let process = state.process.take();
                state.startup = None;
                shared.push(
                    &mut state,
                    Event::EngineTimeout {
                        stage: late,
                        waited,
                    },
                );
                drop(state);
                if let Some(mut process) = process {
                    process.kill();
                }
                // Dropped after the kill: a write blocked on the dead engine's pipe fails then.
                shared.sink().0 = None;
                return;
            }
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tile_ready(id: RequestId) -> Response {
        Response::TileReady {
            req_id: id,
            slot: SlotId(0),
        }
    }

    fn failed(id: Option<RequestId>) -> Response {
        Response::Error {
            req_id: id,
            kind: ErrorKind::RenderFailed,
            message: "no".into(),
        }
    }

    #[test]
    fn request_ids_are_distinct_and_answers_clear_them() {
        let mut ledger = Ledger::new(3);
        let (a, b) = (ledger.begin(), ledger.begin());
        assert_ne!(a, b);
        assert_eq!(
            ledger.accept(tile_ready(a)),
            Accepted::Event(Event::TileReady {
                request: a,
                slot: SlotId(0)
            })
        );
        // Answered once; a second answer to the same id is not trusted.
        assert_eq!(ledger.accept(tile_ready(a)), Accepted::Dropped);
        assert!(ledger.pending.contains(&b));
    }

    #[test]
    fn a_cancelled_request_is_never_reported() {
        let mut ledger = Ledger::new(3);
        let id = ledger.begin();
        assert!(ledger.forget(id));
        // The engine had already answered when the cancel reached it.
        assert_eq!(ledger.accept(tile_ready(id)), Accepted::Dropped);
        assert_eq!(ledger.accept(failed(Some(id))), Accepted::Dropped);
        assert!(
            !ledger.forget(id),
            "cancelling twice reports nothing in flight"
        );
    }

    #[test]
    fn an_answer_for_an_id_never_issued_is_dropped() {
        let mut ledger = Ledger::new(3);
        assert_eq!(ledger.accept(tile_ready(RequestId(99))), Accepted::Dropped);
    }

    #[test]
    fn errors_without_a_request_always_reach_the_caller() {
        let mut ledger = Ledger::new(3);
        assert!(matches!(
            ledger.accept(failed(None)),
            Accepted::Event(Event::RequestFailed { request: None, .. })
        ));
        let id = ledger.begin();
        assert!(matches!(
            ledger.accept(failed(Some(id))),
            Accepted::Event(Event::RequestFailed {
                request: Some(r), ..
            }) if r == id
        ));
        assert!(ledger.pending.is_empty());
    }

    #[test]
    fn a_second_hello_is_a_violation() {
        let mut ledger = Ledger::new(3);
        assert_eq!(
            ledger.accept(Response::Hello {
                protocol_version: PROTOCOL_VERSION
            }),
            Accepted::Violation
        );
    }

    #[test]
    fn a_crash_loses_the_requests_in_flight_oldest_first() {
        let mut ledger = Ledger::new(3);
        let ids: Vec<_> = (0..5).map(|_| ledger.begin()).collect();
        ledger.forget(ids[1]);
        let (lost, will_restart) = ledger.crashed();
        assert_eq!(lost, [ids[0], ids[2], ids[3], ids[4]]);
        assert!(will_restart);
        assert!(ledger.pending.is_empty());
    }

    #[test]
    fn restarts_stop_after_the_budget_and_a_delivered_tile_refills_it() {
        let mut ledger = Ledger::new(2);
        assert!(ledger.crashed().1);
        assert!(ledger.crashed().1);
        // The third crash in a row is one too many.
        assert!(!ledger.crashed().1);

        let mut ledger = Ledger::new(2);
        assert!(ledger.crashed().1);
        assert!(ledger.crashed().1);
        let id = ledger.begin();
        assert!(matches!(
            ledger.accept(tile_ready(id)),
            Accepted::Event(Event::TileReady { .. })
        ));
        // Progress was made, so the engine gets its full budget again.
        assert!(ledger.crashed().1);
        assert!(ledger.crashed().1);
        assert!(!ledger.crashed().1);
    }

    #[test]
    fn no_restarts_means_the_first_crash_is_final() {
        let mut ledger = Ledger::new(0);
        assert!(!ledger.crashed().1);
    }
}
