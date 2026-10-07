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
use vellora_engine_client::bridge::{self, EventKind, TilePriority};
use vellora_engine_client::{Client, ClientConfig, ClientError, Event, TileRequest};
use vellora_ipc::{PageSize, Priority, RequestId, SlotId, TileRect};
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
    config
}

/// The golden document in a directory of its own, which lives as long as the returned guard.
fn golden_file() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("golden.pdf");
    fs::write(&path, GOLDEN_PDF).unwrap();
    (dir, path)
}

/// Collects events until `done` accepts one; returns everything seen, the accepted event last.
fn wait_for(client: &Client, mut done: impl FnMut(&Event) -> bool) -> Vec<Event> {
    let deadline = Instant::now() + PATIENCE;
    let mut seen = Vec::new();
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .unwrap_or_else(|| panic!("no matching event within {PATIENCE:?}; saw {seen:?}"));
        for event in client.wait_events(left) {
            let hit = done(&event);
            seen.push(event);
            if hit {
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
            repaired: false,
            ..
        })
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
    assert_eq!((opened.page_count, opened.repaired), (3, false));
    assert_eq!(handle.page_count(), 3);
    assert_eq!(
        (handle.page_size(1).width, handle.page_size(1).height),
        (200.0, 160.0)
    );
    assert_eq!(handle.page_size(99).width, 0.0);

    let request = handle
        .request_tile(0, 1.0, 0, 0, 64, 64, 2, TilePriority::Prefetch)
        .unwrap();
    let ready = loop {
        assert!(Instant::now() < deadline, "no tile");
        std::thread::sleep(Duration::from_millis(10));
        let events = handle.poll_events();
        if let Some(event) = events.into_iter().find(|e| e.kind == EventKind::TileReady) {
            break event;
        }
    };
    assert_eq!(
        (ready.request, ready.slot, ready.has_request),
        (request, 2, true)
    );
    // Answered requests are no longer in flight.
    assert!(!handle.cancel(request));

    // The bridge reports invalid requests as errors (C++ exceptions), not as panics.
    let error = handle
        .request_tile(0, 0.0, 0, 0, 64, 64, 0, TilePriority::Visible)
        .unwrap_err();
    assert!(error.to_string().contains("scale"), "{error}");

    handle.close();
    handle.close();
    assert!(matches!(
        handle.request_tile(0, 1.0, 0, 0, 8, 8, 0, TilePriority::Visible),
        Err(ClientError::Closed)
    ));
}
