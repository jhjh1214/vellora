//! Runs `inspect` over corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-inspect --test corpus -- --ignored --nocapture
//! ```
//!
//! Nothing may panic or hang (60 s guard per file). A file may fail to open, but only with a
//! typed error; everything that opens must give a report.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::time::Duration;

use vellora_inspect::{Options, inspect};

#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn inspect_handles_every_corpus_file() {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data");
    assert!(
        dir.is_dir(),
        "corpus missing: run `cargo xtask corpus fetch`"
    );
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();
    assert!(!paths.is_empty(), "the corpus directory has no PDF files");

    let (mut reported, mut repaired, mut locked, mut incomplete) = (0, 0, 0, 0);
    let mut totals = [0usize; 13];
    let mut incomplete_files = Vec::new();
    let mut errors = Vec::new();
    let mut failures = Vec::new();
    for path in &paths {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let data = std::fs::read(path).unwrap();
        let (sender, receiver) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = sender.send(inspect(&data, &Options::default()));
        });
        match receiver.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(summary)) => {
                reported += 1;
                repaired += usize::from(summary.repaired);
                locked += usize::from(summary.locked);
                if let Some(f) = summary.features {
                    incomplete += usize::from(!f.complete);
                    if !f.complete {
                        incomplete_files.push(format!("{name}: {:?}", summary.problems));
                    }
                    let flags = [
                        f.open_action_javascript,
                        f.javascript_actions,
                        f.names_javascript,
                        f.additional_actions,
                        f.launch_actions,
                        f.uri_actions,
                        f.submit_form_actions,
                        f.goto_remote_actions,
                        f.embedded_files,
                        f.acroform,
                        f.xfa,
                        f.optional_content,
                        f.signature_fields,
                    ];
                    for (total, flag) in totals.iter_mut().zip(flags) {
                        *total += usize::from(flag);
                    }
                }
            }
            Ok(Err(error)) => errors.push(format!("{name}: {error}")),
            Err(mpsc::RecvTimeoutError::Timeout) => failures.push(format!("{name}: hang")),
            Err(mpsc::RecvTimeoutError::Disconnected) => failures.push(format!("{name}: panic")),
        }
    }
    println!(
        "files {}: {reported} reported ({repaired} repaired, {locked} locked, {incomplete} with \
         an incomplete scan), {} typed errors",
        paths.len(),
        errors.len()
    );
    println!(
        "files with: JS on open {}, JS actions {}, document JS {}, /AA {}, Launch {}, URI {}, SubmitForm {}, GoToR {}, embedded {}, AcroForm {}, XFA {}, layers {}, signature fields {}",
        totals[0],
        totals[1],
        totals[2],
        totals[3],
        totals[4],
        totals[5],
        totals[6],
        totals[7],
        totals[8],
        totals[9],
        totals[10],
        totals[11],
        totals[12]
    );
    for line in &incomplete_files {
        println!("  incomplete: {line}");
    }
    for line in &errors {
        println!("  error: {line}");
    }
    assert!(failures.is_empty(), "{failures:#?}");
}
