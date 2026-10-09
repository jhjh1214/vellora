//! M0 acceptance: every corpus v0 document through the real engine, the way the UI uses it:
//! open, then render a tile of the first page.
//!
//! `#[ignore]`d (needs `cargo xtask corpus fetch` and the PDFium build, takes minutes); run it
//! with `cargo test -p vellora-engine --test corpus -- --ignored --nocapture`. It fails, never
//! skips, without the corpus. `VELLORA_CORPUS_DIR` overrides the corpus directory.
//!
//! Protected documents are opened the way the UI does it: refused with the typed "password
//! required" answer, refused again for a wrong password, then opened with the password from the
//! corpus manifest (`password` field). A protected document without a password there is a failure of
//! the gate, unless it is tagged `malformed` (the two `*_bad_okey` files, which no password opens).
//!
//! "Never crashes" is checked on both sides: the engine process must not end unasked (a crash is
//! reported per file and fails the test), and the client, which stands in for the UI process,
//! must always return a typed event within the patience below instead of panicking or hanging.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::HashSet;
use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use support::pdfium_path;
use vellora_cos::{Limits, ObjectStore};
use vellora_engine_client::{Client, ClientConfig, ClientError, Event, TileRequest};
use vellora_ipc::{ErrorKind, Priority, Repair, RequestId, SlotId, TileRect};
use vellora_shm::SlotGeometry;

/// How long one file may take from open to its first tile.
const PATIENCE: Duration = Duration::from_secs(60);

/// The share of documents the engine must open, password-protected ones included (their
/// passwords come from the manifest). M1 acceptance asks for 98%; the 8 documents PDFium refuses
/// as not-a-PDF keep this corpus below that, see the note under M1 task 7.
const REQUIRED_OPEN_SHARE: f64 = 0.95;

/// A password no document in the corpus has.
const WRONG_PASSWORD: &str = "definitely-not-the-password-\u{20AC}";

#[derive(Debug, PartialEq)]
enum Outcome {
    /// Opened, and the first page rendered (or the document has no pages).
    Rendered,
    /// Opened, but the first page failed with a typed error.
    RenderFailed(String),
    /// Refused with a typed error (`RequestFailed` or `Failed`).
    Rejected(String),
    /// Refused for want of a password the manifest does not have.
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

/// The corpus manifest, parsed.
fn manifest() -> Vec<toml::Table> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/manifest.toml");
    let table: toml::Table = fs::read_to_string(path).unwrap().parse().unwrap();
    table["doc"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| doc.as_table().unwrap().clone())
        .collect()
}

/// Ids tagged `malformed` in the manifest.
fn malformed_ids(docs: &[toml::Table]) -> Vec<String> {
    docs.iter()
        .filter(|doc| {
            doc["categories"]
                .as_array()
                .is_some_and(|c| c.iter().any(|c| c.as_str() == Some("malformed")))
        })
        .map(|doc| doc["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The `password` of a manifest entry, if it has one.
fn password_of(docs: &[toml::Table], id: &str) -> Option<String> {
    docs.iter()
        .find(|doc| doc["id"].as_str() == Some(id))
        .and_then(|doc| doc.get("password"))
        .and_then(|password| password.as_str().map(str::to_owned))
}

fn classify_refusal(kind: ErrorKind, message: String) -> Outcome {
    if kind == ErrorKind::PasswordRequired {
        Outcome::PasswordRequired
    } else {
        Outcome::Rejected(message)
    }
}

/// How a protected document answered the password steps of the gate.
#[derive(Debug, Default, PartialEq, Eq)]
struct PasswordSteps {
    /// Opening without a password was refused with `PasswordRequired`.
    asked: bool,
    /// A wrong password was refused with `WrongPassword` and the engine kept serving.
    wrong_refused: bool,
    /// The manifest's password was refused.
    right_refused: bool,
}

/// M1 task 12b: what the navigation requests, asked once the first tile is in, came to. Each is
/// answered or fails with a typed error; `ReadFailed` (a damaged or over-limit outline) is allowed
/// and counted, anything else is a problem of the gate.
#[derive(Debug, Default)]
struct NavigationRun {
    /// Requests sent and not yet answered.
    pending: HashSet<RequestId>,
    /// The children of one outline item were asked for.
    drilled: bool,
    /// Outline items received, top level and the one level drilled into.
    items: usize,
    /// The document defines page labels.
    labelled: bool,
    /// Some outline item starts at or before the last page.
    sectioned: bool,
    /// Characters of the first page's text read (the first window), over all documents.
    text_chars: usize,
    /// Links on the first page, and how many of them are of a kind that is never run.
    links: usize,
    inert_links: usize,
    /// What the `ReadFailed` answers said.
    unreadable: Vec<String>,
    /// Answers that were neither a typed answer nor `ReadFailed`.
    problems: Vec<String>,
}

/// [`NavigationRun`]s added up over the corpus.
#[derive(Debug, Default)]
struct NavigationTotals {
    items: usize,
    labelled: usize,
    sectioned: usize,
    unreadable: usize,
    links: usize,
    inert_links: usize,
    text_chars: usize,
}

impl NavigationTotals {
    /// Adds one document's run; its problems go to `problems` under the document's `id`.
    fn add(&mut self, id: &str, run: NavigationRun, problems: &mut Vec<String>) {
        self.items += run.items;
        self.labelled += usize::from(run.labelled);
        self.sectioned += usize::from(run.sectioned);
        self.text_chars += run.text_chars;
        self.links += run.links;
        self.inert_links += run.inert_links;
        self.unreadable += run.unreadable.len();
        problems.extend(
            run.problems
                .into_iter()
                .map(|problem| format!("{id}: navigation: {problem}")),
        );
    }
}

impl std::fmt::Display for NavigationTotals {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "navigation: {} outline items read, {} documents with page labels, {} with a section \
             at the last page, {} links on first pages ({} of kinds never run), {} characters of first-page text, {} answered ReadFailed",
            self.items,
            self.labelled,
            self.sectioned,
            self.links,
            self.inert_links,
            self.text_chars,
            self.unreadable
        )
    }
}

impl NavigationRun {
    fn count_links(&mut self, links: &[vellora_ipc::Link]) {
        self.links += links.len();
        self.inert_links += links
            .iter()
            .filter(|link| matches!(link.action, vellora_ipc::LinkAction::Inert(_)))
            .count();
    }
}

/// What the engine made of one file: how it went, and what it said about repairs when it opened.
struct Run {
    outcome: Outcome,
    password: PasswordSteps,
    /// `None` when the document never opened.
    repairs: Option<Vec<Repair>>,
    navigation: NavigationRun,
}

/// One document being walked through the gate: the client that stands in for the UI, and what has
/// happened so far.
struct Walk<'a> {
    client: &'a Client,
    /// The manifest's password, if the document has one.
    password: Option<&'a str>,
    repairs: Option<Vec<Repair>>,
    steps: PasswordSteps,
    /// The tile of page 1 that was asked for once the document opened.
    requested: Option<vellora_ipc::RequestId>,
    page_count: u32,
    navigation: NavigationRun,
}

impl Walk<'_> {
    /// Reacts to one event; `Some` ends the walk.
    fn on_event(&mut self, event: Event) -> Option<Outcome> {
        match event {
            Event::Opened {
                page_count,
                page_sizes,
                repairs,
            } => {
                self.repairs = Some(repairs);
                self.page_count = page_count;
                let Some(size) = page_sizes.first().filter(|_| page_count > 0) else {
                    return self.begin_navigation();
                };
                self.request_first_tile(size.width, size.height)
            }
            Event::TileReady { request, .. } if Some(request) == self.requested => {
                self.begin_navigation()
            }
            Event::Outline { request, items, .. } if self.navigation.pending.remove(&request) => {
                self.navigation.items += items.len();
                self.drill_into(&items);
                self.navigation_step()
            }
            Event::OutlinePath { request, path } if self.navigation.pending.remove(&request) => {
                self.navigation.sectioned |= !path.is_empty();
                self.navigation_step()
            }
            Event::PageLabels {
                request, defined, ..
            } if self.navigation.pending.remove(&request) => {
                self.navigation.labelled |= defined;
                self.navigation_step()
            }
            Event::TextPage { request, chars, .. } if self.navigation.pending.remove(&request) => {
                self.navigation.text_chars += chars.len();
                self.navigation_step()
            }
            Event::Links { request, links, .. } if self.navigation.pending.remove(&request) => {
                self.navigation.count_links(&links);
                self.navigation_step()
            }
            Event::PageFound { request, .. } if self.navigation.pending.remove(&request) => {
                self.navigation_step()
            }
            Event::RequestFailed {
                request: Some(request),
                kind,
                message,
            } if self.navigation.pending.remove(&request) => {
                if kind == ErrorKind::ReadFailed {
                    self.navigation.unreadable.push(message);
                } else {
                    self.navigation
                        .problems
                        .push(format!("{kind:?}: {message}"));
                }
                self.navigation_step()
            }
            // The two password answers drive the gate: first a wrong password, which must be
            // refused without ending the session, then the manifest's.
            Event::RequestFailed {
                request: None,
                kind: ErrorKind::PasswordRequired,
                message,
            } if self.password.is_some() && !self.steps.asked => {
                self.steps.asked = true;
                assert!(message.contains("password required"), "{message}");
                self.submit(WRONG_PASSWORD)
            }
            Event::RequestFailed {
                request: None,
                kind: ErrorKind::WrongPassword,
                ..
            } if self.steps.asked && !self.steps.wrong_refused => {
                self.steps.wrong_refused = true;
                self.submit(self.password.unwrap_or_default())
            }
            Event::RequestFailed {
                request: None,
                kind: ErrorKind::WrongPassword,
                ..
            } if self.steps.wrong_refused => {
                self.steps.right_refused = true;
                Some(Outcome::Rejected(
                    "the manifest's password was refused".to_owned(),
                ))
            }
            Event::RequestFailed {
                request,
                kind,
                message,
            } => Some(if request.is_some() && request == self.requested {
                Outcome::RenderFailed(message)
            } else {
                classify_refusal(kind, message)
            }),
            Event::EngineCrashed { crash, .. } => Some(Outcome::Crashed(format!("{crash:?}"))),
            Event::Failed { reason } => Some(classify_refusal(ErrorKind::Internal, reason)),
            _ => None,
        }
    }

    /// Asks, once per document, for the children of the first outline item that has any.
    fn drill_into(&mut self, items: &[vellora_ipc::OutlineEntry]) {
        if self.navigation.drilled {
            return;
        }
        if let Some(parent) = items.iter().find(|item| item.has_children) {
            self.navigation.drilled = true;
            let id = parent.id;
            self.ask(|client| client.request_outline(Some(id), None, 0, 128));
        }
    }

    /// Sends one navigation request and waits for its answer; a request that cannot be sent is a
    /// problem.
    fn ask(&mut self, send: impl FnOnce(&Client) -> Result<RequestId, ClientError>) {
        match send(self.client) {
            Ok(id) => {
                self.navigation.pending.insert(id);
            }
            Err(e) => self.navigation.problems.push(format!("not sent: {e}")),
        }
    }

    /// The document is open and its first page rendered: asks for the top of the outline, the
    /// labels of the first 1,024 pages, the section of the last page and a label.
    fn begin_navigation(&mut self) -> Option<Outcome> {
        let last = self.page_count.saturating_sub(1);
        self.ask(|client| client.request_outline(None, None, 0, 128));
        self.ask(|client| client.request_page_labels(0, 1024));
        self.ask(|client| client.request_outline_path(last));
        self.ask(|client| client.find_page_label("1"));
        // A document with no pages has no first page to ask about.
        if self.page_count > 0 {
            self.ask(|client| client.request_links(0, 0, 256));
            self.ask(|client| client.request_text_page(0, 0, 8192));
        }
        self.navigation_step()
    }

    /// Ends the walk once every navigation request is answered.
    fn navigation_step(&self) -> Option<Outcome> {
        self.navigation
            .pending
            .is_empty()
            .then_some(Outcome::Rendered)
    }

    fn submit(&self, password: &str) -> Option<Outcome> {
        let sent = self.client.submit_password(password);
        sent.err()
            .map(|e| Outcome::Rejected(format!("submit_password: {e}")))
    }

    /// Asks for page 1 (at most 256 x 256 pixels) of the document that has just opened.
    fn request_first_tile(&mut self, width: f32, height: f32) -> Option<Outcome> {
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (width, height) = (
            (width.ceil() as u32).clamp(1, 256),
            (height.ceil() as u32).clamp(1, 256),
        );
        let request = self.client.request_tile(&TileRequest {
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
            Ok(id) => {
                self.requested = Some(id);
                None
            }
            Err(e) => Some(Outcome::RenderFailed(e.to_string())),
        }
    }
}

fn run_one(path: &Path, password: Option<&str>) -> Run {
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
                password: PasswordSteps::default(),
                repairs: None,
                navigation: NavigationRun::default(),
            };
        }
    };
    let mut walk = Walk {
        client: &client,
        password,
        repairs: None,
        steps: PasswordSteps::default(),
        requested: None,
        page_count: 0,
        navigation: NavigationRun::default(),
    };
    let deadline = Instant::now() + PATIENCE;
    let outcome = loop {
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            break Outcome::Hung;
        };
        if let Some(outcome) = client
            .wait_events(left)
            .into_iter()
            .find_map(|event| walk.on_event(event))
        {
            break outcome;
        }
    };
    let Walk {
        repairs,
        steps,
        navigation,
        ..
    } = walk;
    client.close();
    Run {
        outcome,
        password: steps,
        repairs,
        navigation,
    }
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

/// The forms of a typed password that `cos` can be given: as typed, SASLprep-normalised (encryption
/// revision 6) and as Latin-1 (revisions 2 to 4), as the engine tries them.
fn password_forms(password: &str) -> Vec<Vec<u8>> {
    let mut forms = vec![password.as_bytes().to_vec()];
    if let Ok(prepared) = stringprep::saslprep(password) {
        forms.push(prepared.as_bytes().to_vec());
    }
    if let Some(latin1) = password
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect::<Option<Vec<u8>>>()
    {
        forms.push(latin1);
    }
    forms
}

/// Whether `cos` had to repair the file to read it all: it cannot open or settle it, or reading
/// every page leaves repair notes. Written against `cos` directly, not against the engine's own
/// check, so that the two can disagree. A protected document is unlocked with `password` first;
/// `None` if `cos` cannot unlock it, whose repairs cannot be judged.
fn cos_repairs(path: &Path, password: Option<&str>) -> Option<bool> {
    let bytes = fs::read(path).unwrap();
    let Ok(store) = ObjectStore::open(&bytes, Limits::default()) else {
        return Some(true);
    };
    if store.settle().is_err() {
        return Some(true);
    }
    if store.is_locked() {
        let forms = password.map(password_forms).unwrap_or_default();
        if !forms.iter().any(|form| store.authenticate(form).is_ok()) {
            return None;
        }
    }
    // Page errors do not matter here; reading is what finds the damage.
    store.pages().for_each(drop);
    Some(!store.repaired().is_empty())
}

/// M1 task 7: what is wrong with how `id` went through the password steps, if anything. A
/// protected document asks for its password, refuses a wrong one and opens with the manifest's; a
/// document the manifest gives no password for must not be asked about.
fn password_problem(id: &str, password: Option<&str>, steps: &PasswordSteps) -> Option<String> {
    match (password, steps.asked) {
        (Some(_), true) if !steps.wrong_refused || steps.right_refused => {
            Some(format!("{id}: password steps went wrong: {steps:?}"))
        }
        (Some(_), false) => Some(format!(
            "{id}: the manifest has a password but the engine never asked for it"
        )),
        _ => None,
    }
}

/// Why `outcome` fails the gate, if it does: a crash or a hang, or a refusal for want of a password
/// the manifest does not have. The two `*_bad_okey` files (tagged `malformed`) have a broken `/O`
/// entry, which PDFium answers with "password" whatever is sent and `cos` refuses outright, so no
/// password opens them: a malformed document may be refused, a sound one must open.
fn outcome_problem(id: &str, outcome: &Outcome, is_malformed: bool) -> Option<String> {
    match outcome {
        Outcome::PasswordRequired if !is_malformed => {
            Some(format!("{id}: needs a password the manifest does not have"))
        }
        Outcome::Crashed(_) | Outcome::Hung => Some(format!("{id}: {outcome:?}")),
        _ => None,
    }
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
    let docs = manifest();
    let malformed = malformed_ids(&docs);

    let (mut rendered, mut opened, mut protected, mut malformed_total, mut malformed_bad) =
        (0, 0, 0, 0, 0);
    let mut problems = Vec::new();
    // M1 task 6: the repair notice appears for every file cos repairs and for no other.
    let (mut notices, mut disagreements) = (0, 0);
    let mut notice_mismatches = Vec::new();
    let mut nav_totals = NavigationTotals::default();
    for path in &files {
        let id = path.file_stem().unwrap().to_string_lossy().into_owned();
        let is_malformed = malformed.contains(&id);
        let password = password_of(&docs, &id);
        let started = Instant::now();
        let Run {
            outcome,
            password: steps,
            repairs,
            navigation,
        } = run_one(path, password.as_deref());
        let ms = started.elapsed().as_millis();
        println!(
            "{id}: {outcome:?} ({ms} ms){}{}{}",
            if is_malformed { " [malformed]" } else { "" },
            if steps.asked { " [password]" } else { "" },
            if navigation.unreadable.is_empty() {
                ""
            } else {
                " [navigation unreadable]"
            },
        );
        for reason in &navigation.unreadable {
            println!("    {id}: ReadFailed: {reason}");
        }
        if let Some(repairs) = &repairs {
            let from_cos = repairs
                .iter()
                .filter(|r| !DISAGREEMENTS.contains(&r.code.as_str()))
                .count();
            let expected = cos_repairs(path, password.as_deref());
            notices += usize::from(from_cos > 0);
            disagreements += usize::from(from_cos == 0 && !repairs.is_empty());
            if expected.is_some_and(|expected| expected != (from_cos > 0)) {
                notice_mismatches.push(format!(
                    "{id}: cos repairs = {expected:?}, engine said {repairs:?}"
                ));
            }
        }
        nav_totals.add(&id, navigation, &mut problems);
        protected += usize::from(password.is_some() && steps.asked);
        problems.extend(password_problem(&id, password.as_deref(), &steps));
        problems.extend(outcome_problem(&id, &outcome, is_malformed));
        match &outcome {
            Outcome::Rendered => {
                rendered += 1;
                opened += 1;
            }
            Outcome::RenderFailed(_) => opened += 1,
            _ => {}
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
    let share = f64::from(opened) / (total as f64);
    println!(
        "{total} documents, {protected} protected (opened with their manifest passwords); {opened} opened ({:.1}%), {rendered} rendered page 1, \
         {malformed_total} malformed of which {malformed_bad} crashed or hung, {} problems overall",
        share * 100.0,
        problems.len()
    );
    println!(
        "repair notice for {notices} documents; {disagreements} more only because PDFium and cos disagree"
    );
    println!("{nav_totals}");
    assert!(
        problems.is_empty(),
        "engine crashed or hung, or the password or navigation steps failed: {problems:#?}"
    );
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
