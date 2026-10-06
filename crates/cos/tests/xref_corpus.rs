//! Differential test: object counts from `cos::Xref` against `qpdf --show-xref` on corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`) and qpdf (on `PATH`, or its path in the `QPDF`
//! environment variable), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test xref_corpus -- --ignored --nocapture
//! ```
//!
//! Only files that both tools read cleanly are compared: our parser must accept the classic
//! xref chain, and qpdf must exit 0 without warnings (a warning means qpdf repaired the file, so
//! its table is no longer the file's own). Files with cross-reference streams are skipped until
//! task 7, and so are hybrid-reference files (a trailer with `/XRefStm`, §7.5.8.4): qpdf also
//! counts the objects listed in the stream, which the classic table alone does not contain.
//! They are counted separately so task 7 can compare them.

use std::path::{Path, PathBuf};
use std::process::Command;

use vellora_cos::{Limits, Xref};

fn qpdf() -> String {
    std::env::var("QPDF").unwrap_or_else(|_| "qpdf".to_string())
}

/// Number of in-use objects according to `qpdf --show-xref`, `Ok(None)` if qpdf did not read the
/// file cleanly, and an error if qpdf cannot be run at all.
fn qpdf_count(file: &Path) -> std::io::Result<Option<usize>> {
    let output = Command::new(qpdf())
        .arg("--show-xref")
        .arg(file)
        .output()
        .map_err(|e| {
            std::io::Error::new(
                e.kind(),
                format!("cannot run qpdf (set QPDF to its path): {e}"),
            )
        })?;
    if !output.status.success() || !output.stderr.is_empty() {
        return Ok(None);
    }
    let text = String::from_utf8_lossy(&output.stdout);
    Ok(Some(
        text.lines()
            .filter(|l| {
                let Some((id, _)) = l.split_once(':') else {
                    return false;
                };
                id.split_once('/').is_some_and(|(n, g)| {
                    n.bytes().all(|b| b.is_ascii_digit()) && g.bytes().all(|b| b.is_ascii_digit())
                })
            })
            .count(),
    ))
}

#[test]
#[ignore = "needs the corpus and qpdf; run with --ignored"]
fn object_counts_match_qpdf_show_xref() {
    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data");
    assert!(
        dir.is_dir(),
        "corpus missing: run `cargo xtask corpus fetch`"
    );
    let limits = Limits::default();

    let mut files: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    files.sort();

    let (mut compared, mut not_classic, mut qpdf_unclean, mut hybrid) = (0, 0, 0, 0);
    let mut mismatches = Vec::new();
    for file in &files {
        let data = std::fs::read(file).unwrap();
        let Ok(xref) = Xref::parse(&data, &limits) else {
            not_classic += 1;
            continue;
        };
        if xref
            .revisions
            .iter()
            .any(|r| r.section.trailer.get(b"XRefStm").is_some())
        {
            hybrid += 1;
            continue;
        }
        let Some(expected) = qpdf_count(file).unwrap() else {
            qpdf_unclean += 1;
            continue;
        };
        compared += 1;
        let ours = xref.in_use_count();
        if ours != expected {
            mismatches.push(format!(
                "{}: cos {ours}, qpdf {expected}",
                file.file_name().unwrap().to_string_lossy()
            ));
        }
    }
    println!(
        "files {}: compared {compared}, not a classic xref chain for cos {not_classic}, hybrid {hybrid}, qpdf not clean {qpdf_unclean}",
        files.len()
    );
    assert!(
        compared >= 100,
        "only {compared} files compared; is the corpus complete and qpdf working?"
    );
    assert!(
        mismatches.is_empty(),
        "object count differs from qpdf:\n{}",
        mismatches.join("\n")
    );
}
