//! `cargo xtask corpus fetch`: download the corpus listed in the manifest and verify it.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use sha2::{Digest, Sha256};

use crate::Result;
use crate::manifest::{Doc, Manifest};

/// Upper bound for a single corpus file; guards against a hostile or misconfigured URL.
const MAX_FILE_BYTES: u64 = 512 * 1024 * 1024;

/// What `ensure` did for one document.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Already present with the right hash; nothing downloaded.
    Verified,
    /// Not present; downloaded and verified.
    Fetched,
    /// Present but corrupted; downloaded again and verified.
    Refetched,
}

/// Streams the body of a URL into the writer.
pub(crate) type Downloader<'a> = dyn FnMut(&str, &mut dyn Write) -> Result<()> + 'a;

pub(crate) fn file_path(dir: &Path, doc: &Doc) -> PathBuf {
    dir.join(format!("{}.pdf", doc.id))
}

fn sha256_hex(mut reader: impl Read) -> io::Result<String> {
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    Ok(hasher
        .finalize()
        .iter()
        .fold(String::with_capacity(64), |mut hex, b| {
            // Writing to a String cannot fail.
            let _ = write!(hex, "{b:02x}");
            hex
        }))
}

/// Makes sure `<dir>/<id>.pdf` exists and matches the manifest hash.
///
/// A download lands in a `.part` file first and only replaces the target after its hash
/// matches, so an interrupted or tampered download never leaves a bad file under the real name.
pub(crate) fn ensure(dir: &Path, doc: &Doc, download: &mut Downloader<'_>) -> Result<Outcome> {
    let target = file_path(dir, doc);
    let existed = match File::open(&target) {
        Ok(file) => {
            if sha256_hex(file)? == doc.sha256 {
                return Ok(Outcome::Verified);
            }
            true
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => false,
        Err(e) => return Err(e.into()),
    };

    let part = target.with_extension("pdf.part");
    if let Err(e) = download_verified(&part, doc, download) {
        let _ = fs::remove_file(&part);
        return Err(e);
    }
    fs::rename(&part, &target)?;
    Ok(if existed {
        Outcome::Refetched
    } else {
        Outcome::Fetched
    })
}

fn download_verified(part: &Path, doc: &Doc, download: &mut Downloader<'_>) -> Result<()> {
    let mut file = File::create(part)?;
    download(&doc.url, &mut file)?;
    file.flush()?;
    drop(file);
    let actual = sha256_hex(File::open(part)?)?;
    if actual != doc.sha256 {
        return Err(format!("sha256 mismatch (expected {}, got {actual})", doc.sha256).into());
    }
    Ok(())
}

/// Real downloader: `curl` over HTTPS only, with a size cap.
///
/// `curl` ships with Windows 10+, macOS and every mainstream Linux distribution. Using it keeps
/// a TLS stack (and its license surface under `deny.toml`) out of the dependency tree.
fn curl_download(url: &str, out: &mut dyn Write) -> Result<()> {
    let mut child = Command::new("curl")
        .args(["--fail", "--silent", "--show-error", "--location"])
        .args(["--proto", "=https", "--proto-redir", "=https"])
        .args([
            "--retry",
            "2",
            "--connect-timeout",
            "30",
            "--max-time",
            "600",
        ])
        .arg("--max-filesize")
        .arg(MAX_FILE_BYTES.to_string())
        .arg("--")
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("cannot run `curl` (is it installed and on PATH?): {e}"))?;

    let mut stdout = child.stdout.take().ok_or("curl stdout not captured")?;
    // Belt and braces: `--max-filesize` only works when the server announces a length.
    let copied = io::copy(&mut (&mut stdout).take(MAX_FILE_BYTES + 1), out)?;
    if copied > MAX_FILE_BYTES {
        let _ = child.kill();
        let _ = child.wait();
        return Err(format!("response larger than {MAX_FILE_BYTES} bytes").into());
    }
    let output = child.wait_with_output()?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("curl failed ({}): {}", output.status, stderr.trim()).into());
    }
    Ok(())
}

/// Entry point for `corpus fetch`. Returns the number of failed documents.
pub(crate) fn fetch(manifest_path: &Path, dir: &Path) -> Result<usize> {
    let text = fs::read_to_string(manifest_path)
        .map_err(|e| format!("cannot read {}: {e}", manifest_path.display()))?;
    let manifest = Manifest::parse(&text)?;
    fs::create_dir_all(dir)?;
    let mut download = curl_download;

    let (mut verified, mut fetched, mut failed) = (0usize, 0usize, 0usize);
    for doc in &manifest.doc {
        match ensure(dir, doc, &mut download) {
            Ok(Outcome::Verified) => verified += 1,
            Ok(outcome) => {
                fetched += 1;
                let verb = if outcome == Outcome::Fetched {
                    "fetched"
                } else {
                    "refetched"
                };
                println!("{verb:<9} {}", doc.id);
            }
            Err(e) => {
                failed += 1;
                eprintln!("FAILED    {}: {e}", doc.id);
            }
        }
    }
    println!(
        "corpus: {} documents, {verified} already verified, {fetched} downloaded, {failed} failed",
        manifest.doc.len()
    );
    Ok(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &[u8] = b"%PDF-1.4 test body";

    fn doc_for(body: &[u8]) -> Doc {
        Doc {
            id: "t".into(),
            url: "https://example.org/t.pdf".into(),
            sha256: sha256_hex(body).unwrap(),
            license: "MIT".into(),
            categories: vec!["x".into()],
            notes: String::new(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("vellora-xtask-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn serve<'a>(
        body: &'a [u8],
        calls: &'a mut u32,
    ) -> impl FnMut(&str, &mut dyn Write) -> Result<()> + 'a {
        move |_, out| {
            *calls += 1;
            out.write_all(body)?;
            Ok(())
        }
    }

    #[test]
    fn sha256_known_answer() {
        assert_eq!(
            sha256_hex(&b"abc"[..]).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }

    #[test]
    fn fetches_then_second_run_is_a_noop() {
        let dir = temp_dir("noop");
        let doc = doc_for(BODY);
        let mut calls = 0;
        assert_eq!(
            ensure(&dir, &doc, &mut serve(BODY, &mut calls)).unwrap(),
            Outcome::Fetched
        );
        assert_eq!(
            ensure(&dir, &doc, &mut serve(BODY, &mut calls)).unwrap(),
            Outcome::Verified
        );
        assert_eq!(calls, 1, "second run must not download");
        assert_eq!(fs::read(file_path(&dir, &doc)).unwrap(), BODY);
    }

    #[test]
    fn corrupted_file_is_detected_and_refetched() {
        let dir = temp_dir("corrupt");
        let doc = doc_for(BODY);
        fs::write(file_path(&dir, &doc), b"%PDF-1.4 tampered").unwrap();
        let mut calls = 0;
        assert_eq!(
            ensure(&dir, &doc, &mut serve(BODY, &mut calls)).unwrap(),
            Outcome::Refetched
        );
        assert_eq!(calls, 1);
        assert_eq!(fs::read(file_path(&dir, &doc)).unwrap(), BODY);
    }

    #[test]
    fn bad_download_is_rejected_and_leaves_no_file() {
        let dir = temp_dir("badhash");
        let doc = doc_for(BODY);
        let mut calls = 0;
        let err = ensure(&dir, &doc, &mut serve(b"something else", &mut calls)).unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
        assert!(!file_path(&dir, &doc).exists());
        assert!(!dir.join("t.pdf.part").exists());
    }

    #[test]
    fn failed_refetch_leaves_no_part_file_and_reports_error() {
        let dir = temp_dir("badrefetch");
        let doc = doc_for(BODY);
        fs::write(file_path(&dir, &doc), b"old").unwrap();
        let mut calls = 0;
        assert!(ensure(&dir, &doc, &mut serve(b"nope", &mut calls)).is_err());
        assert!(!dir.join("t.pdf.part").exists());
    }

    #[test]
    fn download_error_is_propagated() {
        let dir = temp_dir("dlerr");
        let doc = doc_for(BODY);
        let mut failing = |_: &str, _: &mut dyn Write| -> Result<()> { Err("network down".into()) };
        assert!(ensure(&dir, &doc, &mut failing).is_err());
        assert!(!dir.join("t.pdf.part").exists());
    }
}
