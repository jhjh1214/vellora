//! M1 task 9b: the application log. The real engine runs sandboxed (Windows `AppContainer`, Linux
//! Landlock and seccomp) and cannot write files; its log lines must still end up in the UI's
//! rotating log files, together with the UI's own, and must never hold a password.
//!
//! One test in this file: `tracing` has one global subscriber per process.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;
use std::fs;
use std::time::{Duration, Instant};

use support::pdfium_path;
use tracing::Level;
use vellora_engine_client::logging::{self, LOG_FILE, LogConfig, LogError};
use vellora_engine_client::{Client, ClientConfig, Event};
use vellora_ipc::ErrorKind;
use vellora_shm::SlotGeometry;

const PATIENCE: Duration = Duration::from_secs(60);

fn wait_for(client: &Client, mut done: impl FnMut(&Event) -> bool) {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("the expected event did not come");
        for event in client.wait_events(left) {
            if done(&event) {
                return;
            }
        }
    }
}

fn all_log_text(dir: &std::path::Path) -> String {
    let mut text = String::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        writeln!(text, "==== {}", path.display()).unwrap();
        text.push_str(&fs::read_to_string(path).unwrap());
    }
    text
}

#[test]
fn ui_and_engine_lines_reach_the_log_files_and_passwords_do_not() {
    let log_dir = tempfile::tempdir().unwrap();
    let mut log = LogConfig::new(log_dir.path().join("nested/logs"));
    log.level = Level::DEBUG;
    let guard = logging::init(&log).unwrap();
    assert!(matches!(logging::init(&log), Err(LogError::AlreadyStarted)));
    assert!(logging::is_started());

    tracing::info!("a line from the ui, marker-ui-4f1c");
    logging::log_qt(Level::WARN, "a qt warning, marker-qt-9d2e");

    // A protected document, opened with a wrong and then the right password, with the engine at
    // its most talkative.
    let document_dir = tempfile::tempdir().unwrap();
    let path = document_dir.path().join("protected.pdf");
    fs::copy(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cos/tests/fixtures/encryption/r6-aes-256-user-password.pdf"),
        &path,
    )
    .unwrap();
    let mut config = ClientConfig::new(
        env!("CARGO_BIN_EXE_vellora-engine"),
        SlotGeometry::new(2, 1 << 20).unwrap(),
    );
    config
        .env
        .push((vellora_render::LIBRARY_ENV.into(), pdfium_path().into()));
    config.env.push(("VELLORA_LOG".into(), "trace".into()));
    let client = Client::open(config, &path).unwrap();
    let refused = |kind: ErrorKind| move |e: &Event| matches!(e, Event::RequestFailed { request: None, kind: k, .. } if *k == kind);
    wait_for(&client, refused(ErrorKind::PasswordRequired));
    client.submit_password("hunter2-not-the-password").unwrap();
    wait_for(&client, refused(ErrorKind::WrongPassword));
    client.submit_password("user-pw").unwrap();
    wait_for(&client, |e| matches!(e, Event::Opened { .. }));
    client.close();

    // The engine says "session ended" as it leaves; its pipe is read by a thread of the client.
    let deadline = Instant::now() + PATIENCE;
    while !client
        .engine_log_tail()
        .iter()
        .any(|line| line.contains("session ended"))
    {
        assert!(
            Instant::now() < deadline,
            "the engine's last line never came: {:?}",
            client.engine_log_tail()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
    guard.flush();

    let text = all_log_text(&log.dir);
    assert!(text.contains("marker-ui-4f1c"), "{text}");
    assert!(text.contains("qt: a qt warning, marker-qt-9d2e"), "{text}");
    assert!(text.contains("engine started"), "{text}");
    // The engine's own lines, written through its sandboxed standard error and tagged.
    assert!(
        text.contains("engine: ") && text.contains("document opened"),
        "no engine line in the log:\n{text}"
    );
    assert!(
        text.contains("opening with a password"),
        "the engine did not log at trace level:\n{text}"
    );
    assert!(text.contains("session ended"), "{text}");
    // The tail kept for crash reports has the same lines, marked with the engine's process id.
    let tail = client.engine_log_tail();
    assert!(tail.len() <= 200 && !tail.is_empty());
    assert!(
        tail.iter().all(|line| line.starts_with("[engine ")),
        "{tail:?}"
    );

    // Task 7: no password at any level, not in the files and not in the tail.
    for secret in ["hunter2", "user-pw"] {
        assert!(!text.contains(secret), "the log contains {secret}:\n{text}");
        assert!(
            tail.iter().all(|line| !line.contains(secret)),
            "the tail contains {secret}"
        );
    }
    assert!(log.dir.join(LOG_FILE).is_file());
    drop(guard);
    assert!(!logging::is_started());
}
