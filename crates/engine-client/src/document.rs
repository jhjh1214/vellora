//! Opening the document so that other processes cannot change it under the engine, and noticing
//! when they did anyway.
//!
//! The engine maps the file, so a writer that truncates it makes the engine fault (the client
//! restarts it, but the user loses their place). What the operating system lets us prevent differs:
//!
//! - **Windows** has mandatory sharing modes: the file is opened with `FILE_SHARE_READ` only, so
//!   no other process can open it for writing, deleting or renaming while it is open here.
//! - **Linux and macOS** only have advisory locks: a shared `flock` stops writers that take the
//!   exclusive lock first (well-behaved tools), and nothing stops the rest. Those are caught by
//!   [`Seen::check`], called at each engine restart.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::Path;
use std::time::SystemTime;

/// Opens `path` for reading with writers denied where the operating system allows it (see the
/// module documentation). Failing to take the advisory lock is not an error: some file systems
/// have no locks, and another process may hold the exclusive one; a viewer still opens the file.
///
/// # Errors
///
/// The operating system's error if the file cannot be opened.
pub(crate) fn open_read_only(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ: readers are welcome (other tabs, other viewers), writers are not.
        options.share_mode(0x1);
    }
    let file = options.open(path)?;
    #[cfg(unix)]
    let _ = os::lock_shared(&file);
    Ok(file)
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod os {
    use std::fs::File;
    use std::io;
    use std::os::fd::AsRawFd;

    /// Takes a shared, non-blocking advisory lock. The lock belongs to the open file description,
    /// which the engine inherits, and ends when the last descriptor closes.
    pub(super) fn lock_shared(file: &File) -> io::Result<()> {
        // SAFETY: `flock` only reads the descriptor number, which `file` keeps open for this call;
        // it touches no memory.
        if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_SH | libc::LOCK_NB) } == 0 {
            Ok(())
        } else {
            Err(io::Error::last_os_error())
        }
    }
}

/// What identifies a version of a file: its length and modification time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Fingerprint {
    len: u64,
    modified: Option<SystemTime>,
}

impl Fingerprint {
    fn of(meta: &fs::Metadata) -> Self {
        Self {
            len: meta.len(),
            modified: meta.modified().ok(),
        }
    }
}

/// How the file on disk differs from the version last seen.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Change {
    /// The open file was modified in place (its length or time changed).
    Modified,
    /// A different file is at the path now (replaced by a rename), or the path is gone.
    Replaced,
}

/// The versions of the open file and of whatever is at its path, as last reported to the user.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Seen {
    /// The file the engine maps.
    handle: Fingerprint,
    /// The file at the path. A writer that renames a new file over the document leaves our handle
    /// on the old contents, so only the path shows it. `None` if the path was unreadable.
    path: Option<Fingerprint>,
}

impl Seen {
    /// Records the versions now.
    pub(crate) fn new(file: &File, path: &Path) -> io::Result<Self> {
        Ok(Self {
            handle: Fingerprint::of(&file.metadata()?),
            path: fs::metadata(path).ok().as_ref().map(Fingerprint::of),
        })
    }

    /// Compares the file and the path with what was last seen, and remembers what they are now, so
    /// a change is reported once.
    pub(crate) fn check(&mut self, file: &File, path: &Path) -> Option<Change> {
        let handle = file.metadata().ok().as_ref().map(Fingerprint::of);
        let at_path = fs::metadata(path).ok().as_ref().map(Fingerprint::of);
        let change = self.compare(handle, at_path);
        if let Some(handle) = handle {
            self.handle = handle;
        }
        self.path = at_path;
        change
    }

    fn compare(&self, handle: Option<Fingerprint>, path: Option<Fingerprint>) -> Option<Change> {
        match (handle, path) {
            (Some(handle), _) if handle != self.handle => Some(Change::Modified),
            (None, _) => Some(Change::Modified),
            (Some(_), path) if path != self.path => Some(Change::Replaced),
            (Some(_), _) => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use super::*;

    fn fp(len: u64, seconds: u64) -> Fingerprint {
        Fingerprint {
            len,
            modified: Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
        }
    }

    fn seen(handle: Fingerprint) -> Seen {
        Seen {
            handle,
            path: Some(handle),
        }
    }

    #[test]
    fn an_unchanged_file_reports_nothing() {
        let a = fp(10, 5);
        assert_eq!(seen(a).compare(Some(a), Some(a)), None);
    }

    #[test]
    fn a_change_to_the_open_file_is_a_modification() {
        let seen = seen(fp(10, 5));
        let (shorter, later) = (fp(9, 5), fp(10, 6));
        assert_eq!(
            seen.compare(Some(shorter), Some(shorter)),
            Some(Change::Modified)
        );
        assert_eq!(
            seen.compare(Some(later), Some(later)),
            Some(Change::Modified)
        );
        assert_eq!(seen.compare(None, None), Some(Change::Modified));
    }

    #[test]
    fn a_different_file_at_the_path_is_a_replacement() {
        let a = fp(10, 5);
        let seen = seen(a);
        assert_eq!(
            seen.compare(Some(a), Some(fp(12, 9))),
            Some(Change::Replaced)
        );
        assert_eq!(seen.compare(Some(a), None), Some(Change::Replaced));
    }

    #[cfg(unix)]
    #[test]
    #[allow(unsafe_code)]
    fn on_unix_an_exclusive_lock_is_refused_while_the_document_is_open() {
        use std::os::fd::AsRawFd;

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        fs::write(&path, b"%PDF-1.4\n").unwrap();
        let exclusive = |file: &File| {
            // SAFETY: `flock` only reads the descriptor number, which `file` keeps open.
            unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) == 0 }
        };

        let document = open_read_only(&path).unwrap();
        // A separate open file description, as another process would have.
        let writer = OpenOptions::new().write(true).open(&path).unwrap();
        assert!(
            !exclusive(&writer),
            "a writer took the lock over the open document"
        );

        drop(document);
        assert!(exclusive(&writer), "the lock outlived the document");
    }

    #[test]
    fn a_real_file_is_reported_once_when_it_changes() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.pdf");
        fs::write(&path, b"%PDF-1.4\n").unwrap();
        let file = open_read_only(&path).unwrap();
        let mut seen = Seen::new(&file, &path).unwrap();
        assert_eq!(seen.check(&file, &path), None);

        // Windows refuses a writer while the file is open (tests/deadlines.rs); elsewhere an
        // append goes through and must be reported exactly once.
        #[cfg(unix)]
        {
            use std::io::Write;
            let mut writer = OpenOptions::new().append(true).open(&path).unwrap();
            writer.write_all(b"%%EOF\n").unwrap();
            assert_eq!(seen.check(&file, &path), Some(Change::Modified));
            assert_eq!(seen.check(&file, &path), None);
        }

        // Replacing the file by a rename leaves the handle on the old contents.
        let other = dir.path().join("b.pdf");
        fs::write(&other, b"%PDF-1.7\n%%EOF\n").unwrap();
        let moved = fs::rename(&other, &path);
        if cfg!(windows) {
            // No FILE_SHARE_DELETE: the open document cannot be renamed over.
            assert!(moved.is_err());
        } else {
            moved.unwrap();
            assert_eq!(seen.check(&file, &path), Some(Change::Replaced));
            assert_eq!(seen.check(&file, &path), None);
        }
    }
}
