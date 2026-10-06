//! Differential test: object counts from `cos::Xref` against `qpdf --show-xref` on corpus v0.
//!
//! Needs the corpus (`cargo xtask corpus fetch`) and qpdf (on `PATH`, or its path in the `QPDF`
//! environment variable), so it only runs on request:
//!
//! ```sh
//! cargo test -p vellora-cos --test xref_corpus -- --ignored --nocapture
//! ```
//!
//! Classic tables, cross-reference streams and hybrid-reference files are all compared; the count
//! is the number of objects in use, compressed ones included. Only files that both tools read
//! cleanly are compared: our parser must accept the xref chain, and qpdf must exit 0 without
//! warnings (a warning means qpdf repaired the file, so its table is no longer the file's own).
//! Files `cos` rejects (damaged or needing recovery, M0 task 8) are counted and listed.

use std::path::{Path, PathBuf};
use std::process::Command;

use vellora_cos::{Limits, SectionKind, Xref};

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

    let (mut compared, mut qpdf_unclean, mut streams, mut hybrid) = (0, 0, 0, 0);
    let mut rejected: Vec<String> = Vec::new();
    let mut mismatches = Vec::new();
    for file in &files {
        let data = std::fs::read(file).unwrap();
        let name = file.file_name().unwrap().to_string_lossy().to_string();
        let xref = match Xref::parse(&data, &limits) {
            Ok(xref) => xref,
            Err(e) => {
                rejected.push(format!("{name}: {e}"));
                continue;
            }
        };
        if xref
            .revisions
            .iter()
            .any(|r| r.section.kind == SectionKind::Stream)
        {
            streams += 1;
        }
        if xref
            .revisions
            .iter()
            .any(|r| r.section.trailer.get(b"XRefStm").is_some())
        {
            hybrid += 1;
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
        "files {}: compared {compared} (with xref streams: {streams}, hybrid: {hybrid}),          cos rejected {}, qpdf not clean {qpdf_unclean}",
        files.len(),
        rejected.len()
    );
    for line in &rejected {
        println!("  rejected: {line}");
    }
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

/// Every compressed entry of every corpus file resolves: the object stream is found through the
/// cross-reference, decodes, lists the object at the promised index, and that object parses.
///
/// Encrypted files and filters other than Flate (task 10/11) cannot be decoded yet; they are
/// counted, not failed. Anything else that fails is a bug.
#[test]
#[ignore = "needs the corpus; run with --ignored"]
fn compressed_objects_resolve_through_object_streams() {
    use vellora_cos::{Error, ObjectKind, ObjectStream, Parser, XrefEntry};

    let dir: PathBuf = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/corpus-data");
    assert!(
        dir.is_dir(),
        "corpus missing: run `cargo xtask corpus fetch`"
    );
    let limits = Limits::default();

    let (mut resolved, mut skipped_files, mut files_with_objstm) = (0usize, 0usize, 0usize);
    let mut failures = Vec::new();
    let mut paths: Vec<_> = std::fs::read_dir(&dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.extension().is_some_and(|e| e == "pdf"))
        .collect();
    paths.sort();
    for path in paths {
        let data = std::fs::read(&path).unwrap();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let Ok(xref) = Xref::parse(&data, &limits) else {
            continue;
        };
        let merged = xref.merged();
        let mut compressed: Vec<(u32, u32, u32)> = merged
            .iter()
            .filter_map(|(&n, e)| match e {
                XrefEntry::Compressed { stream, index } => Some((n, *stream, *index)),
                _ => None,
            })
            .collect();
        if compressed.is_empty() {
            continue;
        }
        files_with_objstm += 1;
        compressed.sort_unstable();
        let encrypted = xref.trailer().is_some_and(|t| t.get(b"Encrypt").is_some());

        let mut cache: std::collections::HashMap<u32, Result<ObjectStream, String>> =
            std::collections::HashMap::default();
        let mut file_failures = Vec::new();
        let mut file_skipped = false;
        for (number, stream_number, index) in compressed {
            let stream = cache.entry(stream_number).or_insert_with(|| {
                let Some(XrefEntry::InUse { offset, .. }) = merged.get(&stream_number) else {
                    return Err(format!("object stream {stream_number} is not in use"));
                };
                let pos = usize::try_from(*offset).unwrap() + xref.base;
                let object = Parser::at(&data, pos, &limits)
                    .parse_indirect_object()
                    .map_err(|e| format!("object stream {stream_number}: {e}"))?;
                let ObjectKind::Stream(s) = &object.object.kind else {
                    return Err(format!("object {stream_number} is not a stream"));
                };
                let raw = &data[s.data.clone()];
                ObjectStream::from_stream(&s.dict, raw, &limits, None, Some(s.data.start as u64))
                    .map_err(|e| match e {
                        Error::Decode { .. } => format!("SKIP {e}"),
                        other => format!("object stream {stream_number}: {other}"),
                    })
            });
            match stream {
                Err(message) if message.starts_with("SKIP") => file_skipped = true,
                Err(message) => file_failures.push(message.clone()),
                Ok(stream) => {
                    let idx = usize::try_from(index).unwrap();
                    if stream.object_number(idx) != Some(number) {
                        file_failures.push(format!(
                            "object {number}: stream {stream_number} index {index} holds {:?}",
                            stream.object_number(idx)
                        ));
                    } else if let Err(e) = stream.object(idx, &limits) {
                        file_failures.push(format!("object {number}: {e}"));
                    } else {
                        resolved += 1;
                    }
                }
            }
        }
        if file_skipped && file_failures.is_empty() {
            skipped_files += 1;
        }
        // Encrypted object streams cannot decode until task 11; their failures are expected.
        if !encrypted {
            failures.extend(file_failures.into_iter().map(|f| format!("{name}: {f}")));
        }
    }
    println!(
        "{files_with_objstm} files use object streams; {resolved} compressed objects resolved; \
         {skipped_files} files skipped (filter beyond Flate)"
    );
    assert!(
        resolved >= 1000,
        "only {resolved} compressed objects resolved"
    );
    assert!(
        failures.is_empty(),
        "compressed objects failed to resolve:\n{}",
        failures.join("\n")
    );
}
