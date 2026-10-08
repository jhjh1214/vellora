//! The client's own deadlines and its protection of the document file (M1 task 2), against a fake
//! engine that misbehaves on purpose (`examples/fake_engine.rs`; `cargo test` builds it).
//!
//! No PDFium is needed: nothing here renders.

// Test code: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use vellora_engine::Deadlines;
use vellora_engine_client::{Client, ClientConfig, ClientError, Event, Stage, TileLookup};
use vellora_engine_client::{ScaleBucket, TileKey};
use vellora_ipc::Priority;
use vellora_shm::SlotGeometry;

/// How long a test waits for an event before calling the client hung.
const PATIENCE: Duration = Duration::from_secs(60);

/// The deadline the tests give the engine. Short, so the tests are quick; long enough that a loaded
/// CI machine does not trip it before the fake engine has even started.
const SHORT: Duration = Duration::from_millis(500);

/// Where `cargo test` put the fake engine: next to the test executables, in `examples/`.
fn fake_engine() -> PathBuf {
    let exe = env::current_exe().unwrap();
    let target = exe.parent().and_then(Path::parent).unwrap();
    let path = target
        .join("examples")
        .join(format!("fake_engine{}", env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "{} is missing: run `cargo test -p vellora-engine-client` (it builds the examples)",
        path.display()
    );
    path
}

/// A client configuration for the fake engine in `mode` (see the example).
fn config(mode: &str) -> ClientConfig {
    let mut config = ClientConfig::new(fake_engine(), SlotGeometry::new(4, 1 << 20).unwrap());
    config.env.push(("VELLORA_FAKE_ENGINE".into(), mode.into()));
    config
}

/// A small file to open, in a directory of its own that lives as long as the guard.
fn document() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("doc.pdf");
    fs::write(&path, b"%PDF-1.4\n%%EOF\n").unwrap();
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

/// Whether a process with this id exists.
fn alive(pid: u32) -> bool {
    if cfg!(windows) {
        let listing = Command::new("tasklist")
            .args(["/NH", "/FI", &format!("PID eq {pid}")])
            .output()
            .unwrap();
        String::from_utf8_lossy(&listing.stdout).contains(&pid.to_string())
    } else {
        Command::new("kill")
            .args(["-0", &pid.to_string()])
            .output()
            .unwrap()
            .status
            .success()
    }
}

/// Waits (a killed process takes a moment to vanish from the process table) until `pid` is gone.
fn assert_gone(pid: u32) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while alive(pid) {
        assert!(Instant::now() < deadline, "engine {pid} is still running");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn some_tile() -> TileKey {
    TileKey {
        page: 0,
        scale: ScaleBucket::from_scale(1.0).unwrap(),
        x: 0,
        y: 0,
    }
}

#[test]
fn an_engine_that_never_says_hello_is_killed_at_its_deadline() {
    let (_dir, path) = document();
    let mut config = config("mute");
    config.hello_timeout = SHORT;
    let started = Instant::now();
    let client = Client::open(config, &path).unwrap();
    let pid = client.engine_id().expect("the engine started");

    let seen = wait_for(&client, |event| {
        matches!(event, Event::EngineTimeout { .. })
    });
    let elapsed = started.elapsed();
    assert!(
        matches!(
            seen.last(),
            Some(Event::EngineTimeout {
                stage: Stage::Hello,
                ..
            })
        ),
        "{seen:?}"
    );
    assert!(
        elapsed >= SHORT,
        "reported after {elapsed:?}, before the deadline"
    );
    assert!(elapsed < SHORT + Duration::from_secs(10), "{elapsed:?}");
    assert_gone(pid);

    // Final: no restart, no further events, and the client refuses work.
    std::thread::sleep(SHORT);
    assert_eq!(client.poll_events(), Vec::new());
    assert!(client.engine_id().is_none());
    assert!(matches!(
        client.request_cached_tile(some_tile(), Priority::Visible),
        Err(ClientError::EngineUnavailable)
    ));
}

#[test]
fn an_engine_that_never_answers_open_is_killed_at_its_deadline() {
    let (_dir, path) = document();
    let mut config = config("no-open");
    config.open_timeout = SHORT;
    let client = Client::open(config, &path).unwrap();
    let pid = client.engine_id().expect("the engine started");

    let seen = wait_for(&client, |event| {
        matches!(event, Event::EngineTimeout { .. })
    });
    assert!(
        matches!(
            seen.last(),
            Some(Event::EngineTimeout {
                stage: Stage::Open,
                waited,
            }) if *waited == SHORT
        ),
        "{seen:?}"
    );
    assert_gone(pid);
    assert!(client.page_count().is_none());
}

#[test]
fn a_wedged_tile_is_killed_by_the_client_and_the_engine_restarts() {
    let (_dir, path) = document();
    let mut config = config("wedged");
    // The watchdog gives the engine twice its hard deadline before it steps in.
    config.deadlines = Deadlines {
        soft: Duration::from_millis(50),
        hard: SHORT / 2,
    };
    let client = Client::open(config, &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let first = client.engine_id().unwrap();

    let TileLookup::Requested(request) = client
        .request_cached_tile(some_tile(), Priority::Visible)
        .unwrap()
    else {
        panic!("a new tile must be requested");
    };
    let asked = Instant::now();

    let seen = wait_for(&client, |event| matches!(event, Event::EngineRestarted));
    let waited = asked.elapsed();
    assert!(
        waited >= SHORT,
        "killed after {waited:?}, before 2x the hard deadline"
    );
    assert!(
        matches!(
            seen.as_slice(),
            [
                Event::EngineTimeout {
                    stage: Stage::Tile,
                    ..
                },
                Event::EngineCrashed {
                    will_restart: true,
                    lost,
                    ..
                },
                Event::EngineRestarted,
            ] if lost == &[request]
        ),
        "{seen:?}"
    );
    assert_gone(first);

    // The new engine is a different process, and it opens the document again.
    assert_ne!(client.engine_id(), Some(first));
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
}

#[test]
fn an_idle_engine_is_never_killed_by_the_watchdog() {
    // With no request in flight there is nothing to be late with: an idle engine is never killed,
    // however short the tile deadline.
    let (_dir, path) = document();
    let mut config = config("wedged");
    config.deadlines = Deadlines {
        soft: Duration::from_millis(10),
        hard: Duration::from_millis(50),
    };
    let client = Client::open(config, &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let pid = client.engine_id().unwrap();
    std::thread::sleep(Duration::from_secs(1));
    assert_eq!(client.poll_events(), Vec::new());
    assert_eq!(client.engine_id(), Some(pid));
}

/// Another process tries to open `path` for writing without changing it.
#[cfg(windows)]
fn other_process_can_write(path: &Path) -> bool {
    Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "try { [IO.File]::Open($env:VELLORA_TEST_PATH, 'Open', 'Write', 'ReadWrite').Close(); exit 0 } catch { exit 7 }",
        ])
        .env("VELLORA_TEST_PATH", path)
        .status()
        .unwrap()
        .success()
}

#[cfg(windows)]
#[test]
fn on_windows_no_other_process_can_open_the_document_for_writing() {
    let (_dir, path) = document();
    // Control: the check itself works while nobody holds the file.
    assert!(other_process_can_write(&path), "the writer check is broken");

    let client = Client::open(config("no-open"), &path).unwrap();
    assert!(
        !other_process_can_write(&path),
        "a writer got in while the document was open"
    );
    // Readers are fine: other tabs and other viewers.
    fs::File::open(&path).unwrap();
    // Nor can the file be renamed over or deleted while it is open.
    assert!(fs::remove_file(&path).is_err());

    drop(client);
    assert!(
        other_process_can_write(&path),
        "the document stayed locked after the client closed"
    );
}

#[cfg(unix)]
#[test]
fn a_file_changed_behind_the_clients_back_is_reported_when_the_engine_restarts() {
    use std::io::Write;

    let (_dir, path) = document();
    let client = Client::open(config("wedged"), &path).unwrap();
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    let pid = client.engine_id().unwrap();

    // Nothing has changed yet: a crash restarts the engine without a notice.
    kill(pid);
    let seen = wait_for(&client, |event| matches!(event, Event::EngineRestarted));
    assert!(
        !seen
            .iter()
            .any(|event| matches!(event, Event::DocumentChanged { .. })),
        "{seen:?}"
    );
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));

    // Advisory locks stop only cooperating writers, so this append goes through.
    fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .unwrap()
        .write_all(b"appended\n")
        .unwrap();
    kill(client.engine_id().unwrap());
    let seen = wait_for(&client, |event| matches!(event, Event::EngineRestarted));
    assert!(
        seen.iter().any(|event| matches!(
            event,
            Event::DocumentChanged {
                change: vellora_engine_client::Change::Modified
            }
        )),
        "{seen:?}"
    );
    // It is reported once, not at every later restart.
    wait_for(&client, |event| matches!(event, Event::Opened { .. }));
    kill(client.engine_id().unwrap());
    let seen = wait_for(&client, |event| matches!(event, Event::EngineRestarted));
    assert!(
        !seen
            .iter()
            .any(|event| matches!(event, Event::DocumentChanged { .. })),
        "{seen:?}"
    );
}

/// Ends a process the way a crash would, from outside.
#[cfg(unix)]
fn kill(pid: u32) {
    let status = Command::new("kill")
        .args(["-9", &pid.to_string()])
        .status()
        .unwrap();
    assert!(status.success(), "could not kill the engine (pid {pid})");
}
