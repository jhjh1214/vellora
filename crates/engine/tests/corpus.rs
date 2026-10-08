//! M0 acceptance: every corpus v0 document through the real engine, the way the UI uses it:
//! open, then render a tile of the first page.
//!
//! `#[ignore]`d (needs `cargo xtask corpus fetch` and the PDFium build, takes minutes); run it
//! with `cargo test -p vellora-engine --test corpus -- --ignored --nocapture`. It fails, never
//! skips, without the corpus. `VELLORA_CORPUS_DIR` overrides the corpus directory.
//!
//! "Never crashes" is checked on both sides: the engine process must not end unasked (a crash is
//! reported per file and fails the test), and the client, which stands in for the UI process,
//! must always return a typed event within the patience below instead of panicking or hanging.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use support::pdfium_path;
use vellora_cos::{Limits, ObjectStore};
use vellora_engine_client::{Client, ClientConfig, Event, TileRequest};
use vellora_ipc::{Priority, Repair, SlotId, TileRect};
use vellora_shm::SlotGeometry;

/// How long one file may take from open to its first tile.
const PATIENCE: Duration = Duration::from_secs(60);

/// M0 criterion: the share of documents the engine must open. Password-protected documents are
/// excluded from the denominator and must be refused with the typed "password required" error.
const REQUIRED_OPEN_SHARE: f64 = 0.95;

#[derive(Debug, PartialEq)]
enum Outcome {
    /// Opened, and the first page rendered (or the document has no pages).
    Rendered,
    /// Opened, but the first page failed with a typed error.
    RenderFailed(String),
    /// Refused with a typed error (`RequestFailed` or `Failed`).
    Rejected(String),
    /// Refused because the document is encrypted and no password was given (a typed, expected
    /// answer: the engine has no password path in M0).
    PasswordRequired,
    /// The engine process ended unasked.
    Crashed(String),
    /// No answer within the patience.
    Hung,
}

fn corpus_dir() -> PathBuf {
    env::var_os("VELLORA_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data"),
        PathBuf::from,
    )
}

/// Ids tagged `malformed` in the manifest (a line-based read: the manifest layout is fixed).
fn malformed_ids() -> Vec<String> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/manifest.toml");
    let text = fs::read_to_string(manifest).unwrap();
    let mut ids = Vec::new();
    let mut current = String::new();
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("id = \"") {
            rest.trim_end_matches('"').clone_into(&mut current);
        } else if line.starts_with("categories") && line.contains("\"malformed\"") {
            ids.push(current.clone());
        }
    }
    ids
}

fn classify_refusal(message: String) -> Outcome {
    if message.contains("password required") {
        Outcome::PasswordRequired
    } else {
        Outcome::Rejected(message)
    }
}

/// What the engine made of one file: how it went, and what it said about repairs when it opened.
struct Run {
    outcome: Outcome,
    /// `None` when the document never opened.
    repairs: Option<Vec<Repair>>,
}

fn run_one(path: &Path) -> Run {
    let mut config = ClientConfig::new(
        env!("CARGO_BIN_EXE_vellora-engine"),
        SlotGeometry::new(2, 1 << 20).unwrap(),
    );
    config
        .env
        .push((vellora_render::LIBRARY_ENV.into(), pdfium_path().into()));
    // A crash must show up as a crash, not be hidden by a restart.
    config.max_restarts = 0;
    let client = match Client::open(config, path) {
        Ok(client) => client,
        Err(e) => {
            return Run {
                outcome: Outcome::Rejected(format!("client: {e}")),
                repairs: None,
            };
        }
    };
    let mut repairs = None;
    let deadline = Instant::now() + PATIENCE;
    let mut requested = None;
    let outcome = loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break Outcome::Hung;
        };
        let mut done = None;
        for event in client.wait_events(left) {
            match event {
                Event::Opened {
                    page_count,
                    page_sizes,
                    repairs: said,
                } => {
                    repairs = Some(said);
                    let Some(size) = page_sizes.first().filter(|_| page_count > 0) else {
                        done = Some(Outcome::Rendered);
                        break;
                    };
                    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
                    let (width, height) = (
                        (size.width.ceil() as u32).clamp(1, 256),
                        (size.height.ceil() as u32).clamp(1, 256),
                    );
                    let request = client.request_tile(&TileRequest {
                        page: 0,
                        scale: 1.0,
                        rect: TileRect {
                            x: 0,
                            y: 0,
                            width,
                            height,
                        },
                        slot: SlotId(0),
                        priority: Priority::Visible,
                    });
                    match request {
                        Ok(id) => requested = Some(id),
                        Err(e) => done = Some(Outcome::RenderFailed(e.to_string())),
                    }
                }
                Event::TileReady { request, .. } if Some(request) == requested => {
                    done = Some(Outcome::Rendered);
                }
                Event::RequestFailed {
                    request, message, ..
                } => {
                    done = Some(if request.is_some() && request == requested {
                        Outcome::RenderFailed(message)
                    } else {
                        classify_refusal(message)
                    });
                }
                Event::EngineCrashed { crash, .. } => {
                    done = Some(Outcome::Crashed(format!("{crash:?}")));
                }
                Event::Failed { reason } => done = Some(classify_refusal(reason)),
                _ => {}
            }
            if done.is_some() {
                break;
            }
        }
        if let Some(outcome) = done {
            break outcome;
        }
    };
    client.close();
    Run { outcome, repairs }
}

/// Engine findings where `cos` repaired nothing: PDFium and `cos` disagree (a page count, a page
/// PDFium cannot measure, an encrypted file PDFium opened and `cos` cannot unlock, a page `cos`
/// cannot read). The notice is right to say so, but it is not a `cos` repair.
const DISAGREEMENTS: [&str; 4] = [
    "page-count-mismatch",
    "page-unmeasurable",
    "cos-locked",
    "cos-page-unreadable",
];

/// Whether `cos` had to repair the file to read it all: it cannot open or settle it, or reading
/// every page leaves repair notes. Written against `cos` directly, not against the engine's own
/// check, so that the two can disagree. `None` for a document `cos` cannot unlock without a
/// password, whose repairs cannot be judged before the password path (M1 task 7).
fn cos_repairs(path: &Path) -> Option<bool> {
    let bytes = fs::read(path).unwrap();
    let Ok(store) = ObjectStore::open(&bytes, Limits::default()) else {
        return Some(true);
    };
    if store.settle().is_err() {
        return Some(true);
    }
    if store.is_locked() {
        return None;
    }
    // Page errors do not matter here; reading is what finds the damage.
    store.pages().for_each(drop);
    Some(!store.repaired().is_empty())
}

#[test]
#[ignore = "needs the corpus (cargo xtask corpus fetch) and PDFium; run with --ignored"]
fn corpus_opens_without_crashing() {
    let dir = corpus_dir();
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .unwrap_or_else(|e| panic!("corpus not found at {}: {e}", dir.display()))
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "pdf"))
        .collect();
    files.sort();
    assert!(!files.is_empty(), "no PDFs in {}", dir.display());
    let malformed = malformed_ids();

    let (mut rendered, mut opened, mut locked, mut malformed_total, mut malformed_bad) =
        (0, 0, 0, 0, 0);
    let mut problems = Vec::new();
    // M1 task 6: the repair notice appears for every file cos repairs and for no other.
    let (mut notices, mut disagreements) = (0, 0);
    let mut notice_mismatches = Vec::new();
    for path in &files {
        let id = path.file_stem().unwrap().to_string_lossy().into_owned();
        let is_malformed = malformed.contains(&id);
        let started = Instant::now();
        let Run { outcome, repairs } = run_one(path);
        let ms = started.elapsed().as_millis();
        println!(
            "{id}: {outcome:?} ({ms} ms){}",
            if is_malformed { " [malformed]" } else { "" }
        );
        if let Some(repairs) = &repairs {
            let from_cos = repairs
                .iter()
                .filter(|r| !DISAGREEMENTS.contains(&r.code.as_str()))
                .count();
            let expected = cos_repairs(path);
            notices += usize::from(from_cos > 0);
            disagreements += usize::from(from_cos == 0 && !repairs.is_empty());
            if expected.is_some_and(|expected| expected != (from_cos > 0)) {
                notice_mismatches.push(format!(
                    "{id}: cos repairs = {expected:?}, engine said {repairs:?}"
                ));
            }
        }
        match &outcome {
            Outcome::Rendered => {
                rendered += 1;
                opened += 1;
            }
            Outcome::RenderFailed(_) => opened += 1,
            Outcome::PasswordRequired => locked += 1,
            Outcome::Rejected(_) => {}
            Outcome::Crashed(_) | Outcome::Hung => problems.push(format!("{id}: {outcome:?}")),
        }
        if is_malformed {
            malformed_total += 1;
            if matches!(outcome, Outcome::Crashed(_) | Outcome::Hung) {
                malformed_bad += 1;
            }
        }
    }

    let total = files.len();
    #[allow(clippy::cast_precision_loss)]
    let share = f64::from(opened) / (f64::from(i32::try_from(total).unwrap() - locked));
    println!(
        "{total} documents, {locked} need a password (not counted); {opened} opened ({:.1}%), {rendered} rendered page 1, \
         {malformed_total} malformed of which {malformed_bad} crashed or hung, {} crashed or hung overall",
        share * 100.0,
        problems.len()
    );
    println!(
        "repair notice for {notices} documents; {disagreements} more only because PDFium and cos disagree"
    );
    assert!(problems.is_empty(), "engine crashed or hung: {problems:#?}");
    assert!(
        notice_mismatches.is_empty(),
        "the repair notice differs from what cos repairs: {notice_mismatches:#?}"
    );
    assert!(
        share >= REQUIRED_OPEN_SHARE,
        "only {:.1}% of the corpus opened",
        share * 100.0
    );
}
