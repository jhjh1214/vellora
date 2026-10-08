//! Starting an engine process under resource limits, and making sense of how it ended.
//!
//! [`EngineProcess::spawn`] is the one place an engine is launched from. It marks the document and
//! the tile region inheritable, starts the executable with the [`LaunchArgs`] command line and its
//! standard streams piped, stops sharing, and puts the child under the [`ResourceLimits`]. Spawns
//! are serialised by a lock, so no other child of this process inherits the two files in the
//! window between sharing and un-sharing (ADR-0015).
//!
//! The engine has no way to say "I crashed": a crash is its pipe closing without a `Close`.
//! [`EngineProcess::crash`] turns that into a typed [`Crash`] with how the process ended.

use std::ffi::OsString;
use std::fs::File;
use std::io;
use std::path::Path;
use std::process::ExitStatus;
use std::sync::Mutex;
use std::thread;
use std::time::{Duration, Instant};

use vellora_engine::{DEFAULT_MAX_DOCUMENT_BYTES, Deadlines, LaunchArgs};
use vellora_shm::{HandleToken, SlotGeometry, share_with_child, stop_sharing};

use crate::limits::{Guard, ResourceLimits};

/// Where the engine looks for PDFium before it looks next to itself: `vellora_render::LIBRARY_ENV`,
/// which this crate does not link (`crates/engine/tests/sandbox.rs` keeps the two equal). The
/// Windows sandbox grants the container read access to the file it names (ADR-0017).
pub const PDFIUM_ENV: &str = "VELLORA_PDFIUM_LIB";

/// The PDFium library next to the engine on Windows (`vellora_render::library_file_name`).
pub const PDFIUM_LIBRARY_WINDOWS: &str = "pdfium.dll";

/// The operating system's handle on the engine process.
#[cfg(unix)]
type Process = std::process::Child;
#[cfg(windows)]
type Process = crate::sandbox::Child;

/// Serialises the share, spawn, un-share sequence.
static SPAWN: Mutex<()> = Mutex::new(());

/// How often [`EngineProcess::wait_timeout`] checks whether the child has exited.
const POLL: Duration = Duration::from_millis(10);

/// Why an engine could not be started.
#[derive(Debug, thiserror::Error)]
pub enum SpawnError {
    /// The document or the region could not be made inheritable (or un-shared).
    #[error("cannot share a file with the engine: {0}")]
    Share(#[from] vellora_shm::Error),
    /// The operating system refused to start the executable.
    #[error("cannot start the engine: {0}")]
    Start(#[source] io::Error),
    /// The sandbox could not be built or the process could not be started inside it (Windows).
    /// There is no fallback to running the engine unsandboxed.
    #[error("cannot start the engine in its sandbox: {0}")]
    Sandbox(#[source] io::Error),
    /// The process started but could not be put under its limits; it was killed.
    #[error("cannot apply the resource limits to the engine: {0}")]
    Limits(#[source] io::Error),
}

/// What to start and under which limits.
#[derive(Debug)]
pub struct SpawnConfig<'a> {
    /// The `vellora-engine` executable.
    pub engine: &'a Path,
    /// The open document, which the engine adopts by handle (it never receives a path).
    pub document: &'a File,
    /// The open tile region file (the other half of `vellora_shm::TileRegion::create`).
    pub region: &'a File,
    /// How the region is cut into slots.
    pub geometry: SlotGeometry,
    /// Documents larger than this are refused by the engine before it maps them.
    pub max_document_bytes: u64,
    /// Soft and hard time per tile; the engine aborts itself after the hard one.
    pub deadlines: Deadlines,
    /// What the operating system lets the process use.
    pub limits: ResourceLimits,
    /// Extra environment variables for the engine (for example where to find PDFium).
    pub env: Vec<(OsString, OsString)>,
}

impl<'a> SpawnConfig<'a> {
    /// The usual configuration: the engine's own defaults for the document size and deadlines,
    /// and a memory cap of [`ResourceLimits::for_mapped`] over the document and the region.
    #[must_use]
    pub fn new(
        engine: &'a Path,
        document: &'a File,
        region: &'a File,
        geometry: SlotGeometry,
    ) -> Self {
        let document_bytes = document.metadata().map_or(0, |meta| meta.len());
        let region_bytes = u64::from(geometry.slot_count()) * u64::from(geometry.slot_bytes());
        Self {
            engine,
            document,
            region,
            geometry,
            max_document_bytes: DEFAULT_MAX_DOCUMENT_BYTES,
            deadlines: Deadlines::default(),
            limits: ResourceLimits::for_mapped(document_bytes.saturating_add(region_bytes)),
            env: Vec::new(),
        }
    }
}

/// How an engine process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Termination {
    /// Exit code 0: the engine ended its session in order.
    Success,
    /// Exit with a non-zero code: a refusal or a startup failure it reported itself.
    Failure(i32),
    /// Killed by a signal (Unix): `SIGABRT` after a hard deadline or a failed allocation,
    /// `SIGSEGV` or `SIGBUS` for a fault, `SIGKILL` from the kernel or from us.
    Signal(i32),
    /// Ended by an unhandled exception or a fast fail (Windows): the exit status is an `NTSTATUS`
    /// error such as `0xC0000005` (access violation), `0xC0000409` (what `abort` raises) or
    /// `0xE0000008` (what PDFium's allocator raises when memory runs out).
    Exception(u32),
}

impl Termination {
    fn of(status: ExitStatus) -> Self {
        if status.success() {
            return Self::Success;
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::ExitStatusExt;
            if let Some(signal) = status.signal() {
                return Self::Signal(signal);
            }
        }
        match status.code() {
            // An `NTSTATUS` with error severity has its top two bits set (`0xC...` system errors,
            // `0xE...` application-defined ones). Anything else is a plain exit code.
            #[allow(clippy::cast_sign_loss)]
            Some(code) if cfg!(windows) && (code as u32) >> 30 == 0b11 => {
                Self::Exception(code as u32)
            }
            Some(code) => Self::Failure(code),
            // Unreachable on the platforms we support: no code means a signal, handled above.
            None => Self::Failure(-1),
        }
    }
}

/// The engine ended without being asked to (its pipe closed before a `Close` was sent).
#[derive(Clone, Copy, Debug, PartialEq, Eq, thiserror::Error)]
#[error("the engine process ended unexpectedly ({termination:?})")]
pub struct Crash {
    /// How it ended.
    pub termination: Termination,
    /// The process had not exited when the pipe closed and the client had to kill it.
    pub killed_by_client: bool,
}

/// A running engine: the child process, its limits and its two pipes.
#[derive(Debug)]
pub struct EngineProcess {
    child: Process,
    document: HandleToken,
    stdin: Option<File>,
    stdout: Option<File>,
    // Dropped after the child is reaped; on Windows, closing the job also kills the engine.
    _guard: Guard,
}

impl EngineProcess {
    /// Starts the engine described by `config`.
    ///
    /// # Errors
    ///
    /// [`SpawnError`] if a file cannot be shared, the executable cannot start, or the limits
    /// cannot be applied (the child is killed in that case).
    pub fn spawn(config: &SpawnConfig<'_>) -> Result<Self, SpawnError> {
        // A poisoned lock only means another spawn panicked; the lock protects no data.
        let _serial = SPAWN
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);

        let document = share_with_child(config.document)?;
        let region = match share_with_child(config.region) {
            Ok(token) => token,
            Err(error) => {
                let _ = stop_sharing(config.document);
                return Err(error.into());
            }
        };
        let launch = LaunchArgs {
            file: document,
            region,
            geometry: config.geometry,
            max_document_bytes: config.max_document_bytes,
            deadlines: config.deadlines,
        };

        let started = start(config, &launch);

        // Whatever happened, later children must not inherit the files.
        let unshared_document = stop_sharing(config.document);
        let unshared_region = stop_sharing(config.region);
        let mut started = started?;
        if let Err(error) = unshared_document.and(unshared_region) {
            kill_and_reap(&mut started.child);
            return Err(error.into());
        }
        Ok(Self {
            child: started.child,
            document,
            stdin: Some(started.stdin),
            stdout: Some(started.stdout),
            _guard: started.guard,
        })
    }

    /// The operating-system process id.
    #[must_use]
    pub fn id(&self) -> u32 {
        self.child.id()
    }

    /// The number the engine knows the document by: the `handle_token` its `Open` request must
    /// carry.
    #[must_use]
    pub fn document_token(&self) -> HandleToken {
        self.document
    }

    /// The pipe to the engine's standard input (requests). `None` after the first call.
    pub fn take_stdin(&mut self) -> Option<File> {
        self.stdin.take()
    }

    /// The pipe from the engine's standard output (responses). `None` after the first call.
    pub fn take_stdout(&mut self) -> Option<File> {
        self.stdout.take()
    }

    /// Waits up to `timeout` for the process to end.
    ///
    /// # Errors
    ///
    /// The operating system's error if the process cannot be queried.
    pub fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<Termination>> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(status) = self.child.try_wait()? {
                return Ok(Some(Termination::of(status)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(POLL);
        }
    }

    /// Reports the end of a session whose pipe closed without a `Close`: how the process ended,
    /// given `grace` to finish exiting, after which it is killed. A client calls this when it
    /// reads end-of-file from the engine's output; treat the result as a crash even for
    /// [`Termination::Success`] (an engine does not leave a session on its own).
    pub fn crash(&mut self, grace: Duration) -> Crash {
        if let Ok(Some(termination)) = self.wait_timeout(grace) {
            return Crash {
                termination,
                killed_by_client: false,
            };
        }
        self.child.kill().ok();
        let termination = self
            .child
            .wait()
            .map_or(Termination::Failure(-1), Termination::of);
        Crash {
            termination,
            killed_by_client: true,
        }
    }

    /// Kills the process and waits for it. Does nothing if it already ended.
    pub fn kill(&mut self) {
        kill_and_reap(&mut self.child);
    }
}

impl Drop for EngineProcess {
    fn drop(&mut self) {
        // Closing our end of the pipe asks a healthy engine to leave; one that does not is killed.
        self.stdin = None;
        if !matches!(self.wait_timeout(Duration::from_millis(500)), Ok(Some(_))) {
            kill_and_reap(&mut self.child);
        }
    }
}

fn kill_and_reap(child: &mut Process) {
    // The process may already be gone, which is the outcome wanted.
    let _ = child.kill();
    let _ = child.wait();
}

/// A launched engine, before it is wrapped in an [`EngineProcess`].
struct Started {
    child: Process,
    stdin: File,
    stdout: File,
    guard: Guard,
}

/// Starts the engine under its resource limits (Unix: `setrlimit` between fork and exec).
#[cfg(unix)]
fn start(config: &SpawnConfig<'_>, launch: &LaunchArgs) -> Result<Started, SpawnError> {
    use std::os::fd::OwnedFd;
    use std::process::{Command, Stdio};

    let mut command = Command::new(config.engine);
    command
        .args(launch.to_args())
        .envs(config.env.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit());
    crate::limits::prepare(&mut command, config.limits);
    let mut child = command.spawn().map_err(SpawnError::Start)?;
    let pipes = child
        .stdin
        .take()
        .zip(child.stdout.take())
        .map(|(stdin, stdout)| {
            (
                File::from(OwnedFd::from(stdin)),
                File::from(OwnedFd::from(stdout)),
            )
        });
    let guard = match crate::limits::confine(&child, config.limits) {
        Ok(guard) => guard,
        Err(error) => {
            kill_and_reap(&mut child);
            return Err(SpawnError::Limits(error));
        }
    };
    let Some((stdin, stdout)) = pipes else {
        kill_and_reap(&mut child);
        return Err(SpawnError::Start(io::Error::other(
            "no pipes to the engine",
        )));
    };
    Ok(Started {
        child,
        stdin,
        stdout,
        guard,
    })
}

/// Starts the engine in an `AppContainer`, created suspended and put under its job before it runs
/// (ADR-0017).
#[cfg(windows)]
fn start(config: &SpawnConfig<'_>, launch: &LaunchArgs) -> Result<Started, SpawnError> {
    use std::os::windows::io::AsRawHandle;

    let args = launch.to_args();
    let inherit = [
        config.document.as_raw_handle(),
        config.region.as_raw_handle(),
    ];
    let spec = crate::sandbox::Spec {
        engine: config.engine,
        args: &args,
        env: &config.env,
        inherit: &inherit,
        limits: config.limits,
    };
    let started = crate::sandbox::spawn(&spec).map_err(SpawnError::Sandbox)?;
    Ok(Started {
        child: started.child,
        stdin: started.stdin,
        stdout: started.stdout,
        guard: started.guard,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[cfg(unix)]
    #[test]
    fn a_signal_is_named() {
        use std::os::unix::process::ExitStatusExt;
        // The raw wait status of a process killed by SIGABRT is the signal number.
        assert_eq!(
            Termination::of(ExitStatus::from_raw(6)),
            Termination::Signal(6)
        );
        // An exit code lives in the second byte of the raw status.
        assert_eq!(
            Termination::of(ExitStatus::from_raw(3 << 8)),
            Termination::Failure(3)
        );
        assert_eq!(
            Termination::of(ExitStatus::from_raw(0)),
            Termination::Success
        );
    }

    #[cfg(windows)]
    #[test]
    fn an_ntstatus_is_an_exception_and_a_small_code_a_failure() {
        use std::os::windows::process::ExitStatusExt;
        assert_eq!(
            Termination::of(ExitStatus::from_raw(0xC000_0409)),
            Termination::Exception(0xC000_0409)
        );
        assert_eq!(
            Termination::of(ExitStatus::from_raw(0xE000_0008)),
            Termination::Exception(0xE000_0008)
        );
        assert_eq!(
            Termination::of(ExitStatus::from_raw(2)),
            Termination::Failure(2)
        );
        assert_eq!(
            Termination::of(ExitStatus::from_raw(0)),
            Termination::Success
        );
    }
}
