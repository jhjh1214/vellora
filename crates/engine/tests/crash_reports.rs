//! M1 task 9c: what the UI records when the sandboxed engine fails. The engine cannot write files,
//! so the client writes a report with the last lines the engine printed.
//!
//! Debug builds only: the engine's `VELLORA_TEST_ENGINE_CRASH` hook is not compiled into release
//! builds.

#![cfg(debug_assertions)]
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use support::{GOLDEN_PDF, pdfium_path};
use vellora_engine_client::{Client, ClientConfig, Event, TileRequest};
use vellora_ipc::{ErrorKind, Priority, SlotId, TileRect};
use vellora_shm::SlotGeometry;

const PATIENCE: Duration = Duration::from_secs(60);

fn client(mode: &str, crash_dir: Option<&Path>) -> (tempfile::TempDir, Client) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("golden.pdf");
    fs::write(&path, GOLDEN_PDF).unwrap();
    let mut config = ClientConfig::new(
        env!("CARGO_BIN_EXE_vellora-engine"),
        SlotGeometry::new(2, 1 << 20).unwrap(),
    );
    config
        .env
        .push((vellora_render::LIBRARY_ENV.into(), pdfium_path().into()));
    config
        .env
        .push(("VELLORA_TEST_ENGINE_CRASH".into(), mode.into()));
    config.max_restarts = 0;
    config.crash_dir = crash_dir.map(Path::to_owned);
    let client = Client::open(config, &path).unwrap();
    (dir, client)
}

fn wait_for(client: &Client, mut done: impl FnMut(&Event) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("the expected event did not come");
        if client.wait_events(left).iter().any(&mut done) {
            return;
        }
    }
}

fn ask_for_a_tile(client: &Client) {
    wait_for(client, |e| matches!(e, Event::Opened { .. }));
    client
        .request_tile(&TileRequest {
            page: 0,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 20,
                height: 20,
            },
            slot: SlotId(0),
            priority: Priority::Visible,
        })
        .unwrap();
}

fn reports(dir: &Path) -> Vec<PathBuf> {
    let mut found: Vec<PathBuf> = fs::read_dir(dir)
        .map(|entries| {
            entries
                .map(|e| e.unwrap().path())
                .filter(|p| {
                    p.file_name()
                        .is_some_and(|n| n.to_string_lossy().starts_with("engine-crash-"))
                })
                .collect()
        })
        .unwrap_or_default();
    found.sort();
    found
}

/// Waits for a report to appear (it is written a moment after the event).
fn wait_for_report(dir: &Path) -> String {
    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(path) = reports(dir).first() {
            return fs::read_to_string(path).unwrap();
        }
        assert!(Instant::now() < deadline, "no report in {}", dir.display());
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn a_panic_the_engine_catches_is_recorded_with_its_log_tail() {
    let reports_dir = tempfile::tempdir().unwrap();
    let (_document, client) = client("panic", Some(reports_dir.path()));
    ask_for_a_tile(&client);
    wait_for(&client, |e| {
        matches!(
            e,
            Event::RequestFailed {
                kind: ErrorKind::Internal,
                ..
            }
        )
    });

    let text = wait_for_report(reports_dir.path());
    assert!(text.starts_with("Vellora engine report\n"), "{text}");
    assert!(
        text.contains("what: the engine reported an internal error"),
        "{text}"
    );
    assert!(text.contains("details: internal error"), "{text}");
    assert!(text.contains("version: "), "{text}");
    // The tail: the engine's own lines, among them what it printed when it panicked.
    assert!(text.contains("[engine "), "{text}");
    assert!(
        text.contains("forced engine panic"),
        "the panic message is not in the tail:\n{text}"
    );
    assert!(text.contains("document opened"), "{text}");
    // The engine survives a caught panic: the same session answers the next tile request.
    assert!(client.engine_id().is_some());
}

#[test]
fn an_engine_that_aborts_is_recorded_with_how_it_ended() {
    let reports_dir = tempfile::tempdir().unwrap();
    let (_document, client) = client("abort", Some(reports_dir.path()));
    ask_for_a_tile(&client);
    wait_for(&client, |e| matches!(e, Event::EngineCrashed { .. }));

    let text = wait_for_report(reports_dir.path());
    assert!(
        text.contains("what: the engine ended without being asked"),
        "{text}"
    );
    assert!(text.contains("details: Crash"), "{text}");
    assert!(
        text.contains("forced engine abort"),
        "the last line before the abort is missing:\n{text}"
    );
    let lines_in_report = text
        .lines()
        .skip_while(|l| !l.starts_with("Last "))
        .filter(|l| l.starts_with("[engine "))
        .count();
    assert!(
        (2..=200).contains(&lines_in_report),
        "{lines_in_report} lines\n{text}"
    );
}

#[test]
fn nothing_is_written_unless_a_folder_is_given() {
    let (_document, client) = client("abort", None);
    ask_for_a_tile(&client);
    wait_for(&client, |e| matches!(e, Event::EngineCrashed { .. }));
    // The tail is still there for whoever wants it.
    assert_ne!(client.engine_log_tail(), Vec::<String>::new());
}

#[test]
fn a_crash_loop_does_not_flood_the_folder() {
    let reports_dir = tempfile::tempdir().unwrap();
    let (_document, client) = client("panic", Some(reports_dir.path()));
    ask_for_a_tile(&client);
    for _ in 0..3 {
        // Every tile panics; each is an internal error within the spacing of one report.
        let _ = client.request_tile(&TileRequest {
            page: 0,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 10,
                height: 10,
            },
            slot: SlotId(1),
            priority: Priority::Visible,
        });
    }
    wait_for_report(reports_dir.path());
    std::thread::sleep(Duration::from_millis(600));
    assert_eq!(reports(reports_dir.path()).len(), 1);
}
