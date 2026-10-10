//! M1 task 14c: the text the engine extracts against an independent extractor.
//!
//! The reference is Poppler's `pdftotext`, used **only as an external test tool**: it is installed
//! in CI, never linked or shipped (dependency policy). For each document of the text subset
//! (`tests/corpus/text-manifest.toml`) the first pages are extracted both ways, normalised, and
//! compared by edit distance; each must be within [`MAX_DISTANCE`].
//!
//! `#[ignore]`d (needs the corpus, the PDFium build and `pdftotext`); run it with
//! `cargo test -p vellora-engine --test text_compare -- --ignored --nocapture`. It **fails**, never
//! skips, without them. `VELLORA_CORPUS_DIR` overrides the corpus directory, `VELLORA_PDFTOTEXT`
//! the `pdftotext` executable. `VELLORA_TEXT_SURVEY=1` measures every document of the corpus
//! (not asserting) to help choose a subset.
//!
//! What is compared, and why it is fair: the characters of the first [`PAGES`] pages with all white
//! space and control characters removed (the two extractors place spaces and breaks by different
//! rules, which says nothing about the characters), ligatures expanded and soft hyphens dropped,
//! at most [`MAX_CHARS`] characters. The distance is the Levenshtein distance over characters
//! divided by the longer text.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::env;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use support::pdfium_path;
use vellora_engine_client::{Client, ClientConfig, Event};
use vellora_shm::SlotGeometry;

/// Pages compared from the start of each document.
const PAGES: u32 = 10;
/// Characters compared per document (the rest is ignored on both sides).
const MAX_CHARS: usize = 6000;
/// The most two extractions may differ, as a share of the longer one.
const MAX_DISTANCE: f64 = 0.02;
/// Texts shorter than this are too small to compare meaningfully.
const MIN_CHARS: usize = 30;
/// How long one document may take to extract.
const PATIENCE: Duration = Duration::from_secs(60);

fn corpus_dir() -> PathBuf {
    env::var_os("VELLORA_CORPUS_DIR").map_or_else(
        || Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data"),
        PathBuf::from,
    )
}

fn pdftotext() -> PathBuf {
    env::var_os("VELLORA_PDFTOTEXT").map_or_else(|| PathBuf::from("pdftotext"), PathBuf::from)
}

/// The ids of a corpus manifest, in order.
fn ids(manifest: &Path) -> Vec<String> {
    let table: toml::Table = fs::read_to_string(manifest).unwrap().parse().unwrap();
    table["doc"]
        .as_array()
        .unwrap()
        .iter()
        .map(|doc| doc["id"].as_str().unwrap().to_owned())
        .collect()
}

/// The text of the first pages as the engine extracts it, or why it could not be.
fn engine_text(path: &Path) -> Result<String, String> {
    let mut config = ClientConfig::new(
        env!("CARGO_BIN_EXE_vellora-engine"),
        SlotGeometry::new(2, 1 << 20).unwrap(),
    );
    config
        .env
        .push((vellora_render::LIBRARY_ENV.into(), pdfium_path().into()));
    config.max_restarts = 0;
    let client = Client::open(config, path).map_err(|e| format!("open: {e}"))?;
    let deadline = Instant::now() + PATIENCE;
    let wait = |client: &Client| -> Result<Vec<Event>, String> {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("timed out")?;
        Ok(client.wait_events(left))
    };

    let mut pages = None;
    while pages.is_none() {
        for event in wait(&client)? {
            match event {
                Event::Opened { page_count, .. } => pages = Some(page_count),
                Event::RequestFailed { message, .. } => return Err(format!("open: {message}")),
                Event::EngineCrashed { crash, .. } => return Err(format!("crashed: {crash:?}")),
                _ => {}
            }
        }
    }
    let mut text = String::new();
    for page in 0..pages.unwrap_or(0).min(PAGES) {
        let mut skip = 0_u32;
        loop {
            let request = client
                .request_text_page(page, skip, 8192)
                .map_err(|e| e.to_string())?;
            let (chars, total) = loop {
                let mut found = None;
                for event in wait(&client)? {
                    match event {
                        Event::TextPage {
                            request: r,
                            chars,
                            total,
                            ..
                        } if r == request => found = Some((chars, total)),
                        Event::RequestFailed { message, .. } => {
                            return Err(format!("text of page {}: {message}", page + 1));
                        }
                        Event::EngineCrashed { crash, .. } => {
                            return Err(format!("crashed: {crash:?}"));
                        }
                        _ => {}
                    }
                }
                if let Some(found) = found {
                    break found;
                }
            };
            text.extend(chars.iter().map(|c| c.ch));
            skip += u32::try_from(chars.len()).unwrap();
            if chars.is_empty() || skip >= total {
                break;
            }
        }
        text.push('\n');
    }
    client.close();
    Ok(text)
}

/// The text of the first pages as `pdftotext` extracts it.
fn reference_text(path: &Path) -> Result<String, String> {
    let output = Command::new(pdftotext())
        .args(["-q", "-enc", "UTF-8", "-f", "1", "-l", &PAGES.to_string()])
        .arg(path)
        .arg("-")
        .output()
        .map_err(|e| {
            format!("cannot run pdftotext ({e}); install Poppler or set VELLORA_PDFTOTEXT")
        })?;
    if !output.status.success() {
        return Err(format!("pdftotext: {}", output.status));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// The characters that are compared: no white space or control characters, ligatures expanded,
/// soft hyphens dropped, at most [`MAX_CHARS`].
fn normalise(text: &str) -> Vec<char> {
    let mut out = Vec::new();
    for c in text.chars() {
        let expanded: &[char] = match c {
            '\u{FB00}' => &['f', 'f'],
            '\u{FB01}' => &['f', 'i'],
            '\u{FB02}' => &['f', 'l'],
            '\u{FB03}' => &['f', 'f', 'i'],
            '\u{FB04}' => &['f', 'f', 'l'],
            '\u{FB05}' | '\u{FB06}' => &['s', 't'],
            '\u{00AD}' => &[],
            _ if c.is_whitespace() || c.is_control() || c == '\u{200B}' || c == '\u{FFFD}' => &[],
            _ => std::slice::from_ref(&c),
        };
        out.extend_from_slice(expanded);
        if out.len() >= MAX_CHARS {
            out.truncate(MAX_CHARS);
            break;
        }
    }
    out
}

/// Levenshtein distance over characters (two rows).
fn distance(a: &[char], b: &[char]) -> usize {
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    let mut current = vec![0; b.len() + 1];
    for (i, ca) in a.iter().enumerate() {
        current[0] = i + 1;
        for (j, cb) in b.iter().enumerate() {
            let substitute = previous[j] + usize::from(ca != cb);
            current[j + 1] = substitute.min(previous[j + 1] + 1).min(current[j] + 1);
        }
        std::mem::swap(&mut previous, &mut current);
    }
    previous[b.len()]
}

/// The distance as a share of the longer text; `None` if both are too short to compare.
fn share(a: &[char], b: &[char]) -> Option<f64> {
    let longer = a.len().max(b.len());
    if longer < MIN_CHARS {
        return None;
    }
    #[allow(clippy::cast_precision_loss)]
    Some(distance(a, b) as f64 / longer as f64)
}

/// One document: its distance, or why it could not be measured.
fn compare(path: &Path) -> Result<(Option<f64>, usize, usize), String> {
    let ours = normalise(&engine_text(path)?);
    let theirs = normalise(&reference_text(path)?);
    Ok((share(&ours, &theirs), ours.len(), theirs.len()))
}

#[test]
fn the_distance_of_equal_and_different_texts() {
    let a: Vec<char> = "kitten".chars().collect();
    let b: Vec<char> = "sitting".chars().collect();
    assert_eq!(distance(&a, &b), 3);
    assert_eq!(distance(&a, &a), 0);
    assert_eq!(distance(&[], &b), 7);
    assert_eq!(distance(&a, &[]), 6);
}

#[test]
fn normalising_ignores_what_the_extractors_decide_differently() {
    let text = normalise("  e\u{FB03}cient\u{00AD} \r\n\t co\u{FB00}ee \u{FFFD}");
    assert_eq!(text.iter().collect::<String>(), "efficientcoffee");
    assert_eq!(normalise(&"a".repeat(MAX_CHARS * 2)).len(), MAX_CHARS);
    assert_eq!(share(&text, &text), None, "too short to compare");
    let long: Vec<char> = "abcdefghij".repeat(30).chars().collect();
    assert_eq!(share(&long, &long), Some(0.0));
}

#[test]
#[ignore = "needs the corpus (cargo xtask corpus fetch), PDFium and pdftotext; run with --ignored"]
fn engine_text_matches_pdftotext_on_the_text_subset() {
    let survey = env::var_os("VELLORA_TEXT_SURVEY").is_some();
    let manifest = if survey {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/manifest.toml")
    } else {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus/text-manifest.toml")
    };
    let dir = corpus_dir();
    let mut worst = (String::new(), 0.0_f64);
    let mut failures = Vec::new();
    let mut measured = 0;
    for id in ids(&manifest) {
        let path = dir.join(format!("{id}.pdf"));
        assert!(
            path.is_file(),
            "{} is missing: fetch the corpus",
            path.display()
        );
        match compare(&path) {
            Ok((Some(share), ours, theirs)) => {
                measured += 1;
                println!("{id}: {:.2}% ({ours} / {theirs} characters)", share * 100.0);
                if share > worst.1 {
                    worst = (id.clone(), share);
                }
                if share > MAX_DISTANCE {
                    failures.push(format!("{id}: {:.2}%", share * 100.0));
                }
            }
            Ok((None, ours, theirs)) => println!("{id}: too short to compare ({ours} / {theirs})"),
            Err(why) => {
                println!("{id}: not measured: {why}");
                if !survey {
                    failures.push(format!("{id}: {why}"));
                }
            }
        }
    }
    println!(
        "{measured} documents measured; worst: {} at {:.2}%",
        worst.0,
        worst.1 * 100.0
    );
    if survey {
        return;
    }
    assert!(
        measured >= 30,
        "only {measured} documents could be compared"
    );
    assert!(
        failures.is_empty(),
        "text differs from pdftotext by more than {:.0}%: {failures:#?}",
        MAX_DISTANCE * 100.0
    );
}
