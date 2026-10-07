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
//! # Not done
//!
//! No deadline covers the handshake and `Open` (a hung start-up waits for the user to close the
//! document), and the UI cannot yet ask for a stuck tile to be killed from its side; the engine's
//! own hard deadline aborts it (task 19).

use std::collections::{HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, BufReader, BufWriter};
use std::path::{Path, PathBuf};
use std::process::{ChildStdin, ChildStdout};
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use vellora_engine::{DEFAULT_MAX_DOCUMENT_BYTES, Deadlines};
use vellora_ipc::{
    ErrorKind, PROTOCOL_VERSION, PageSize, Priority, Request, RequestId, Response, SlotId,
    TileRect, check_version, read_frame, write_frame,
};
use vellora_shm::{SlotGeometry, TileRegion};

use crate::cache::{DEFAULT_BUDGET_BYTES, ReserveError, TileCache, TileKey};
use crate::limits::ResourceLimits;
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
        }
    }
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
        /// The document needed repair.
        repaired: bool,
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
    repaired: bool,
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
                repaired,
            } => Accepted::Event(Event::Opened {
                page_count,
                page_sizes,
                repaired,
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
    opened: Option<OpenedInfo>,
    cache: TileCache,
    /// The cache entry each in-flight cached request is for.
    tile_keys: HashMap<RequestId, TileKey>,
}

impl State {
    /// Updates the cache for what the engine just said about a request.
    fn settle_tile(&mut self, event: &Event) {
        match event {
            Event::TileReady { request, .. } => {
                if let Some(key) = self.tile_keys.remove(request) {
                    // `false`: invalidated while in flight, the slot is freed instead.
                    self.cache.complete(&key);
                }
            }
            Event::RequestFailed {
                request: Some(request),
                ..
            } => {
                if let Some(key) = self.tile_keys.remove(request) {
                    self.cache.abandon(&key);
                }
            }
            _ => {}
        }
    }
}

/// The write side of the current engine's pipe. Locked separately from [`State`]: a write can
/// block on a full pipe, and the reader needs `State` to drain the engine's answers.
#[derive(Debug)]
struct Sink(Option<BufWriter<ChildStdin>>);

#[derive(Debug)]
struct Shared {
    config: ClientConfig,
    document: File,
    region_file: File,
    region: TileRegion,
    state: Mutex<State>,
    sink: Mutex<Sink>,
    wake: Condvar,
}

impl Shared {
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
    writer: BufWriter<ChildStdin>,
    stdout: ChildStdout,
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
    pub fn open(config: ClientConfig, path: &Path) -> Result<Self, ClientError> {
        let document = File::open(path).map_err(ClientError::Document)?;
        let (region, region_file) = TileRegion::create(config.geometry)?;
        let shared = Arc::new(Shared {
            state: Mutex::new(State {
                phase: Phase::Restarting,
                generation: 0,
                events: VecDeque::new(),
                ledger: Ledger::new(config.max_restarts),
                process: None,
                opened: None,
                cache: TileCache::new(config.geometry, config.cache_budget_bytes),
                tile_keys: HashMap::new(),
            }),
            sink: Mutex::new(Sink(None)),
            wake: Condvar::new(),
            config,
            document,
            region_file,
            region,
        });
        let launched = launch(&shared)?;
        install(&shared, launched, false)?;
        Ok(Self { shared })
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
            state.ledger.begin()
        };
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
            let id = state.ledger.begin();
            state.tile_keys.insert(id, key);
            (id, slot)
        };
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
        self.shared.state().cache.invalidate_page(page)
    }

    /// Number of tiles in the cache, in flight and ready.
    #[must_use]
    pub fn cached_tiles(&self) -> usize {
        self.shared.state().cache.len()
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
            if let Some(key) = state.tile_keys.remove(&request) {
                state.cache.abandon(&key);
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

    /// Whether the engine had to repair the document.
    #[must_use]
    pub fn repaired(&self) -> bool {
        self.shared
            .state()
            .opened
            .as_ref()
            .is_some_and(|o| o.repaired)
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
    if let Some(limits) = config.limits {
        spawn.limits = limits;
    }
    let mut process = EngineProcess::spawn(&spawn)?;
    let (Some(stdin), Some(stdout)) = (process.take_stdin(), process.take_stdout()) else {
        // Both pipes were requested at spawn; losing one means there is nothing to talk to.
        return Err(ClientError::EngineUnavailable);
    };
    let mut writer = BufWriter::new(stdin);
    // The engine says `Hello` first and reads ours afterwards, but a pipe holds both of our
    // messages, so there is no need to wait for its `Hello` before sending `Open`.
    write_frame(
        &mut writer,
        &Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        },
    )?;
    write_frame(
        &mut writer,
        &Request::Open {
            handle_token: process.document_token().get(),
        },
    )?;
    Ok(Launched {
        process,
        writer,
        stdout,
    })
}

/// Makes `launched` the current engine and starts its reader. Dropping it instead (the client was
/// closed meanwhile) kills the process.
fn install(shared: &Arc<Shared>, launched: Launched, restarted: bool) -> Result<(), ClientError> {
    let Launched {
        process,
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
    state.phase = Phase::Running;
    shared.sink().0 = Some(writer);
    if restarted {
        shared.push(&mut state, Event::EngineRestarted);
    }
    Ok(())
}

/// The reader thread of one engine process.
fn read_responses(shared: &Arc<Shared>, generation: u64, stdout: ChildStdout) {
    let mut input = BufReader::new(stdout);

    match read_frame::<_, Response>(&mut input) {
        Ok(Some(Response::Hello { protocol_version })) => {
            if let Err(error) = check_version(protocol_version) {
                fail(shared, generation, &error.to_string());
                return;
            }
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
                match state.ledger.accept(response) {
                    Accepted::Event(event) => {
                        if let Event::Opened {
                            page_count,
                            page_sizes,
                            repaired,
                        } = &event
                        {
                            state.opened = Some(OpenedInfo {
                                page_count: *page_count,
                                page_sizes: page_sizes.clone(),
                                repaired: *repaired,
                            });
                        }
                        state.settle_tile(&event);
                        shared.push(&mut state, event);
                    }
                    Accepted::Dropped => {}
                    Accepted::Violation => break End::Violation,
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
        state.process.take()
    };
    let Some(mut process) = process else { return };
    if end == End::Violation {
        process.kill();
    }
    let crash = process.crash(CRASH_GRACE);
    drop(process);
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
