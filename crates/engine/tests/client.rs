//! The engine client against the real engine executable: open, render, survive a crash, render
//! again, and the same through the bridge's Rust entry points.
//!
//! Lives in this package because only here the engine binary is built for the test
//! (`CARGO_BIN_EXE_vellora-engine`), like `tests/limits.rs`. Needs the PDFium build from
//! `cargo xtask pdfium fetch` and fails, never skips, without it.

// Test helpers: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::path::PathBuf;
use std::process::Command;
use std::time::{Duration, Instant};

use support::{GOLDEN_PDF, bgrx_to_rgb, difference, golden_image, pdfium_path};
use vellora_engine_client::bridge::{self, EventKind, TilePriority, TileState};
use vellora_engine_client::{
    Client, ClientConfig, ClientError, Event, ScaleBucket, TILE_PIXELS, TileKey, TileLookup,
    TileRequest,
};
use vellora_ipc::{ErrorKind, PageSize, Priority, RequestId, SlotId, TileRect};
use vellora_shm::SlotGeometry;

/// Same tolerance as the render crate's golden test.
const MEAN_LIMIT: f64 = 0.5;
const LOUD_LIMIT: f64 = 0.005;

/// How long a test waits for an event before calling the client hung.
const PATIENCE: Duration = Duration::from_secs(60);

fn config(max_restarts: u32) -> ClientConfig {
    let mut config = ClientConfig::new(
        env!("CARGO_BIN_EXE_vellora-engine"),
        SlotGeometry::new(4, 1 << 20).unwrap(),
    );
    config
        .env
        .push((vellora_render::LIBRARY_ENV.into(), pdfium_path().into()));
    config.max_restarts = max_restarts;
    // Thumbnails have tests of their own; the others need not map a region for them.
    config.thumbnail_budget_bytes = 0;
    config
}

/// The golden document in a directory of its own, which lives as long as the returned guard.
fn golden_file() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("golden.pdf");
    fs::write(&path, GOLDEN_PDF).unwrap();
    (dir, path)
}

thread_local! {
    /// Events taken from the client but not yet looked at. A batch can hold several events that
    /// a test waits for one after the other (`EngineRestarted`, then `Opened` of the new engine):
    /// the ones after the match belong to the next call, not to the bin.
    static LEFTOVER: std::cell::RefCell<std::collections::VecDeque<Event>> =
        const { std::cell::RefCell::new(std::collections::VecDeque::new()) };
}

/// Collects events until `done` accepts one; returns everything seen, the accepted event last. The
/// events after it in the same batch stay for the next call.
fn wait_for(client: &Client, mut done: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    loop {
        while let Some(event) = LEFTOVER.with(|left| left.borrow_mut().pop_front()) {
            let hit = done(&event);
            seen.push(event);
            if hit {
                return seen;
            }
        }
        let left = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("no matching event within {PATIENCE:?}; saw {seen:?}"));
        let mut batch = client.wait_events(left).into_iter();
        while let Some(event) = batch.next() {
            let hit = done(&event);
            seen.push(event);
            if hit {
                LEFTOVER.with(|left| left.borrow_mut().extend(batch));
                return seen;
            }
        }
    }
}
/// Renders page `page` of the golden document at 1.5x into `slot` and checks it against the
/// committed image.
fn render_and_check(client: &Client, page: u32, slot: u32) -> RequestId {
    let size = client
        .page_size(page)
        .expect("page size known after Opened");
    let scale = 1.5_f32;
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    let (width, height) = (
        (size.width * scale).ceil() as u32,
        (size.height * scale).ceil() as u32,
    );
    let request = client
        .request_tile(&TileRequest {
            page,
            scale,
            rect: TileRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            slot: SlotId(slot),
            priority: Priority::Visible,
        })
        .unwrap();
    wait_for(
        client,
        |event| matches!(event, Event::TileReady { request: r, slot: s } if *r == request && *s == SlotId(slot)),
    );

    let mut pixels = vec![0; client.geometry().slot_bytes() as usize];
    client.read_slot(SlotId(slot), &mut pixels).unwrap();
    let used = width as usize * height as usize * 4;
    let actual = bgrx_to_rgb(&pixels[..used]);
    let (golden_width, golden_height, golden) = golden_image(page as usize);
    assert_eq!(
        (width, height),
        (golden_width, golden_height),
        "page {page}"
    );
    let (mean, loud) = difference(&actual, &golden);
    assert!(mean <= MEAN_LIMIT, "page {page}: mean error {mean}");
    assert!(loud <= LOUD_LIMIT, "page {page}: {loud} of pixels are off");
    request
}

/// Ends a process the way a crash would, from outside.
fn kill(pid: u32) {
    let status = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .output()
            .unwrap()
            .status
    } else {
        Command::new("kill")
            .args(["-9", &pid.to_string()])
            .status()
            .unwrap()
    };
    assert!(status.success(), "could not kill the engine (pid {pid})");
}

#[test]
fn open_render_crash_restart_render() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(3), &path).unwrap();

    // Opening is asynchronous: nothing is known until the event arrives.
    assert_eq!(client.page_count(), None);
    let seen = wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    assert!(matches!(
        seen.last(),
        Some(Event::Opened {
            page_count: 3,
            repairs,
            ..
        }) if repairs.is_empty()
    ));
    assert_eq!(client.page_count(), Some(3));
    assert_eq!(
        client.page_size(0),
        Some(PageSize {
            width: 220.0,
            height: 140.0
        })
    );
    assert_eq!(client.page_size(3), None);

    render_and_check(&client, 0, 0);

    // The engine dies under the client.
    let first_engine = client.engine_id().expect("an engine is running");
    kill(first_engine);
    let seen = wait_for(&client, |event| matches!(event, Event::EngineRestarted));
    let [
        Event::EngineCrashed {
            crash,
            lost,
            will_restart: true,
        },
        Event::EngineRestarted,
    ] = seen.as_slice()
    else {
        panic!("expected a crash followed by a restart, got {seen:?}");
    };
    assert!(lost.is_empty(), "nothing was in flight");
    assert_ne!(
        crash.termination,
        vellora_engine_client::Termination::Success
    );

    // A new process opens the same document again, and renders.
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let second_engine = client.engine_id().expect("the new engine is running");
    assert_ne!(first_engine, second_engine);
    render_and_check(&client, 2, 1);
    // Slot 0 is reusable after the crash.
    render_and_check(&client, 0, 0);

    client.close();
    assert!(matches!(
        client.request_tile(&TileRequest {
            page: 0,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 8,
                height: 8
            },
            slot: SlotId(0),
            priority: Priority::Visible,
        }),
        Err(ClientError::Closed)
    ));
}

#[test]
fn the_client_gives_up_when_it_may_not_restart() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(0), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));

    kill(client.engine_id().unwrap());
    let seen = wait_for(&client, |event| {
        matches!(event, Event::EngineCrashed { .. })
    });
    assert!(matches!(
        seen.last(),
        Some(Event::EngineCrashed {
            will_restart: false,
            ..
        })
    ));

    let request = TileRequest {
        page: 0,
        scale: 1.0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 8,
            height: 8,
        },
        slot: SlotId(0),
        priority: Priority::Visible,
    };
    assert!(matches!(
        client.request_tile(&request),
        Err(ClientError::EngineUnavailable)
    ));
    assert!(client.engine_id().is_none());
}

#[test]
fn a_document_the_engine_cannot_open_is_reported_not_fatal() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("not-a-pdf.pdf");
    fs::write(&path, b"this is not a PDF").unwrap();
    let client = Client::open(config(3), &path).unwrap();

    let seen = wait_for(&client, |event| {
        matches!(event, Event::RequestFailed { .. })
    });
    assert!(
        matches!(
            seen.last(),
            Some(Event::RequestFailed {
                request: None,
                kind: vellora_ipc::ErrorKind::OpenFailed,
                ..
            })
        ),
        "{seen:?}"
    );
    assert_eq!(client.page_count(), None);
}

#[test]
fn an_invalid_request_is_refused_before_it_is_sent() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));

    let too_big = TileRequest {
        page: 0,
        scale: 1.0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: 5000,
            height: 10,
        },
        slot: SlotId(0),
        priority: Priority::Visible,
    };
    assert!(matches!(
        client.request_tile(&too_big),
        Err(ClientError::Protocol(_))
    ));
    // Nothing was in flight, so a crash would have nothing to report as lost.
    assert!(!client.cancel(RequestId(1)));
}

#[test]
fn the_bridge_entry_points_drive_the_engine() {
    let (_dir, path) = golden_file();
    let mut handle = bridge::open_with(config(3), &path).unwrap();

    let deadline = Instant::now() + PATIENCE;
    let mut events = Vec::new();
    while !events
        .iter()
        .any(|e: &bridge::EngineEvent| e.kind == EventKind::Opened)
    {
        assert!(Instant::now() < deadline, "never opened");
        std::thread::sleep(Duration::from_millis(10));
        events.extend(handle.poll_events());
    }
    let opened = events.iter().find(|e| e.kind == EventKind::Opened).unwrap();
    assert_eq!(opened.page_count, 3);
    assert_eq!(opened.repairs, Vec::<bridge::RepairNote>::new());
    assert_eq!(handle.page_count(), 3);
    assert_eq!(
        (handle.page_size(1).width, handle.page_size(1).height),
        (200.0, 160.0)
    );
    assert_eq!(handle.page_size(99).width, 0.0);

    assert_ne!(handle.engine_id(), 0, "an engine is running");
    assert_eq!(
        handle.engine_id(),
        handle.client().unwrap().engine_id().unwrap()
    );
    assert_eq!(bridge::tile_pixels(), 512);
    assert_eq!(bridge::bucket_scale(1.0), 1.0);
    assert_eq!(bridge::bucket_scale(0.0), 0.0);
    let ticket = handle
        .request_tile(0, 1.0, 0, 0, TilePriority::Prefetch)
        .unwrap();
    assert_eq!(ticket.state, TileState::Requested);
    let request = ticket.request;
    let mut tile = vec![0; handle.slot_bytes() as usize];
    assert!(
        !handle.read_tile(0, 1.0, 0, 0, &mut tile).unwrap(),
        "not readable before TileReady"
    );
    let ready = loop {
        assert!(Instant::now() < deadline, "no tile");
        std::thread::sleep(Duration::from_millis(10));
        let events = handle.poll_events();
        if let Some(event) = events.into_iter().find(|e| e.kind == EventKind::TileReady) {
            break event;
        }
    };
    assert_eq!((ready.request, ready.has_request), (request, true));
    // Answered requests are no longer in flight.
    assert!(!handle.cancel(request));

    // The finished tile is served from the cache: no new request, and its pixels are the page.
    assert!(handle.read_tile(0, 1.0, 0, 0, &mut tile).unwrap());
    assert!(
        tile.iter().any(|&b| b != 0xFF),
        "page 0 has content, not just white"
    );
    assert_eq!(
        handle
            .request_tile(0, 1.0, 0, 0, TilePriority::Visible)
            .unwrap()
            .state,
        TileState::Ready
    );
    handle.invalidate_page(0);
    assert!(!handle.read_tile(0, 1.0, 0, 0, &mut tile).unwrap());

    // The bridge reports invalid requests as errors (C++ exceptions), not as panics.
    let error = handle
        .request_tile(0, 0.0, 0, 0, TilePriority::Visible)
        .unwrap_err();
    assert!(error.to_string().contains("invalid tile"), "{error}");
    let mut short = [0_u8; 16];
    assert!(
        handle
            .read_tile(0, 1.0, 0, 0, &mut short)
            .is_ok_and(|hit| !hit)
    );

    handle.close();
    handle.close();
    assert!(matches!(
        handle.request_tile(0, 1.0, 0, 0, TilePriority::Visible),
        Err(ClientError::Closed)
    ));
}

/// The cache key of the first tile of `page` at scale 1.
fn key(page: u32) -> TileKey {
    TileKey {
        page,
        scale: ScaleBucket::from_scale(1.0).unwrap(),
        x: 0,
        y: 0,
    }
}

/// Requests a cached tile that must be new, waits for it and returns its pixels.
fn fetch(client: &Client, key: TileKey) -> Vec<u8> {
    let TileLookup::Requested(request) =
        client.request_cached_tile(key, Priority::Visible).unwrap()
    else {
        panic!("expected a new request for {key:?}");
    };
    wait_for(
        client,
        |event| matches!(event, Event::TileReady { request: r, .. } if *r == request),
    );
    let mut pixels = vec![0; client.geometry().slot_bytes() as usize];
    assert!(client.read_tile(&key, &mut pixels).unwrap());
    pixels
}

/// Renders the same tile without the cache, for reference (slot 3, before the cache is used).
fn reference_tile(client: &Client, page: u32) -> Vec<u8> {
    let request = client
        .request_tile(&TileRequest {
            page,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: TILE_PIXELS,
                height: TILE_PIXELS,
            },
            slot: SlotId(3),
            priority: Priority::Visible,
        })
        .unwrap();
    wait_for(
        client,
        |event| matches!(event, Event::TileReady { request: r, .. } if *r == request),
    );
    let mut pixels = vec![0; client.geometry().slot_bytes() as usize];
    client.read_slot(SlotId(3), &mut pixels).unwrap();
    pixels
}

#[test]
fn a_cached_tile_is_rendered_once_and_matches_a_direct_render() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let reference = reference_tile(&client, 0);

    let first = fetch(&client, key(0));
    assert_eq!(first, reference, "a cached tile is the plain render");
    assert!(first.iter().any(|&b| b != 0xFF), "the page has content");
    assert_eq!(client.cached_tiles(), 1);

    // A hit sends nothing: no request, hence no event, and the same pixels.
    assert_eq!(
        client
            .request_cached_tile(key(0), Priority::Visible)
            .unwrap(),
        TileLookup::Ready
    );
    let quiet = client.wait_events(Duration::from_millis(300));
    assert_eq!(quiet.len(), 0, "{quiet:?}");
    let mut again = vec![0; client.geometry().slot_bytes() as usize];
    assert!(client.read_tile(&key(0), &mut again).unwrap());
    assert_eq!(again, first);

    // Another tile is another entry; a short buffer is refused rather than half-filled.
    fetch(&client, key(1));
    assert_eq!(client.cached_tiles(), 2);
    assert!(client.read_tile(&key(1), &mut [0; 8]).is_err());

    // Invalidation forgets the page's tiles and the next request goes to the engine again.
    assert_eq!(client.invalidate_page(0), 1);
    assert!(!client.read_tile(&key(0), &mut again).unwrap());
    assert!(matches!(
        client
            .request_cached_tile(key(0), Priority::Visible)
            .unwrap(),
        TileLookup::Requested(_)
    ));
}

#[test]
fn a_request_in_flight_is_not_asked_for_twice_and_cancel_frees_its_entry() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));

    let TileLookup::Requested(id) = client
        .request_cached_tile(key(0), Priority::Visible)
        .unwrap()
    else {
        panic!("first request must be sent");
    };
    // Either the engine is still busy (in flight) or it has answered already (ready).
    assert!(matches!(
        client
            .request_cached_tile(key(0), Priority::Visible)
            .unwrap(),
        TileLookup::InFlight | TileLookup::Ready
    ));
    assert_eq!(client.cached_tiles(), 1);

    // If the cancel got there first the entry is gone; otherwise the tile had been answered.
    if client.cancel(id) {
        assert_eq!(client.cached_tiles(), 0);
        assert!(
            !client
                .read_tile(
                    &key(0),
                    &mut vec![0; client.geometry().slot_bytes() as usize]
                )
                .unwrap()
        );
    } else {
        assert_eq!(client.cached_tiles(), 1);
    }
}

#[test]
fn finished_tiles_survive_a_crash_and_requests_in_flight_do_not() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let first = fetch(&client, key(0));

    // Two more requests, then the engine dies.
    for page in [1, 2] {
        assert!(matches!(
            client
                .request_cached_tile(key(page), Priority::Visible)
                .unwrap(),
            TileLookup::Requested(_)
        ));
    }
    kill(client.engine_id().unwrap());
    let seen = wait_for(&client, |event| matches!(event, Event::EngineRestarted));
    let answered = seen
        .iter()
        .filter(|event| matches!(event, Event::TileReady { .. }))
        .count();
    let lost = seen
        .iter()
        .find_map(|event| match event {
            Event::EngineCrashed { lost, .. } => Some(lost.len()),
            _ => None,
        })
        .expect("a crash was reported");
    // Every request was either answered before the crash or lost in it; only the answered ones stay.
    assert_eq!(answered + lost, 2, "{seen:?}");
    assert_eq!(client.cached_tiles(), 1 + answered);

    // The tile that was ready is still readable and identical; a lost one can be asked for again.
    let mut pixels = vec![0; client.geometry().slot_bytes() as usize];
    assert!(client.read_tile(&key(0), &mut pixels).unwrap());
    assert_eq!(pixels, first);
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    for page in [1, 2] {
        match client
            .request_cached_tile(key(page), Priority::Visible)
            .unwrap()
        {
            TileLookup::Requested(_) | TileLookup::Ready => {}
            other => panic!("page {page}: {other:?}"),
        }
    }
}

// ---- M1 task 11: thumbnails ----

/// The size of the thumbnail of `page` at `scale`: the whole page, rounded up.
fn thumbnail_size(client: &Client, page: u32, scale: ScaleBucket) -> (u32, u32) {
    let size = client.page_size(page).expect("page size known");
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    (
        (size.width * scale.scale()).ceil() as u32,
        (size.height * scale.scale()).ceil() as u32,
    )
}

/// Requests a thumbnail that must be new, waits for it and returns its pixels.
fn fetch_thumbnail(client: &Client, page: u32, scale: ScaleBucket) -> Vec<u8> {
    let (width, height) = thumbnail_size(client, page, scale);
    let TileLookup::Requested(request) = client
        .request_thumbnail(page, scale, width, height)
        .unwrap()
    else {
        panic!("expected a new request for the thumbnail of page {page}");
    };
    wait_for(
        client,
        |event| matches!(event, Event::TileReady { request: r, .. } if *r == request),
    );
    let mut pixels = vec![0; width as usize * height as usize * 4];
    assert!(client.read_thumbnail(page, scale, &mut pixels).unwrap());
    pixels
}

#[test]
fn thumbnails_are_cached_apart_from_tiles_within_their_own_budget() {
    let (_dir, path) = golden_file();
    let mut config = config(3);
    // Two thumbnail slots: a third thumbnail pushes the oldest out.
    config.thumbnail_budget_bytes = 2 << 20;
    let client = Client::open(config, &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let scale = ScaleBucket::from_scale(0.5).unwrap();

    // A thumbnail is the plain render of the whole page at that scale.
    let thumbnail = fetch_thumbnail(&client, 0, scale);
    let (width, height) = thumbnail_size(&client, 0, scale);
    let request = client
        .request_tile(&TileRequest {
            page: 0,
            scale: scale.scale(),
            rect: TileRect {
                x: 0,
                y: 0,
                width,
                height,
            },
            slot: SlotId(3),
            priority: Priority::Visible,
        })
        .unwrap();
    wait_for(
        &client,
        |event| matches!(event, Event::TileReady { request: r, .. } if *r == request),
    );
    let mut direct = vec![0; client.geometry().slot_bytes() as usize];
    client.read_slot(SlotId(3), &mut direct).unwrap();
    assert_eq!(thumbnail, direct[..thumbnail.len()]);
    assert!(thumbnail.iter().any(|&b| b != 0xFF), "the page has content");
    assert_eq!(client.cached_tiles(), 0, "a thumbnail is not a tile");
    assert_eq!(client.cached_thumbnails(), 1);

    // A hit sends nothing.
    assert_eq!(
        client.request_thumbnail(0, scale, width, height).unwrap(),
        TileLookup::Ready
    );

    // Tiles fill their own cache untouched by thumbnails, and thumbnails evict only thumbnails.
    fetch(&client, key(0));
    fetch_thumbnail(&client, 1, scale);
    fetch_thumbnail(&client, 2, scale);
    assert_eq!(client.cached_thumbnails(), 2);
    assert_eq!(client.cached_tiles(), 1);
    let mut tile = vec![0; client.geometry().slot_bytes() as usize];
    assert!(
        client.read_tile(&key(0), &mut tile).unwrap(),
        "the tile survived"
    );
    let mut gone = vec![0; width as usize * height as usize * 4];
    assert!(
        !client.read_thumbnail(0, scale, &mut gone).unwrap(),
        "the oldest thumbnail was evicted"
    );

    // Invalidating a page drops its thumbnail as well as its tiles.
    assert_eq!(client.invalidate_page(2), 1);
    assert_eq!(client.cached_thumbnails(), 1);
}

#[test]
fn a_thumbnail_the_cache_cannot_hold_is_refused() {
    let (_dir, path) = golden_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let scale = ScaleBucket::from_scale(0.5).unwrap();
    // Thumbnails are off in this configuration (no budget).
    assert!(matches!(
        client.request_thumbnail(0, scale, 100, 70),
        Err(ClientError::InvalidTile)
    ));

    let mut on = self::config(3);
    on.thumbnail_budget_bytes = 1 << 20;
    let (_dir2, path2) = golden_file();
    let client = Client::open(on, &path2).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    for (width, height) in [
        (0, 10),
        (10, 0),
        (TILE_PIXELS + 1, 10),
        (10, TILE_PIXELS + 1),
    ] {
        assert!(
            matches!(
                client.request_thumbnail(0, scale, width, height),
                Err(ClientError::InvalidTile)
            ),
            "{width}x{height}"
        );
    }
    assert_eq!(client.cached_thumbnails(), 0);
}

// ---- M1 task 7: encrypted documents ----

/// A one-page document with the user password `user-pw` (qpdf-made, from the `cos` fixtures).
fn protected_file() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("protected.pdf");
    let source = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../cos/tests/fixtures/encryption/r6-aes-256-user-password.pdf");
    fs::copy(source, &path).unwrap();
    (dir, path)
}

/// The refusal that answers an `Open` that needs a password.
fn is_refusal(event: &Event, expected: ErrorKind) -> bool {
    matches!(event, Event::RequestFailed { request: None, kind, .. } if *kind == expected)
}

#[test]
fn an_encrypted_document_asks_for_its_password_and_opens_with_it() {
    let (_dir, path) = protected_file();
    let client = Client::open(config(0), &path).unwrap();

    wait_for(&client, |e| is_refusal(e, ErrorKind::PasswordRequired));
    assert_eq!(
        client.page_count(),
        None,
        "nothing is shown before the password"
    );
    // Three wrong attempts in a row: each is refused and the engine stays up.
    for _ in 0..3 {
        client.submit_password("wrong").unwrap();
        wait_for(&client, |e| is_refusal(e, ErrorKind::WrongPassword));
        assert_eq!(client.page_count(), None);
    }
    // Text the protocol cannot carry is refused before it is sent, and changes nothing.
    assert!(matches!(
        client.submit_password("nul\0inside"),
        Err(ClientError::Protocol(_))
    ));
    client.submit_password("user-pw").unwrap();
    let seen = wait_for(&client, |e| matches!(e, Event::Opened { .. }));
    assert!(matches!(
        seen.last(),
        Some(Event::Opened { page_count: 1, repairs, .. }) if repairs.is_empty()
    ));
    assert_eq!(client.page_count(), Some(1));
    // It renders, and a second password for the open document is refused client-side.
    let request = client
        .request_tile(&TileRequest {
            page: 0,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 50,
                height: 50,
            },
            slot: SlotId(0),
            priority: Priority::Visible,
        })
        .unwrap();
    wait_for(
        &client,
        |e| matches!(e, Event::TileReady { request: r, .. } if *r == request),
    );
    assert!(matches!(
        client.submit_password("user-pw"),
        Err(ClientError::AlreadyOpen)
    ));
}

#[test]
fn an_accepted_password_is_sent_again_to_a_restarted_engine() {
    let (_dir, path) = protected_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |e| is_refusal(e, ErrorKind::PasswordRequired));
    client.submit_password("user-pw").unwrap();
    wait_for(&client, |e| matches!(e, Event::Opened { .. }));

    kill(client.engine_id().expect("an engine is running"));
    // The new engine opens the document by itself: no refusal comes in between.
    let seen = wait_for(&client, |e| matches!(e, Event::Opened { .. }));
    assert!(
        seen.iter()
            .all(|e| !matches!(e, Event::RequestFailed { .. })),
        "the user must not be asked again: {seen:?}"
    );
    assert!(seen.iter().any(|e| matches!(e, Event::EngineRestarted)));
    assert_eq!(client.page_count(), Some(1));
}

#[test]
fn a_refused_password_is_not_kept_for_a_restarted_engine() {
    let (_dir, path) = protected_file();
    let client = Client::open(config(3), &path).unwrap();
    wait_for(&client, |e| is_refusal(e, ErrorKind::PasswordRequired));
    client.submit_password("wrong").unwrap();
    wait_for(&client, |e| is_refusal(e, ErrorKind::WrongPassword));

    kill(client.engine_id().expect("an engine is running"));
    // Asked from scratch (`PasswordRequired`), not answered with the refused password again
    // (`WrongPassword`).
    let seen = wait_for(&client, |e| matches!(e, Event::RequestFailed { .. }));
    assert!(
        is_refusal(seen.last().unwrap(), ErrorKind::PasswordRequired),
        "{seen:?}"
    );
    client.submit_password("user-pw").unwrap();
    wait_for(&client, |e| matches!(e, Event::Opened { .. }));
}

#[test]
fn the_bridge_carries_the_password_answers() {
    let (_dir, path) = protected_file();
    let mut handle = bridge::open_with(config(0), &path).unwrap();
    let next = |wanted: &dyn Fn(&bridge::EngineEvent) -> bool| {
        let deadline = Instant::now() + PATIENCE;
        loop {
            assert!(Instant::now() < deadline, "no matching event");
            if let Some(event) = handle.poll_events().into_iter().find(|e| wanted(e)) {
                return event;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
    };
    let failed = |kind: bridge::FailureKind| {
        move |e: &bridge::EngineEvent| {
            e.kind == EventKind::RequestFailed && !e.has_request && e.failure == kind
        }
    };
    next(&failed(bridge::FailureKind::PasswordRequired));
    handle.submit_password("wrong").unwrap();
    next(&failed(bridge::FailureKind::WrongPassword));
    handle.submit_password("user-pw").unwrap();
    let opened = next(&|e| e.kind == EventKind::Opened);
    assert_eq!(opened.page_count, 1);
    handle.close();
    assert!(matches!(
        handle.submit_password("user-pw"),
        Err(ClientError::Closed)
    ));
}
