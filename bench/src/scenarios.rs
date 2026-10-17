//! Reading scenarios, driven through the real engine process the way the shell drives it:
//! scrolling in each view mode, a thumbnail fling, a search over the whole document and printing.
//!
//! Every run starts a fresh engine, so nothing is cached from an earlier run, and the clock starts
//! when the document is open. Each scenario returns one throughput (work done per second of wall
//! time, requests and waiting included); `measure` takes the median over the runs.
//!
//! # The scroll scenarios
//!
//! They ask for the 512 x 512 tiles at scale 1.0 that a viewer shows after each scroll step and
//! does not already have (a set of keys stands for the tile cache), and wait for all of them
//! before the next step, which is the worst case for a reader who scrolls as fast as the screen
//! updates. The step latency is the time from the first request of a step to its last tile.
//!
//! - `single`: one page fills the window; a step turns the page (4 tiles of a letter page).
//! - `facing`: two pages side by side; a step turns both (8 tiles).
//! - `continuous`: pages stacked with a gap in an 800 x 900 window; a step scrolls 300 px.

// Page and tile arithmetic on small, bounded numbers (a few thousand pixels, a few hundred pages).
#![allow(
    clippy::cast_possible_truncation,
    clippy::cast_precision_loss,
    clippy::cast_sign_loss
)]

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::time::{Duration, Instant};

use vellora_engine_client::{Client, ClientConfig, Event, ScaleBucket, TileLookup, TileRequest};
use vellora_ipc::{Priority, RequestId, SearchOutcome, SearchQuery, SlotId, TileRect};
use vellora_shm::SlotGeometry;

use crate::Result;
use crate::measure::Setup;

/// How long to wait for the engine before calling a scenario failed.
const PATIENCE: Duration = Duration::from_secs(300);
/// Side of a tile in pixels, the shell's `TILE_PIXELS`.
const TILE: u32 = 512;
/// Slots of the tile region: more than the most tiles a step asks for.
const TILE_SLOTS: u32 = 16;
/// Bytes of a print band slot.
const PRINT_SLOT_BYTES: u32 = 4 << 20;
/// Print resolution in dots per inch.
const PRINT_DPI: f32 = 150.0;
/// The window of the continuous scroll, and the page gap and step in pixels.
const WINDOW: (f32, f32) = (800.0, 900.0);
const GAP: f32 = 10.0;
const SCROLL_STEP: f32 = 300.0;
/// Thumbnails are asked for at this scale (a letter page becomes 123 x 159 pixels).
const THUMBNAIL_SCALE: f32 = 0.2;

/// A view mode of the scroll scenarios.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Mode {
    Single,
    Facing,
    Continuous,
}

impl Mode {
    pub(crate) const ALL: [Self; 3] = [Self::Single, Self::Facing, Self::Continuous];

    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::Single => "scroll-single",
            Self::Facing => "scroll-facing",
            Self::Continuous => "scroll-continuous",
        }
    }
}

/// What one run of a scenario measured.
pub(crate) struct Outcome {
    /// Units of work done (tiles, thumbnails, pages).
    pub(crate) work: f64,
    pub(crate) elapsed: Duration,
    /// Time of each step, for the scroll scenarios.
    pub(crate) steps: Vec<Duration>,
}

impl Outcome {
    pub(crate) fn per_second(&self) -> f64 {
        self.work / self.elapsed.as_secs_f64()
    }
}

/// A running engine with a document open.
struct Session {
    client: Client,
    pages: u32,
    page: (f32, f32),
}

impl Session {
    fn open(setup: &Setup, path: &Path, geometry: SlotGeometry, print_slots: u32) -> Result<Self> {
        let mut config: ClientConfig = setup.config(geometry);
        config.print_slots = print_slots;
        let client = Client::open(config, path)?;
        let deadline = Instant::now() + PATIENCE;
        loop {
            let left = deadline
                .checked_duration_since(Instant::now())
                .ok_or("the engine did not open the document in time")?;
            for event in client.wait_events(left) {
                match event {
                    Event::Opened { page_count, .. } => {
                        let size = client
                            .page_size(0)
                            .ok_or("the engine sent no size for page 1")?;
                        return Ok(Self {
                            client,
                            pages: page_count,
                            page: (size.width, size.height),
                        });
                    }
                    Event::Failed { reason } => return Err(reason.into()),
                    Event::EngineCrashed { crash, .. } => {
                        return Err(format!("the engine crashed: {crash:?}").into());
                    }
                    _ => {}
                }
            }
        }
    }

    /// Waits for the next events and takes the ids among `pending` that finished out of it,
    /// calling `on_ready` with each.
    fn wait_some(
        &self,
        pending: &mut HashSet<RequestId>,
        mut on_ready: impl FnMut(RequestId) -> Result<()>,
    ) -> Result<()> {
        for event in self.client.wait_events(PATIENCE) {
            match event {
                Event::TileReady { request, .. } if pending.remove(&request) => {
                    on_ready(request)?;
                }
                Event::RequestFailed { message, .. } => {
                    return Err(format!("the engine reported: {message}").into());
                }
                Event::EngineCrashed { crash, .. } => {
                    return Err(format!("the engine crashed: {crash:?}").into());
                }
                Event::Failed { reason } => return Err(reason.into()),
                _ => {}
            }
        }
        Ok(())
    }

    /// Waits until `pending` is empty.
    fn drain(&self, pending: &mut HashSet<RequestId>) -> Result<()> {
        let deadline = Instant::now() + PATIENCE;
        while !pending.is_empty() {
            if Instant::now() >= deadline {
                return Err("the engine did not answer in time".into());
            }
            self.wait_some(pending, |_| Ok(()))?;
        }
        Ok(())
    }
}

fn tile_geometry() -> Result<SlotGeometry> {
    Ok(SlotGeometry::new(TILE_SLOTS, TILE * TILE * 4)?)
}

/// The tiles of one scroll step: `(page, column, row)`.
type Tile = (u32, u32, u32);

fn step_tiles(mode: Mode, step: u32, pages: u32, page: (f32, f32)) -> Vec<Tile> {
    let columns = |width: f32| (width / TILE as f32).ceil() as u32;
    let rows = |height: f32| (height / TILE as f32).ceil() as u32;
    let whole_page = |index: u32| -> Vec<Tile> {
        (0..rows(page.1))
            .flat_map(|row| (0..columns(page.0)).map(move |column| (index, column, row)))
            .collect()
    };
    match mode {
        Mode::Single => whole_page(step % pages),
        Mode::Facing => {
            let mut tiles = whole_page((2 * step) % pages);
            tiles.extend(whole_page((2 * step + 1) % pages));
            tiles
        }
        Mode::Continuous => {
            let pitch = page.1 + GAP;
            let top = step as f32 * SCROLL_STEP;
            let bottom = top + WINDOW.1;
            let first = (top / pitch).floor() as u32;
            let last = (bottom / pitch).floor() as u32;
            let mut tiles = Vec::new();
            for index in first..=last.min(pages.saturating_sub(1)) {
                let page_top = index as f32 * pitch;
                let from = (top - page_top).max(0.0);
                let to = (bottom - page_top).min(page.1);
                if to <= from {
                    continue;
                }
                for row in (from / TILE as f32).floor() as u32..rows(to) {
                    for column in 0..columns(WINDOW.0.min(page.0)) {
                        tiles.push((index, column, row));
                    }
                }
            }
            tiles
        }
    }
}

/// Scrolls through `steps` steps of `mode`.
pub(crate) fn scroll(setup: &Setup, path: &Path, mode: Mode, steps: u32) -> Result<Outcome> {
    let session = Session::open(setup, path, tile_geometry()?, 0)?;
    let mut have: HashSet<Tile> = HashSet::new();
    let mut times = Vec::new();
    let mut tiles_done = 0_u32;
    let started = Instant::now();
    for step in 0..steps {
        let wanted: Vec<Tile> = step_tiles(mode, step, session.pages, session.page)
            .into_iter()
            .filter(|tile| have.insert(*tile))
            .collect();
        if wanted.is_empty() {
            continue;
        }
        let step_started = Instant::now();
        for chunk in wanted.chunks(TILE_SLOTS as usize) {
            let mut pending = HashSet::new();
            for (slot, &(page, column, row)) in chunk.iter().enumerate() {
                let id = session.client.request_tile(&TileRequest {
                    page,
                    scale: 1.0,
                    rect: TileRect {
                        x: column * TILE,
                        y: row * TILE,
                        width: TILE,
                        height: TILE,
                    },
                    slot: SlotId(slot as u32),
                    priority: Priority::Visible,
                })?;
                pending.insert(id);
            }
            session.drain(&mut pending)?;
        }
        times.push(step_started.elapsed());
        tiles_done += u32::try_from(wanted.len())?;
    }
    let elapsed = started.elapsed();
    session.client.close();
    Ok(Outcome {
        work: f64::from(tiles_done),
        elapsed,
        steps: times,
    })
}

/// Asks for the thumbnails of `count` pages as fast as the cache takes requests, the way a fling
/// down the thumbnail strip does, and waits for all of them.
pub(crate) fn thumbnails(setup: &Setup, path: &Path, count: u32) -> Result<Outcome> {
    let session = Session::open(setup, path, tile_geometry()?, 0)?;
    let scale = ScaleBucket::from_scale(THUMBNAIL_SCALE).ok_or("bad thumbnail scale")?;
    let width = (session.page.0 * scale.scale()).ceil() as u32;
    let height = (session.page.1 * scale.scale()).ceil() as u32;
    let total = count.min(session.pages);
    let mut pending = HashSet::new();
    let mut buffer = vec![0_u8; (width * height * 4) as usize];
    let started = Instant::now();
    let mut next = 0;
    while next < total {
        match session
            .client
            .request_thumbnail(next, scale, width, height)?
        {
            TileLookup::Requested(id) => {
                pending.insert(id);
                next += 1;
            }
            TileLookup::Ready | TileLookup::InFlight => next += 1,
            TileLookup::Full => session.wait_some(&mut pending, |_| Ok(()))?,
            _ => return Err("unexpected thumbnail lookup".into()),
        }
    }
    session.drain(&mut pending)?;
    let elapsed = started.elapsed();
    // A thumbnail that is blank measures nothing.
    let page = total - 1;
    if !session.client.read_thumbnail(page, scale, &mut buffer)? {
        return Err("the last thumbnail is not in the cache".into());
    }
    if buffer
        .as_chunks::<4>()
        .0
        .iter()
        .all(|px| px[..3] == [0xFF, 0xFF, 0xFF])
    {
        return Err("the last thumbnail is blank".into());
    }
    session.client.close();
    Ok(Outcome {
        work: f64::from(total),
        elapsed,
        steps: Vec::new(),
    })
}

/// Searches the whole document for `text` (case sensitive) and returns pages searched.
pub(crate) fn search(setup: &Setup, path: &Path, text: &str) -> Result<Outcome> {
    let session = Session::open(setup, path, tile_geometry()?, 0)?;
    let started = Instant::now();
    let id = session.client.request_search(&SearchQuery {
        text: text.into(),
        case_sensitive: true,
        whole_word: false,
        regex: false,
    })?;
    let deadline = started + PATIENCE;
    let (pages_done, hits) = loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("the search did not finish in time")?;
        let mut done = None;
        for event in session.client.wait_events(left) {
            match event {
                Event::SearchDone {
                    request,
                    outcome,
                    hits,
                    pages_done,
                } if request == id => {
                    if outcome != SearchOutcome::Finished {
                        return Err(format!("the search ended as {outcome:?}").into());
                    }
                    done = Some((pages_done, hits));
                }
                Event::RequestFailed { message, .. } => {
                    return Err(format!("the engine reported: {message}").into());
                }
                Event::EngineCrashed { crash, .. } => {
                    return Err(format!("the engine crashed: {crash:?}").into());
                }
                _ => {}
            }
        }
        if let Some(done) = done {
            break done;
        }
    };
    let elapsed = started.elapsed();
    if hits == 0 {
        return Err("the search found nothing, so it measured the wrong thing".into());
    }
    session.client.close();
    Ok(Outcome {
        work: f64::from(pages_done),
        elapsed,
        steps: Vec::new(),
    })
}

/// Prints `count` pages at 150 dpi: bands of the page as tall as a slot allows, four in flight,
/// each one copied out as the shell copies it into the `QPainter`.
pub(crate) fn print(setup: &Setup, path: &Path, count: u32) -> Result<Outcome> {
    let geometry = SlotGeometry::new(2, PRINT_SLOT_BYTES)?;
    let session = Session::open(setup, path, geometry, 4)?;
    let scale = PRINT_DPI / 72.0;
    let width = (session.page.0 * scale).ceil() as u32;
    let height = (session.page.1 * scale).ceil() as u32;
    let rows = (PRINT_SLOT_BYTES / 4 / width).clamp(1, height);
    let total = count.min(session.pages);
    let mut bands: Vec<(u32, TileRect)> = Vec::new();
    for page in 0..total {
        let mut y = 0;
        while y < height {
            let band = rows.min(height - y);
            bands.push((
                page,
                TileRect {
                    x: 0,
                    y,
                    width,
                    height: band,
                },
            ));
            y += band;
        }
    }
    let started = Instant::now();
    let mut next = 0;
    let mut pending: HashSet<RequestId> = HashSet::new();
    let mut sizes: HashMap<RequestId, usize> = HashMap::new();
    let mut finished = 0;
    let deadline = started + PATIENCE;
    let mut painted = false;
    while finished < bands.len() {
        if Instant::now() >= deadline {
            return Err("printing did not finish in time".into());
        }
        while let Some(&(page, rect)) = bands.get(next) {
            let Some(id) = session
                .client
                .request_print_band(page, scale, rect, false)?
            else {
                break;
            };
            pending.insert(id);
            sizes.insert(id, rect.width as usize * rect.height as usize * 4);
            next += 1;
        }
        session.wait_some(&mut pending, |id| {
            let mut buffer = vec![0_u8; sizes.remove(&id).ok_or("unknown band")?];
            if !session.client.read_print_band(id, &mut buffer)? {
                return Err("a finished band could not be read".into());
            }
            painted |= buffer
                .as_chunks::<4>()
                .0
                .iter()
                .any(|px| px[..3] != [0xFF, 0xFF, 0xFF]);
            finished += 1;
            Ok(())
        })?;
    }
    if !painted {
        return Err("every printed band was blank".into());
    }
    let elapsed = started.elapsed();
    session.client.close();
    Ok(Outcome {
        work: f64::from(total),
        elapsed,
        steps: Vec::new(),
    })
}
