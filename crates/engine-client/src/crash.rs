//! Local crash reports (ADR-0019): minidumps of the UI process and records of engine incidents.
//!
//! - [`install`] starts the **monitor**, another run of the same executable (`--crash-monitor`),
//!   connects to it and attaches a crash handler to this process. When this process crashes, the
//!   handler asks the monitor to write a minidump and waits for it; the monitor writes
//!   `crash-<time>.dmp` and a text file beside it. The monitor exits when this process goes away.
//! - [`write_engine_report`] records that the sandboxed engine ended unasked or reported an internal
//!   error, with the last lines it printed. The engine cannot write files, so the UI does.
//!
//! Nothing here uses the network and nothing is uploaded. Files go to [`default_crash_dir`] (inside
//! the log folder). At most [`KEEP_REPORTS`] are kept.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use crash_handler::{CrashEventResult, CrashHandler, make_crash_event};
use minidumper::{Client, LoopAction, MinidumpBinary, Server, ServerHandler, SocketName};

/// The command-line flag that makes the executable run as the monitor instead of the application.
pub const MONITOR_FLAG: &str = "--crash-monitor";

/// Most crash reports (dumps and engine reports) kept in the folder; the oldest go first.
pub const KEEP_REPORTS: usize = 20;

/// Message kind carrying the description (version, commit, system) the monitor puts beside a dump.
const MESSAGE_INFO: u32 = 1;
/// Longest description accepted from the application.
const MAX_INFO_BYTES: usize = 4096;
/// How long the monitor waits for the application to connect before it gives up.
const CONNECT_PATIENCE: Duration = Duration::from_secs(30);
/// How long [`install`] waits for the monitor to accept a connection.
const START_PATIENCE: Duration = Duration::from_secs(10);

/// Why crash reporting could not be set up.
#[derive(Debug, thiserror::Error)]
pub enum CrashError {
    /// The folder, the socket or the monitor process could not be created.
    #[error("cannot set up crash reporting: {0}")]
    Io(#[from] io::Error),
    /// The monitor did not answer.
    #[error("the crash monitor did not start: {0}")]
    Monitor(String),
}

/// The folder crash reports go to: `crashes` inside the log folder; `VELLORA_CRASH_DIR` overrides.
#[must_use]
pub fn default_crash_dir() -> PathBuf {
    match std::env::var_os("VELLORA_CRASH_DIR").filter(|d| !d.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => crate::logging::default_log_dir().join("crashes"),
    }
}

/// `unix` seconds as `2026-10-09T08:12:00Z` (UTC).
#[must_use]
pub fn utc_timestamp(unix_seconds: u64) -> String {
    let days = i64::try_from(unix_seconds / 86_400).unwrap_or(0);
    let secs = unix_seconds % 86_400;
    // Howard Hinnant's civil-from-days.
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { year + 1 } else { year };
    format!(
        "{year:04}-{month:02}-{day:02}T{:02}:{:02}:{:02}Z",
        secs / 3600,
        secs % 3600 / 60,
        secs % 60
    )
}

fn now_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

/// Creates `<prefix>-<time>[-n].<extension>` in `dir`, failing if it exists, so that two reports in
/// one second never overwrite each other.
fn create_unique(dir: &Path, prefix: &str, extension: &str) -> io::Result<(File, PathBuf)> {
    fs::create_dir_all(dir)?;
    let now = now_seconds();
    for attempt in 0..100 {
        let name = if attempt == 0 {
            format!("{prefix}-{now}.{extension}")
        } else {
            format!("{prefix}-{now}-{attempt}.{extension}")
        };
        let path = dir.join(name);
        match OpenOptions::new().write(true).create_new(true).open(&path) {
            Ok(file) => return Ok((file, path)),
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::other("no free report name"))
}

/// Deletes the oldest reports so that at most `keep` remain. A dump and the text file beside it
/// count as one.
pub fn prune(dir: &Path, keep: usize) {
    let Ok(entries) = fs::read_dir(dir) else {
        return;
    };
    // (modified, stem) of every report file; a dump and its sidecar share a stem.
    let mut reports: Vec<(SystemTime, String)> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let extension = Path::new(&name)
            .extension()
            .and_then(std::ffi::OsStr::to_str)
            .unwrap_or_default();
        let is_report = (name.starts_with("crash-") || name.starts_with("engine-crash-"))
            && (extension.eq_ignore_ascii_case("dmp") || extension.eq_ignore_ascii_case("txt"));
        if !is_report {
            continue;
        }
        let stem = name
            .rsplit_once('.')
            .map_or(name.clone(), |(s, _)| s.to_owned());
        let modified = entry
            .metadata()
            .and_then(|m| m.modified())
            .unwrap_or(UNIX_EPOCH);
        match reports.iter_mut().find(|(_, s)| *s == stem) {
            Some(existing) => existing.0 = existing.0.max(modified),
            None => reports.push((modified, stem)),
        }
    }
    reports.sort();
    let excess = reports.len().saturating_sub(keep);
    for (_, stem) in reports.into_iter().take(excess) {
        for extension in ["dmp", "txt"] {
            let _ = fs::remove_file(dir.join(format!("{stem}.{extension}")));
        }
    }
}

/// Records that the engine ended unasked or reported an internal error: what happened, and the last
/// lines it printed (untrusted text, already sanitised by the log capture). Returns the file.
///
/// # Errors
///
/// The operating system's error if the folder or file cannot be written.
pub fn write_engine_report(
    dir: &Path,
    what: &str,
    details: &str,
    tail: &[String],
) -> io::Result<PathBuf> {
    let (mut file, path) = create_unique(dir, "engine-crash", "txt")?;
    let time = utc_timestamp(now_seconds());
    writeln!(file, "Vellora engine report")?;
    writeln!(file, "time: {time}")?;
    writeln!(file, "version: {}", env!("CARGO_PKG_VERSION"))?;
    writeln!(
        file,
        "system: {} {}",
        std::env::consts::OS,
        std::env::consts::ARCH
    )?;
    writeln!(file, "what: {what}")?;
    writeln!(file, "details: {details}")?;
    writeln!(file)?;
    writeln!(
        file,
        "Last {} lines the engine printed (text from the engine, not checked):",
        tail.len()
    )?;
    for line in tail {
        writeln!(file, "{line}")?;
    }
    file.flush()?;
    prune(dir, KEEP_REPORTS);
    Ok(path)
}

/// The socket the application and its monitor talk over: a short path (Unix socket paths are
/// limited to about 100 bytes), in the crash folder if that fits, else in the temporary folder.
fn socket_path(dir: &Path) -> PathBuf {
    let name = format!("monitor-{}.sock", std::process::id());
    let preferred = dir.join(&name);
    if preferred.as_os_str().len() <= 90 {
        preferred
    } else {
        std::env::temp_dir().join(format!("vellora-{name}"))
    }
}

/// Keeps crash reporting alive: dropping it detaches the handler and ends the monitor.
pub struct CrashGuard {
    handler: Option<CrashHandler>,
    monitor: Child,
    socket: PathBuf,
}

impl std::fmt::Debug for CrashGuard {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CrashGuard")
            .field("monitor", &self.monitor.id())
            .field("socket", &self.socket)
            .finish_non_exhaustive()
    }
}

impl Drop for CrashGuard {
    fn drop(&mut self) {
        // Detaching drops the client, which closes the socket: the monitor sees the disconnect and
        // leaves by itself.
        self.handler = None;
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while std::time::Instant::now() < deadline {
            if matches!(self.monitor.try_wait(), Ok(Some(_))) {
                break;
            }
            thread::sleep(Duration::from_millis(10));
        }
        let _ = self.monitor.kill();
        let _ = self.monitor.wait();
        let _ = fs::remove_file(&self.socket);
    }
}

/// Starts the monitor (`monitor_exe` run with [`MONITOR_FLAG`]) and attaches a crash handler to this
/// process. `info` (version, commit, system) is put beside every dump.
///
/// # Errors
///
/// [`CrashError`] if the folder or the monitor cannot be set up. The application should go on
/// without crash dumps and say so in its log.
#[allow(unsafe_code)]
pub fn install(dir: &Path, monitor_exe: &Path, info: &str) -> Result<CrashGuard, CrashError> {
    fs::create_dir_all(dir)?;
    let socket = socket_path(dir);
    let _ = fs::remove_file(&socket);
    let mut command = Command::new(monitor_exe);
    command
        .arg(MONITOR_FLAG)
        .arg(&socket)
        .arg(dir)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let mut monitor = command.spawn()?;

    let deadline = std::time::Instant::now() + START_PATIENCE;
    let client = loop {
        match Client::with_name(SocketName::Path(&socket)) {
            Ok(client) => break client,
            Err(error) => {
                if let Ok(Some(status)) = monitor.try_wait() {
                    return Err(CrashError::Monitor(format!("it exited with {status}")));
                }
                if std::time::Instant::now() >= deadline {
                    let _ = monitor.kill();
                    let _ = monitor.wait();
                    return Err(CrashError::Monitor(error.to_string()));
                }
                thread::sleep(Duration::from_millis(25));
            }
        }
    };
    let mut description = info.to_owned();
    let mut end = description.len().min(MAX_INFO_BYTES);
    while !description.is_char_boundary(end) {
        end -= 1;
    }
    description.truncate(end);
    client
        .send_message(MESSAGE_INFO, description)
        .map_err(|error| CrashError::Monitor(error.to_string()))?;

    let client = Arc::new(client);
    // SAFETY: the callback runs in the crashed process, where almost nothing is safe to do. It only
    // uses the already connected client to ask the monitor for a dump and to wait for it, which is
    // what `minidumper` is designed for; it allocates nothing of its own and takes no lock.
    let handler = CrashHandler::attach(unsafe {
        make_crash_event(move |context: &crash_handler::CrashContext| {
            // Flush earlier messages first, then ask for the dump.
            let _ = client.ping();
            CrashEventResult::Handled(client.request_dump(context).is_ok())
        })
    })
    .map_err(|error| CrashError::Monitor(error.to_string()))?;
    // Linux: only the monitor may inspect this process (the kernel's ptrace rules).
    #[cfg(any(target_os = "linux", target_os = "android"))]
    handler.set_ptracer(Some(monitor.id()));

    Ok(CrashGuard {
        handler: Some(handler),
        monitor,
        socket,
    })
}

/// Writes the dump the application asks for, a text file beside it, and ends when the application
/// goes away.
struct Writer {
    dir: PathBuf,
    info: Mutex<String>,
    connected: Arc<AtomicBool>,
}

impl ServerHandler for Writer {
    fn create_minidump_file(&self) -> Result<(File, PathBuf), io::Error> {
        create_unique(&self.dir, "crash", "dmp")
    }

    fn on_minidump_created(&self, result: Result<MinidumpBinary, minidumper::Error>) -> LoopAction {
        if let Ok(mut dump) = result {
            let _ = dump.file.flush();
            let info = self
                .info
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .clone();
            let sidecar = dump.path.with_extension("txt");
            let text = format!(
                "Vellora crash report\ntime: {}\nfile: {}\n{info}\n\nThe .dmp file holds the stacks of the \
                 threads, not the heap. Look at it before you share it.\n",
                utc_timestamp(now_seconds()),
                dump.path
                    .file_name()
                    .map_or_else(String::new, |n| n.to_string_lossy().into_owned()),
            );
            let _ = fs::write(sidecar, text);
            prune(&self.dir, KEEP_REPORTS);
        }
        // The application is gone after a crash; so is the reason to run.
        LoopAction::Exit
    }

    fn on_message(&self, kind: u32, buffer: Vec<u8>) {
        if kind == MESSAGE_INFO {
            let text =
                String::from_utf8_lossy(&buffer[..buffer.len().min(MAX_INFO_BYTES)]).into_owned();
            *self.info.lock().unwrap_or_else(PoisonError::into_inner) = text;
        }
    }

    fn on_client_connected(&self, _clients: usize) -> LoopAction {
        self.connected.store(true, Ordering::Release);
        LoopAction::Continue
    }

    fn on_client_disconnected(&self, clients: usize) -> LoopAction {
        if clients == 0 {
            LoopAction::Exit
        } else {
            LoopAction::Continue
        }
    }
}

/// The monitor: serves one application until it disconnects (a normal exit) or crashes (a dump is
/// written first). Returns the process exit code.
#[must_use]
pub fn run_monitor(socket: &Path, dir: &Path) -> i32 {
    let Ok(mut server) = Server::with_name(SocketName::Path(socket)) else {
        return 2;
    };
    let shutdown = Arc::new(AtomicBool::new(false));
    let connected = Arc::new(AtomicBool::new(false));
    // An application that never connects (it died first) must not leave a monitor behind.
    {
        let (shutdown, connected) = (Arc::clone(&shutdown), Arc::clone(&connected));
        thread::spawn(move || {
            thread::sleep(CONNECT_PATIENCE);
            if !connected.load(Ordering::Acquire) {
                shutdown.store(true, Ordering::Release);
            }
        });
    }
    let handler = Writer {
        dir: dir.to_owned(),
        info: Mutex::new(String::new()),
        connected,
    };
    let result = server.run(Box::new(handler), &shutdown, None);
    let _ = fs::remove_file(socket);
    i32::from(result.is_err())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timestamps_are_utc_iso_8601() {
        assert_eq!(utc_timestamp(0), "1970-01-01T00:00:00Z");
        assert_eq!(utc_timestamp(951_782_400), "2000-02-29T00:00:00Z");
        assert_eq!(utc_timestamp(1_760_000_000), "2025-10-09T08:53:20Z");
        assert_eq!(utc_timestamp(4_102_444_799), "2099-12-31T23:59:59Z");
    }

    #[test]
    fn an_engine_report_names_what_happened_and_keeps_the_tail() {
        let dir = tempfile::tempdir().unwrap();
        let tail = vec!["[engine 7] first".to_owned(), "[engine 7] last".to_owned()];
        let path = write_engine_report(dir.path(), "it ended", "Crash { x }", &tail).unwrap();
        assert!(
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .starts_with("engine-crash-")
        );
        let text = fs::read_to_string(path).unwrap();
        assert!(
            text.starts_with("Vellora engine report\ntime: 20"),
            "{text}"
        );
        assert!(text.contains("what: it ended"));
        assert!(text.contains("details: Crash { x }"));
        assert!(text.contains("Last 2 lines"));
        assert!(text.trim_end().ends_with("[engine 7] last"));
    }

    #[test]
    fn two_reports_in_one_second_do_not_overwrite_each_other() {
        let dir = tempfile::tempdir().unwrap();
        let a = write_engine_report(dir.path(), "one", "", &[]).unwrap();
        let b = write_engine_report(dir.path(), "two", "", &[]).unwrap();
        assert_ne!(a, b);
        assert!(fs::read_to_string(a).unwrap().contains("what: one"));
        assert!(fs::read_to_string(b).unwrap().contains("what: two"));
    }

    #[test]
    fn only_the_newest_reports_are_kept_and_a_dump_goes_with_its_text() {
        let dir = tempfile::tempdir().unwrap();
        for i in 0..5 {
            fs::write(dir.path().join(format!("crash-{i}.dmp")), b"MDMP").unwrap();
            fs::write(dir.path().join(format!("crash-{i}.txt")), b"info").unwrap();
            // Modification times a second apart, oldest first.
            let when = UNIX_EPOCH + Duration::from_secs(1_000_000 + i);
            for extension in ["dmp", "txt"] {
                let file = File::options()
                    .write(true)
                    .open(dir.path().join(format!("crash-{i}.{extension}")))
                    .unwrap();
                file.set_modified(when).unwrap();
            }
        }
        fs::write(dir.path().join("notes.txt"), b"not a report").unwrap();
        prune(dir.path(), 2);
        let mut left: Vec<String> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "crash-3.dmp",
                "crash-3.txt",
                "crash-4.dmp",
                "crash-4.txt",
                "notes.txt"
            ]
        );
    }

    #[test]
    fn the_default_folder_is_inside_the_log_folder_unless_overridden() {
        let dir = default_crash_dir();
        let text = dir.to_string_lossy().to_lowercase();
        assert!(text.contains("vellora") || std::env::var_os("VELLORA_CRASH_DIR").is_some());
    }

    #[test]
    fn the_socket_path_is_short() {
        let long = Path::new(
            "/a/very/long/folder/name/that/goes/on/and/on/and/on/and/on/and/on/and/on/and/on/and/on",
        );
        assert!(socket_path(long).as_os_str().len() <= 100);
        let short = Path::new("/tmp/c");
        assert!(socket_path(short).starts_with("/tmp/c"));
    }
}
