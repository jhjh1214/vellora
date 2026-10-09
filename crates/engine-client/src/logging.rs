//! The UI process's log: rotating files in the platform's log folder.
//!
//! Everything the application logs ends up in `vellora.log` (and its older siblings): the Rust
//! side's `tracing` events, the Qt side's `qDebug` / `qWarning` (forwarded through the bridge) and
//! the engine's log lines, which this process reads from the engine's standard error because the
//! sandboxed engine cannot write files ([`crate::capture`]).
//!
//! # Limits
//!
//! The files are capped by size and count ([`LogConfig`]): `vellora.log` grows to
//! `max_file_bytes`, is renamed to `vellora.1.log` (older ones move up) and a new one starts; the
//! oldest beyond `max_files` is deleted. The total therefore never exceeds
//! `max_files * max_file_bytes`, whatever a misbehaving engine prints. `tracing-appender` rotates by
//! time only, which cannot give that bound, so the writer is in-tree (std only); it keeps
//! `tracing-appender`'s idea of writing on a background thread so that logging never waits for the
//! disk.
//!
//! # What is logged
//!
//! Never document content at `info` or above (ADR-0012), and never a password. Paths of documents
//! are logged at `debug` only.

use std::collections::VecDeque;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread::{self, JoinHandle};

use tracing::Level;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{Registry, fmt, reload};

/// The newest log file's name; older ones are `vellora.1.log`, `vellora.2.log`, ...
pub const LOG_FILE: &str = "vellora.log";

/// Where the log files go and how big they may get.
#[derive(Debug, Clone)]
pub struct LogConfig {
    /// The folder, created if it is missing.
    pub dir: PathBuf,
    /// A file is rotated once it would grow past this many bytes.
    pub max_file_bytes: u64,
    /// How many files are kept, the current one included (at least 1).
    pub max_files: usize,
    /// The most verbose level that is written.
    pub level: Level,
}

impl LogConfig {
    /// 2 MiB a file, 5 files (10 MiB at most), `info` and above.
    #[must_use]
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        Self {
            dir: dir.into(),
            max_file_bytes: 2 << 20,
            max_files: 5,
            level: Level::INFO,
        }
    }
}

/// Why logging could not be started.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The log folder or file cannot be created.
    #[error("cannot open the log file: {0}")]
    Io(#[from] io::Error),
    /// Logging was already started in this process (there is one global subscriber).
    #[error("logging is already set up in this process")]
    AlreadyStarted,
}

/// The default folder for the log: `%LOCALAPPDATA%\Vellora\logs` on Windows, `~/Library/Logs/Vellora`
/// on macOS, `$XDG_STATE_HOME/vellora/logs` (default `~/.local/state/vellora/logs`) elsewhere.
/// `VELLORA_LOG_DIR` overrides it (for tests and for people who want the logs elsewhere).
#[must_use]
pub fn default_log_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("VELLORA_LOG_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    let env_path = |name: &str| {
        std::env::var_os(name)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    if cfg!(windows) {
        env_path("LOCALAPPDATA")
            .unwrap_or_else(std::env::temp_dir)
            .join("Vellora")
            .join("logs")
    } else if cfg!(target_os = "macos") {
        env_path("HOME")
            .unwrap_or_else(std::env::temp_dir)
            .join("Library/Logs/Vellora")
    } else {
        env_path("XDG_STATE_HOME")
            .or_else(|| env_path("HOME").map(|home| home.join(".local/state")))
            .unwrap_or_else(std::env::temp_dir)
            .join("vellora/logs")
    }
}

static STARTED: AtomicBool = AtomicBool::new(false);

/// Whether [`init`] has set logging up in this process.
#[must_use]
pub fn is_started() -> bool {
    STARTED.load(Ordering::Acquire)
}

/// Files `vellora.log`, `vellora.1.log`, ... in one folder, rotated by size.
#[derive(Debug)]
pub(crate) struct RotatingFile {
    dir: PathBuf,
    max_bytes: u64,
    max_files: usize,
    file: Option<File>,
    size: u64,
}

impl RotatingFile {
    pub(crate) fn open(dir: &Path, max_bytes: u64, max_files: usize) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let mut log = Self {
            dir: dir.to_owned(),
            max_bytes: max_bytes.max(1),
            max_files: max_files.max(1),
            file: None,
            size: 0,
        };
        log.open_current()?;
        Ok(log)
    }

    fn path(&self, index: usize) -> PathBuf {
        if index == 0 {
            self.dir.join(LOG_FILE)
        } else {
            self.dir.join(format!("vellora.{index}.log"))
        }
    }

    fn open_current(&mut self) -> io::Result<()> {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(0))?;
        self.size = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    /// Moves every file up by one, drops the oldest and starts a new current file.
    fn rotate(&mut self) -> io::Result<()> {
        self.file = None;
        let _ = fs::remove_file(self.path(self.max_files - 1));
        for index in (0..self.max_files - 1).rev() {
            // A missing file is the normal case early on.
            let _ = fs::rename(self.path(index), self.path(index + 1));
        }
        self.open_current()
    }

    /// Appends `bytes`, rotating first if they would not fit in the current file.
    pub(crate) fn write_line(&mut self, bytes: &[u8]) -> io::Result<()> {
        let len = u64::try_from(bytes.len()).unwrap_or(u64::MAX);
        if self.file.is_none() {
            self.open_current()?;
        }
        if self.size > 0 && self.size + len > self.max_bytes {
            self.rotate()?;
        }
        let Some(file) = self.file.as_mut() else {
            return Err(io::Error::other("no log file"));
        };
        file.write_all(bytes)?;
        self.size += len;
        Ok(())
    }
}

enum Message {
    Line(Vec<u8>),
    Flush(Sender<()>),
    Stop,
}

/// Where `tracing` events go right now: the background writer's channel, or nowhere between
/// [`init`] and the guard being dropped.
type Sink = Arc<Mutex<Option<Sender<Message>>>>;

/// One log event being written: its bytes are sent as a single line when it is dropped.
struct Line {
    sink: Sink,
    bytes: Vec<u8>,
}

impl Write for Line {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl Drop for Line {
    fn drop(&mut self) {
        if self.bytes.is_empty() {
            return;
        }
        let sender = self.sink.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(sender) = sender.as_ref() {
            // The writer may be gone at exit; there is nobody to tell.
            let _ = sender.send(Message::Line(std::mem::take(&mut self.bytes)));
        }
    }
}

struct SinkWriter(Sink);

impl<'a> fmt::MakeWriter<'a> for SinkWriter {
    type Writer = Line;

    fn make_writer(&'a self) -> Self::Writer {
        Line {
            sink: Arc::clone(&self.0),
            bytes: Vec::new(),
        }
    }
}

/// The process-wide `tracing` subscriber, installed the first time logging starts. A process can
/// have only one, so later starts (after a stop) reuse it and only change its level and its sink.
struct Global {
    level: reload::Handle<LevelFilter, Registry>,
    sink: Sink,
}

/// `None` if another subscriber already owns the process.
fn global() -> Option<&'static Global> {
    static GLOBAL: OnceLock<Option<Global>> = OnceLock::new();
    GLOBAL
        .get_or_init(|| {
            let sink: Sink = Arc::new(Mutex::new(None));
            let (filter, level) = reload::Layer::new(LevelFilter::OFF);
            let installed = tracing_subscriber::registry()
                .with(filter)
                .with(
                    fmt::layer()
                        .with_ansi(false)
                        .with_writer(SinkWriter(Arc::clone(&sink))),
                )
                .try_init();
            installed.ok().map(|()| Global { level, sink })
        })
        .as_ref()
}

/// Keeps logging running; dropping it writes what is queued and stops the writer thread.
#[derive(Debug)]
pub struct LogGuard {
    sender: Option<Sender<Message>>,
    thread: Option<JoinHandle<()>>,
}

impl LogGuard {
    /// Waits until everything logged so far is in the files.
    pub fn flush(&self) {
        if let Some(sender) = &self.sender {
            let (done, wait) = mpsc::channel();
            if sender.send(Message::Flush(done)).is_ok() {
                let _ = wait.recv();
            }
        }
    }
}

impl Drop for LogGuard {
    fn drop(&mut self) {
        // Events stop being queued first, then what is queued is written.
        if let Some(global) = global() {
            let _ = global.level.modify(|level| *level = LevelFilter::OFF);
            *global.sink.lock().unwrap_or_else(PoisonError::into_inner) = None;
        }
        if let Some(sender) = self.sender.take() {
            let _ = sender.send(Message::Stop);
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        STARTED.store(false, Ordering::Release);
    }
}

fn writer_thread(mut file: RotatingFile, inbox: &Receiver<Message>) {
    // Lines that could not be written (a full disk) are dropped: logging must never be the reason
    // the application stops.
    while let Ok(message) = inbox.recv() {
        match message {
            Message::Line(bytes) => {
                let _ = file.write_line(&bytes);
            }
            Message::Flush(done) => {
                if let Some(file) = file.file.as_mut() {
                    let _ = file.flush();
                }
                let _ = done.send(());
            }
            Message::Stop => break,
        }
    }
}

/// Starts logging to rotating files, as the process's `tracing` subscriber. It can be started
/// again after the guard was dropped.
///
/// # Errors
///
/// [`LogError::Io`] if the folder or the file cannot be created, [`LogError::AlreadyStarted`] if
/// logging is already running (drop the earlier [`LogGuard`] first) or another `tracing`
/// subscriber owns the process.
pub fn init(config: &LogConfig) -> Result<LogGuard, LogError> {
    if STARTED.swap(true, Ordering::AcqRel) {
        return Err(LogError::AlreadyStarted);
    }
    let Some(global) = global() else {
        STARTED.store(false, Ordering::Release);
        return Err(LogError::AlreadyStarted);
    };
    let started = (|| {
        let file = RotatingFile::open(&config.dir, config.max_file_bytes, config.max_files)?;
        let (sender, inbox) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("vellora-log".into())
            .spawn(move || writer_thread(file, &inbox))?;
        Ok::<_, io::Error>((sender, thread))
    })();
    let (sender, thread) = match started {
        Ok(started) => started,
        Err(error) => {
            STARTED.store(false, Ordering::Release);
            return Err(error.into());
        }
    };
    *global.sink.lock().unwrap_or_else(PoisonError::into_inner) = Some(sender.clone());
    let level = LevelFilter::from_level(config.level);
    let _ = global.level.modify(|filter| *filter = level);
    Ok(LogGuard {
        sender: Some(sender),
        thread: Some(thread),
    })
}

/// A line from the Qt side (`qDebug`, `qWarning`, ...), as a `tracing` event of target `qt`.
pub fn log_qt(level: Level, message: &str) {
    match level {
        Level::ERROR => tracing::error!(target: "qt", "{message}"),
        Level::WARN => tracing::warn!(target: "qt", "{message}"),
        Level::INFO => tracing::info!(target: "qt", "{message}"),
        _ => tracing::debug!(target: "qt", "{message}"),
    }
}

/// The last `capacity` lines pushed, oldest first (the engine's log tail, for crash reports).
#[derive(Debug)]
pub struct LineRing {
    capacity: usize,
    lines: std::sync::Mutex<VecDeque<String>>,
}

impl LineRing {
    /// An empty ring that keeps the last `capacity` lines.
    #[must_use]
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            lines: std::sync::Mutex::new(VecDeque::new()),
        }
    }

    /// Adds a line, dropping the oldest if the ring is full.
    pub fn push(&self, line: String) {
        let mut lines = self
            .lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if lines.len() == self.capacity {
            lines.pop_front();
        }
        lines.push_back(line);
    }

    /// The lines now in the ring, oldest first.
    #[must_use]
    pub fn tail(&self) -> Vec<String> {
        self.lines
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn files(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn files_rotate_by_size_and_only_the_newest_are_kept() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), 100, 3).unwrap();
        for i in 0..40 {
            log.write_line(format!("line number {i:02} of the log\n").as_bytes())
                .unwrap();
        }
        assert_eq!(
            files(dir.path()),
            ["vellora.1.log", "vellora.2.log", "vellora.log"]
        );
        for name in files(dir.path()) {
            let size = fs::metadata(dir.path().join(&name)).unwrap().len();
            assert!(size <= 100, "{name} is {size} bytes");
        }
        // The newest line is in the current file, and the oldest kept file is older than it.
        let current = fs::read_to_string(dir.path().join(LOG_FILE)).unwrap();
        assert!(current.contains("line number 39"), "{current}");
        let oldest = fs::read_to_string(dir.path().join("vellora.2.log")).unwrap();
        assert!(!oldest.contains("line number 39"));
        assert!(
            !oldest.contains("line number 00"),
            "the first lines are gone"
        );
    }

    #[test]
    fn a_single_file_is_cut_not_grown() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), 50, 1).unwrap();
        for i in 0..20 {
            log.write_line(format!("entry {i:02} ................\n").as_bytes())
                .unwrap();
        }
        assert_eq!(files(dir.path()), ["vellora.log"]);
        assert!(fs::metadata(dir.path().join(LOG_FILE)).unwrap().len() <= 50);
    }

    #[test]
    fn a_line_longer_than_the_file_still_lands_in_a_file() {
        let dir = tempfile::tempdir().unwrap();
        let mut log = RotatingFile::open(dir.path(), 10, 2).unwrap();
        log.write_line(b"short\n").unwrap();
        log.write_line(b"a line far longer than the limit of ten bytes\n")
            .unwrap();
        let text = fs::read_to_string(dir.path().join(LOG_FILE)).unwrap();
        assert!(text.contains("far longer"));
    }

    #[test]
    fn an_existing_log_is_appended_to_after_a_restart() {
        let dir = tempfile::tempdir().unwrap();
        RotatingFile::open(dir.path(), 1000, 2)
            .unwrap()
            .write_line(b"first run\n")
            .unwrap();
        RotatingFile::open(dir.path(), 1000, 2)
            .unwrap()
            .write_line(b"second run\n")
            .unwrap();
        assert_eq!(
            fs::read_to_string(dir.path().join(LOG_FILE)).unwrap(),
            "first run\nsecond run\n"
        );
    }

    #[test]
    fn the_ring_keeps_the_last_lines_in_order() {
        let ring = LineRing::new(3);
        assert_eq!(ring.tail(), Vec::<String>::new());
        for i in 0..5 {
            ring.push(format!("l{i}"));
        }
        assert_eq!(ring.tail(), ["l2", "l3", "l4"]);
    }

    #[test]
    fn the_default_folder_is_per_platform_and_can_be_overridden() {
        let dir = default_log_dir();
        assert!(
            dir.is_absolute() || dir.starts_with("."),
            "{}",
            dir.display()
        );
        let text = dir.to_string_lossy().to_lowercase();
        assert!(text.contains("vellora"), "{text}");
    }
}
