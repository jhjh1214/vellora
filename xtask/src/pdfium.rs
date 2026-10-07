//! `cargo xtask pdfium fetch`: download the pinned PDFium binaries and verify them.
//!
//! The pin lives in `third_party/pdfium.lock` (release, per-platform URL, archive SHA-256 and the
//! shared library's path inside the archive). Archives are extracted to
//! `third_party/pdfium/<platform>/`, which is git-ignored.

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

use serde::Deserialize;

use crate::Result;
use crate::corpus::{Downloader, curl_download, sha256_hex};

/// Only archives from this release page are accepted.
const URL_PREFIX: &str = "https://github.com/bblanchon/pdfium-binaries/releases/download/";

/// Written next to the extracted files: the archive hash and the library hash, one per line.
const STAMP: &str = ".vellora-sha256";

/// The parsed `pdfium.lock`.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Lock {
    /// Upstream release tag, informational (the URLs are authoritative).
    pub(crate) release: String,
    /// PDFium version string from the archive's `VERSION` file, informational.
    pub(crate) version: String,
    pub(crate) platform: BTreeMap<String, Platform>,
}

/// One platform's asset.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Platform {
    pub(crate) url: String,
    /// Lower-case hex SHA-256 of the `.tgz`.
    pub(crate) sha256: String,
    /// Shared library path inside the archive, relative, with `/` separators.
    pub(crate) library: String,
}

impl Lock {
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let lock: Self = toml::from_str(text)?;
        lock.validate()?;
        Ok(lock)
    }

    fn validate(&self) -> Result<()> {
        if self.release.trim().is_empty() || self.version.trim().is_empty() {
            return Err("release and version must not be empty".into());
        }
        if self.platform.is_empty() {
            return Err("no [platform.*] tables".into());
        }
        for (key, platform) in &self.platform {
            platform
                .validate(key)
                .map_err(|e| format!("platform {key}: {e}"))?;
        }
        Ok(())
    }
}

impl Platform {
    fn validate(&self, key: &str) -> Result<()> {
        let key_ok = !key.is_empty()
            && key
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-');
        if !key_ok {
            return Err("key must be lower-case letters, digits and '-'".into());
        }
        // The expected file name is exact, which also keeps the `pdfium-v8-*` assets out: the
        // security model requires a V8-free engine.
        let expected = format!("pdfium-{key}.tgz");
        let ok = self
            .url
            .strip_prefix(URL_PREFIX)
            .and_then(|rest| rest.rsplit_once('/'))
            .is_some_and(|(_, name)| name == expected);
        if !ok {
            return Err(format!("url must be {URL_PREFIX}<tag>/{expected}").into());
        }
        let hash_ok = self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hash_ok {
            return Err("sha256 must be 64 lower-case hex digits".into());
        }
        let lib = Path::new(&self.library);
        let lib_ok = !self.library.is_empty()
            && !self.library.contains('\\')
            && lib.is_relative()
            && lib
                .components()
                .all(|c| matches!(c, std::path::Component::Normal(_)));
        if !lib_ok {
            return Err("library must be a relative path with '/' separators and no '..'".into());
        }
        Ok(())
    }
}

/// The lock key for the machine running xtask, if PDFium binaries are pinned for it.
pub(crate) fn host_platform() -> Option<&'static str> {
    match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => Some("win-x64"),
        ("linux", "x86_64") => Some("linux-x64"),
        ("macos", "aarch64") => Some("mac-arm64"),
        ("macos", "x86_64") => Some("mac-x64"),
        _ => None,
    }
}

/// What `ensure` did.
#[derive(Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    /// Already extracted and the library still matches its recorded hash; nothing downloaded.
    Verified,
    /// Not present; downloaded, verified and extracted.
    Fetched,
    /// Present but incomplete or changed on disk; downloaded and extracted again.
    Refetched,
}

/// Unpacks a `.tgz`. Arguments: the directory to run in, then the archive file name and the
/// destination directory name, both relative to it.
pub(crate) type Extractor<'a> = dyn FnMut(&Path, &str, &str) -> Result<()> + 'a;

fn read_stamp(dir: &Path) -> Option<(String, String)> {
    let text = fs::read_to_string(dir.join(STAMP)).ok()?;
    let mut lines = text.lines();
    Some((lines.next()?.to_owned(), lines.next()?.to_owned()))
}

fn library_hash(dir: &Path, platform: &Platform) -> Option<String> {
    sha256_hex(File::open(dir.join(&platform.library)).ok()?).ok()
}

/// Makes sure `<root>/<key>/` holds the locked PDFium and that its library is untouched.
///
/// The archive lands in a `.part` file and is extracted into a `.tmp` directory; only after the
/// archive hash matched and the library is present does the new directory replace the old one,
/// so an interrupted or tampered download never leaves a half-filled platform directory.
pub(crate) fn ensure(
    root: &Path,
    key: &str,
    platform: &Platform,
    download: &mut Downloader<'_>,
    extract: &mut Extractor<'_>,
) -> Result<Outcome> {
    let dir = root.join(key);
    let existed = dir.exists();
    if let Some((archive, library)) = read_stamp(&dir)
        && archive == platform.sha256
        && library_hash(&dir, platform).as_deref() == Some(&library)
    {
        return Ok(Outcome::Verified);
    }

    fs::create_dir_all(root)?;
    let part_name = format!("{key}.tgz.part");
    let tmp_name = format!("{key}.tmp");
    let result = install(
        root, &dir, &part_name, &tmp_name, platform, download, extract,
    );
    let _ = fs::remove_file(root.join(&part_name));
    let _ = fs::remove_dir_all(root.join(&tmp_name));
    result?;
    Ok(if existed {
        Outcome::Refetched
    } else {
        Outcome::Fetched
    })
}

fn install(
    root: &Path,
    dir: &Path,
    part_name: &str,
    tmp_name: &str,
    platform: &Platform,
    download: &mut Downloader<'_>,
    extract: &mut Extractor<'_>,
) -> Result<()> {
    let part = root.join(part_name);
    let tmp = root.join(tmp_name);
    let mut file = File::create(&part)?;
    download(&platform.url, &mut file)?;
    io::Write::flush(&mut file)?;
    drop(file);
    let actual = sha256_hex(File::open(&part)?)?;
    if actual != platform.sha256 {
        return Err(format!(
            "sha256 mismatch (expected {}, got {actual})",
            platform.sha256
        )
        .into());
    }

    let _ = fs::remove_dir_all(&tmp);
    fs::create_dir_all(&tmp)?;
    extract(root, part_name, tmp_name)?;
    let library = library_hash(&tmp, platform)
        .ok_or_else(|| format!("archive does not contain {}", platform.library))?;
    fs::write(tmp.join(STAMP), format!("{}\n{library}\n", platform.sha256))?;

    if dir.exists() {
        fs::remove_dir_all(dir)?;
    }
    fs::rename(&tmp, dir)?;
    Ok(())
}

/// Real extractor: the system `tar` (bsdtar on Windows 10+ and macOS, GNU tar on Linux).
///
/// Runs inside `root` with relative arguments: GNU tar reads `C:\...` as `host:path`.
fn tar_extract(root: &Path, archive: &str, dest: &str) -> Result<()> {
    let output = Command::new("tar")
        .current_dir(root)
        .args(["-xzf", archive, "-C", dest])
        .output()
        .map_err(|e| format!("cannot run `tar` (is it installed and on PATH?): {e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("tar failed ({}): {}", output.status, stderr.trim()).into());
    }
    Ok(())
}

/// Entry point for `pdfium fetch`. `platform` defaults to the host's.
pub(crate) fn fetch(lock_path: &Path, root: &Path, platform: Option<&str>) -> Result<()> {
    let text = fs::read_to_string(lock_path)
        .map_err(|e| format!("cannot read {}: {e}", lock_path.display()))?;
    let lock = Lock::parse(&text)?;
    let key = match platform {
        Some(key) => key,
        None => host_platform().ok_or_else(|| {
            format!(
                "no PDFium binaries are pinned for {}-{}; pass --platform <key> (known: {})",
                std::env::consts::OS,
                std::env::consts::ARCH,
                lock.platform.keys().cloned().collect::<Vec<_>>().join(", ")
            )
        })?,
    };
    let entry = lock
        .platform
        .get(key)
        .ok_or_else(|| format!("platform {key:?} is not in {}", lock_path.display()))?;

    let outcome = ensure(root, key, entry, &mut curl_download, &mut tar_extract)?;
    let verb = match outcome {
        Outcome::Verified => "already verified",
        Outcome::Fetched => "downloaded",
        Outcome::Refetched => "downloaded again",
    };
    println!(
        "pdfium {} ({}): {key} {verb}: {}",
        lock.version,
        lock.release,
        root.join(key).join(&entry.library).display()
    );
    Ok(())
}

/// Default install root, `<workspace>/third_party/pdfium`.
pub(crate) fn default_root(workspace: &Path) -> PathBuf {
    workspace.join("third_party/pdfium")
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use super::*;

    const ARCHIVE: &[u8] = b"pretend this is a tarball";
    const LIB: &[u8] = b"pretend this is a shared library";

    fn platform_for(archive: &[u8]) -> Platform {
        Platform {
            url: format!("{URL_PREFIX}chromium%2F1/pdfium-test-x64.tgz"),
            sha256: sha256_hex(archive).unwrap(),
            library: "lib/libpdfium.so".into(),
        }
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("vellora-pdfium-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Counts calls; the fake extractor writes `lib/libpdfium.so` (or not).
    struct Fake {
        downloads: u32,
        extracts: u32,
        body: &'static [u8],
        lib: Option<&'static [u8]>,
    }

    impl Fake {
        fn new() -> Self {
            Self {
                downloads: 0,
                extracts: 0,
                body: ARCHIVE,
                lib: Some(LIB),
            }
        }

        fn run(&mut self, root: &Path, key: &str, platform: &Platform) -> Result<Outcome> {
            let (body, lib) = (self.body, self.lib);
            let (downloads, extracts) = (&mut self.downloads, &mut self.extracts);
            ensure(
                root,
                key,
                platform,
                &mut |_, out| {
                    *downloads += 1;
                    out.write_all(body)?;
                    Ok(())
                },
                &mut |root, _, dest| {
                    *extracts += 1;
                    if let Some(lib) = lib {
                        let dir = root.join(dest).join("lib");
                        fs::create_dir_all(&dir)?;
                        fs::write(dir.join("libpdfium.so"), lib)?;
                    }
                    Ok(())
                },
            )
        }
    }

    #[test]
    fn repository_lock_is_valid_and_covers_the_ci_platforms() {
        let text = fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../third_party/pdfium.lock"),
        )
        .unwrap();
        let lock = Lock::parse(&text).unwrap();
        for key in ["win-x64", "linux-x64", "mac-arm64"] {
            assert!(lock.platform.contains_key(key), "{key} missing");
        }
    }

    fn lock_text(url: &str, sha: &str, library: &str) -> String {
        format!(
            "release = \"r\"\nversion = \"1\"\n[platform.win-x64]\nurl = \"{url}\"\n\
             sha256 = \"{sha}\"\nlibrary = \"{library}\"\n"
        )
    }

    #[test]
    fn lock_validation_rejects_bad_entries() {
        let good_url = format!("{URL_PREFIX}chromium%2F1/pdfium-win-x64.tgz");
        let v8_url = format!("{URL_PREFIX}chromium%2F1/pdfium-v8-win-x64.tgz");
        let sha = "0".repeat(64);
        let upper = "A".repeat(64);
        Lock::parse(&lock_text(&good_url, &sha, "bin/pdfium.dll")).unwrap();

        let cases = [
            (&v8_url, &sha, "bin/pdfium.dll", "v8 asset"),
            (
                &"http://example.org/pdfium-win-x64.tgz".to_owned(),
                &sha,
                "bin/a.dll",
                "http",
            ),
            (
                &"https://example.org/pdfium-win-x64.tgz".to_owned(),
                &sha,
                "bin/a.dll",
                "foreign host",
            ),
            (&good_url, &"abc".to_owned(), "bin/pdfium.dll", "short hash"),
            (&good_url, &upper, "bin/pdfium.dll", "upper-case hash"),
            (&good_url, &sha, "../pdfium.dll", "parent dir"),
            (&good_url, &sha, "/etc/pdfium.dll", "absolute"),
            (&good_url, &sha, "bin\\pdfium.dll", "backslash"),
        ];
        for (url, sha, library, what) in cases {
            assert!(
                Lock::parse(&lock_text(url, sha, library)).is_err(),
                "{what}"
            );
        }
    }

    #[test]
    fn fetches_then_second_run_is_a_noop() {
        let root = temp_dir("noop");
        let platform = platform_for(ARCHIVE);
        let mut f = Fake::new();
        assert_eq!(f.run(&root, "t", &platform).unwrap(), Outcome::Fetched);
        assert_eq!(f.run(&root, "t", &platform).unwrap(), Outcome::Verified);
        assert_eq!(
            (f.downloads, f.extracts),
            (1, 1),
            "second run must not download"
        );
        assert_eq!(fs::read(root.join("t/lib/libpdfium.so")).unwrap(), LIB);
        assert!(!root.join("t.tgz.part").exists() && !root.join("t.tmp").exists());
    }

    #[test]
    fn modified_library_is_detected_and_replaced() {
        let root = temp_dir("tamper");
        let platform = platform_for(ARCHIVE);
        let mut f = Fake::new();
        f.run(&root, "t", &platform).unwrap();
        fs::write(root.join("t/lib/libpdfium.so"), b"tampered").unwrap();
        assert_eq!(f.run(&root, "t", &platform).unwrap(), Outcome::Refetched);
        assert_eq!(fs::read(root.join("t/lib/libpdfium.so")).unwrap(), LIB);
    }

    #[test]
    fn missing_library_or_new_pin_triggers_a_refetch() {
        let root = temp_dir("repin");
        let mut f = Fake::new();
        f.run(&root, "t", &platform_for(ARCHIVE)).unwrap();
        fs::remove_file(root.join("t/lib/libpdfium.so")).unwrap();
        assert_eq!(
            f.run(&root, "t", &platform_for(ARCHIVE)).unwrap(),
            Outcome::Refetched
        );

        // The lock now pins a different archive: the old extraction must not count as verified.
        let mut g = Fake::new();
        g.body = b"a newer tarball";
        let repinned = platform_for(g.body);
        assert_eq!(g.run(&root, "t", &repinned).unwrap(), Outcome::Refetched);
        assert_eq!(g.downloads, 1);
    }

    #[test]
    fn bad_download_is_rejected_and_leaves_nothing() {
        let root = temp_dir("badhash");
        let mut f = Fake::new();
        f.body = b"something else";
        let err = f.run(&root, "t", &platform_for(ARCHIVE)).unwrap_err();
        assert!(err.to_string().contains("sha256 mismatch"), "{err}");
        assert_eq!(
            f.extracts, 0,
            "an unverified archive must never be extracted"
        );
        assert!(!root.join("t").exists());
        assert!(!root.join("t.tgz.part").exists() && !root.join("t.tmp").exists());
    }

    #[test]
    fn archive_without_the_library_is_an_error_and_keeps_the_old_install() {
        let root = temp_dir("nolib");
        let platform = platform_for(ARCHIVE);
        Fake::new().run(&root, "t", &platform).unwrap();
        fs::write(root.join("t/lib/libpdfium.so"), b"tampered").unwrap();

        let mut f = Fake::new();
        f.lib = None;
        let err = f.run(&root, "t", &platform).unwrap_err();
        assert!(err.to_string().contains("does not contain"), "{err}");
        assert_eq!(
            fs::read(root.join("t/lib/libpdfium.so")).unwrap(),
            b"tampered"
        );
        assert!(!root.join("t.tmp").exists());
    }

    #[test]
    fn download_and_extract_errors_are_propagated_and_clean_up() {
        let root = temp_dir("errors");
        let platform = platform_for(ARCHIVE);
        let mut failing = |_: &str, _: &mut dyn Write| -> Result<()> { Err("network down".into()) };
        let mut never = |_: &Path, _: &str, _: &str| -> Result<()> { Ok(()) };
        assert!(ensure(&root, "t", &platform, &mut failing, &mut never).is_err());

        let mut serve = |_: &str, out: &mut dyn Write| -> Result<()> {
            out.write_all(ARCHIVE)?;
            Ok(())
        };
        let mut broken = |_: &Path, _: &str, _: &str| -> Result<()> { Err("tar failed".into()) };
        assert!(ensure(&root, "t", &platform, &mut serve, &mut broken).is_err());
        assert!(!root.join("t.tgz.part").exists() && !root.join("t.tmp").exists());
        assert!(!root.join("t").exists());
    }
}
