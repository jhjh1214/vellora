//! Operating-system limits for the engine process (ADR-0004), applied by the parent at spawn.
//!
//! The in-code limits of `vellora-cos` stop a hostile document from asking for too much; these
//! stop PDFium and the codecs (C and C++, outside our control) from taking the machine with them.
//!
//! | Platform | Memory | CPU time | No child processes | Dies with the UI |
//! |---|---|---|---|---|
//! | Windows | job `ProcessMemoryLimit` (committed bytes) | job `PerProcessUserTimeLimit` | job `ActiveProcessLimit = 1` | job kill-on-close |
//! | Linux | `RLIMIT_AS` (address space) | `RLIMIT_CPU` | not enforced yet | engine exits at end of input |
//! | macOS | **not enforced** (the kernel ignores `RLIMIT_AS` and `RLIMIT_DATA`) | `RLIMIT_CPU` | not enforced yet | engine exits at end of input |
//!
//! On Linux and macOS the engine also starts with `RLIMIT_CORE = 0`: a core file would hold the
//! document's content, and writing one delays the exit of a process that aborts on a deadline.
//!
//! The unenforced cells are listed in the security model's tracker; the per-OS sandbox of the
//! later phases is what closes them. On Windows the job is assigned to the engine while it is still
//! suspended (`sandbox.rs`, ADR-0017), so no instruction of it runs outside the job.

use std::io;
#[cfg(unix)]
use std::process::{Child, Command};

/// Address space or committed memory the engine gets on top of the document and the tile region.
/// PDFium, its codecs, thread stacks and the allocator's arenas all live inside it; a 10,000-page
/// document needs a few hundred MiB, so this is generous on purpose: the cap is a backstop
/// against a runaway, not a budget.
pub const MEMORY_HEADROOM_BYTES: u64 = 4 << 30;

/// What the engine process may use.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ResourceLimits {
    /// Most memory the engine may hold. On Windows this is committed memory; on Linux it is the
    /// whole address space, which includes the mapped document and tile region (use
    /// [`ResourceLimits::for_mapped`]). `None` leaves memory unlimited. Not enforced on macOS.
    pub memory_bytes: Option<u64>,
    /// Most CPU time the engine may use in its lifetime. A session renders for hours, so there is
    /// no default: set it when a bounded, short-lived engine is wanted (the CLI's one-shot
    /// commands). `None` leaves it unlimited.
    pub cpu_seconds: Option<u64>,
}

impl ResourceLimits {
    /// Whether this platform enforces [`memory_bytes`](Self::memory_bytes).
    pub const ENFORCES_MEMORY: bool = cfg!(not(target_os = "macos"));

    /// The default memory cap for an engine that maps `mapped_bytes` (the document plus the tile
    /// region): [`MEMORY_HEADROOM_BYTES`] on top of them.
    #[must_use]
    pub fn for_mapped(mapped_bytes: u64) -> Self {
        Self {
            memory_bytes: Some(MEMORY_HEADROOM_BYTES.saturating_add(mapped_bytes)),
            cpu_seconds: None,
        }
    }
}

/// Holds what must live as long as the child (the Windows job; nothing elsewhere).
#[derive(Debug)]
pub(crate) struct Guard {
    #[cfg(windows)]
    _job: os::Job,
}

#[cfg(unix)]
/// Prepares `command` so that the child starts under `limits` where they can be set before it
/// runs (Unix). Call [`confine`] with the spawned child afterwards for what needs its handle.
pub(crate) fn prepare(command: &mut Command, limits: ResourceLimits) {
    os::prepare(command, limits);
}

/// Applies the limits that need the running child (the Windows job) and returns what keeps them
/// alive.
#[cfg(unix)]
pub(crate) fn confine(child: &Child, limits: ResourceLimits) -> io::Result<Guard> {
    os::confine(child, limits)
}

/// Puts the process under the job. The sandbox calls it on a process that is still suspended.
#[cfg(windows)]
pub(crate) fn confine(
    process: &impl std::os::windows::io::AsRawHandle,
    limits: ResourceLimits,
) -> io::Result<Guard> {
    os::confine(process.as_raw_handle(), limits)
}

#[cfg(unix)]
#[allow(unsafe_code)]
mod os {
    use std::io;
    use std::os::unix::process::CommandExt;
    use std::process::{Child, Command};

    use super::{Guard, ResourceLimits};

    /// Sets the soft and hard limit of `resource` to `value`.
    macro_rules! set_limit {
        ($resource:expr, $value:expr) => {{
            let value = libc::rlim_t::try_from($value).unwrap_or(libc::RLIM_INFINITY);
            let limit = libc::rlimit {
                rlim_cur: value,
                rlim_max: value,
            };
            // SAFETY: `setrlimit` reads the struct it is given and changes only this process's
            // limits. It is async-signal-safe, so it may run between `fork` and `exec`.
            if unsafe { libc::setrlimit($resource, &raw const limit) } == 0 {
                Ok(())
            } else {
                Err(io::Error::last_os_error())
            }
        }};
    }

    pub(super) fn prepare(command: &mut Command, limits: ResourceLimits) {
        // Built outside the `unsafe` block below: its own `unsafe` is the `setrlimit` call.
        let apply = move || -> io::Result<()> {
            // No core dump, ever: it would write what the engine holds (the document's pixels and
            // text) to disk, and on a host with a crash-collecting `core_pattern` the process
            // lingers for seconds before it is gone, which is not what a hard deadline means.
            set_limit!(libc::RLIMIT_CORE, 0_u64)?;
            #[cfg(not(target_os = "macos"))]
            if let Some(bytes) = limits.memory_bytes {
                set_limit!(libc::RLIMIT_AS, bytes)?;
            }
            if let Some(seconds) = limits.cpu_seconds {
                set_limit!(libc::RLIMIT_CPU, seconds)?;
            }
            Ok(())
        };
        // SAFETY: `apply` runs in the forked child before `exec` and only calls `setrlimit`
        // (async-signal-safe): no allocation, no locks, none of the parent's other threads' state.
        unsafe { command.pre_exec(apply) };
    }

    // Same signature as on Windows, where the job object can fail.
    #[allow(clippy::unnecessary_wraps)]
    pub(super) fn confine(_child: &Child, _limits: ResourceLimits) -> io::Result<Guard> {
        Ok(Guard {})
    }
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod os {
    use std::io;
    use std::os::windows::io::RawHandle;

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{
        AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_ACTIVE_PROCESS,
        JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
        JOB_OBJECT_LIMIT_PROCESS_MEMORY, JOB_OBJECT_LIMIT_PROCESS_TIME, JOB_OBJECT_UILIMIT_DESKTOP,
        JOB_OBJECT_UILIMIT_DISPLAYSETTINGS, JOB_OBJECT_UILIMIT_EXITWINDOWS,
        JOB_OBJECT_UILIMIT_GLOBALATOMS, JOB_OBJECT_UILIMIT_HANDLES,
        JOB_OBJECT_UILIMIT_READCLIPBOARD, JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS,
        JOB_OBJECT_UILIMIT_WRITECLIPBOARD, JOBOBJECT_BASIC_UI_RESTRICTIONS,
        JOBOBJECT_EXTENDED_LIMIT_INFORMATION, JobObjectBasicUIRestrictions,
        JobObjectExtendedLimitInformation, SetInformationJobObject,
    };

    use super::{Guard, ResourceLimits};

    /// A job object; closing the last handle kills every process in it.
    #[derive(Debug)]
    pub(super) struct Job(HANDLE);

    // SAFETY: a job handle is a kernel object reference that any thread may use or close.
    unsafe impl Send for Job {}
    // SAFETY: as above; the handle is only closed in `drop`.
    unsafe impl Sync for Job {}

    impl Drop for Job {
        fn drop(&mut self) {
            // SAFETY: the handle came from `CreateJobObjectW`, is owned by this value and is
            // closed exactly once, here.
            unsafe { CloseHandle(self.0) };
        }
    }

    pub(super) fn confine(process: RawHandle, limits: ResourceLimits) -> io::Result<Guard> {
        // SAFETY: both arguments are optional and null selects the defaults.
        let handle = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
        if handle.is_null() {
            return Err(io::Error::last_os_error());
        }
        let job = Job(handle);

        // SAFETY: the structure is plain data for which all-zero means "no limit set".
        let mut info: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
        // Kill the engine when the UI goes away, show no crash dialog for it, and let it have
        // no child processes of its own (it never needs one).
        let mut flags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
            | JOB_OBJECT_LIMIT_DIE_ON_UNHANDLED_EXCEPTION
            | JOB_OBJECT_LIMIT_ACTIVE_PROCESS;
        info.BasicLimitInformation.ActiveProcessLimit = 1;
        if let Some(bytes) = limits.memory_bytes {
            flags |= JOB_OBJECT_LIMIT_PROCESS_MEMORY;
            info.ProcessMemoryLimit = usize::try_from(bytes).unwrap_or(usize::MAX);
        }
        if let Some(seconds) = limits.cpu_seconds {
            flags |= JOB_OBJECT_LIMIT_PROCESS_TIME;
            // 100 ns units.
            info.BasicLimitInformation.PerProcessUserTimeLimit =
                i64::try_from(seconds.saturating_mul(10_000_000)).unwrap_or(i64::MAX);
        }
        info.BasicLimitInformation.LimitFlags = flags;

        let size = u32::try_from(size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>())
            .map_err(|_| io::Error::other("job limit structure too large"))?;
        // SAFETY: `info` is a valid structure of the class and size given, and outlives the call.
        let set = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectExtendedLimitInformation,
                (&raw const info).cast(),
                size,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }

        // The engine draws into memory and has no window: it may not touch the clipboard, the
        // display settings, global atoms, other processes' windows or the session (logoff).
        let ui = JOBOBJECT_BASIC_UI_RESTRICTIONS {
            UIRestrictionsClass: JOB_OBJECT_UILIMIT_DESKTOP
                | JOB_OBJECT_UILIMIT_DISPLAYSETTINGS
                | JOB_OBJECT_UILIMIT_EXITWINDOWS
                | JOB_OBJECT_UILIMIT_GLOBALATOMS
                | JOB_OBJECT_UILIMIT_HANDLES
                | JOB_OBJECT_UILIMIT_READCLIPBOARD
                | JOB_OBJECT_UILIMIT_SYSTEMPARAMETERS
                | JOB_OBJECT_UILIMIT_WRITECLIPBOARD,
        };
        let ui_size = u32::try_from(size_of::<JOBOBJECT_BASIC_UI_RESTRICTIONS>())
            .map_err(|_| io::Error::other("job UI structure too large"))?;
        // SAFETY: `ui` is a valid structure of the class and size given, and outlives the call.
        let set = unsafe {
            SetInformationJobObject(
                job.0,
                JobObjectBasicUIRestrictions,
                (&raw const ui).cast(),
                ui_size,
            )
        };
        if set == 0 {
            return Err(io::Error::last_os_error());
        }
        // The sandbox calls this on a suspended process, so no instruction of the engine has run
        // outside the job.
        // SAFETY: both handles are valid: the job is owned above and the process handle is kept
        // alive by the caller.
        let assigned = unsafe { AssignProcessToJobObject(job.0, process) };
        if assigned == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Guard { _job: job })
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn the_default_memory_cap_adds_headroom_to_what_is_mapped() {
        let limits = ResourceLimits::for_mapped(1 << 20);
        assert_eq!(limits.memory_bytes, Some(MEMORY_HEADROOM_BYTES + (1 << 20)));
        assert_eq!(limits.cpu_seconds, None);
        // A document near `u64::MAX` must not wrap around to a tiny cap.
        assert_eq!(
            ResourceLimits::for_mapped(u64::MAX).memory_bytes,
            Some(u64::MAX)
        );
    }

    /// Runs a shell that prints one `ulimit` value, started the way the engine is.
    #[cfg(unix)]
    fn ulimit(flag: &str, limits: ResourceLimits) -> String {
        let mut command = Command::new("sh");
        command.args(["-c", &format!("ulimit {flag}")]);
        prepare(&mut command, limits);
        let output = command.output().unwrap();
        assert!(output.status.success(), "{output:?}");
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    #[cfg(unix)]
    #[test]
    fn the_engine_never_dumps_core() {
        // Whatever limits are asked for, including none: a core file would hold the document.
        assert_eq!(ulimit("-c", ResourceLimits::default()), "0");
        assert_eq!(ulimit("-c", ResourceLimits::for_mapped(1 << 20)), "0");
    }

    #[cfg(unix)]
    #[test]
    fn cpu_time_is_limited_with_rlimit() {
        let limits = ResourceLimits {
            memory_bytes: None,
            cpu_seconds: Some(123),
        };
        assert_eq!(ulimit("-t", limits), "123");
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    #[test]
    fn memory_is_limited_with_the_address_space_rlimit() {
        let limits = ResourceLimits {
            memory_bytes: Some(1 << 30),
            cpu_seconds: None,
        };
        // `ulimit -v` reports KiB.
        assert_eq!(ulimit("-v", limits), (1u64 << 20).to_string());
    }

    #[cfg(unix)]
    #[test]
    fn no_limits_leave_the_inherited_ones_alone() {
        let unlimited = ulimit("-t", ResourceLimits::default());
        assert_eq!(
            unlimited,
            ulimit("-t", ResourceLimits::default()),
            "no limit must change nothing"
        );
        assert_ne!(unlimited, "123");
    }

    /// A process that lives long enough for the test to act on it.
    #[cfg(windows)]
    fn sleeper() -> std::process::Child {
        std::process::Command::new("ping")
            .args(["-n", "60", "127.0.0.1"])
            .stdout(std::process::Stdio::null())
            .spawn()
            .unwrap()
    }

    #[cfg(windows)]
    #[test]
    fn closing_the_job_kills_the_process() {
        let mut child = sleeper();
        let guard = confine(&child, ResourceLimits::default()).unwrap();
        assert!(
            child.try_wait().unwrap().is_none(),
            "still running in its job"
        );
        let started = std::time::Instant::now();
        drop(guard);
        // The job ends the process with exit code 0, so only the time shows that it was killed:
        // left alone it would run for a minute.
        child.wait().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(10));
    }

    #[cfg(windows)]
    #[test]
    fn a_process_in_the_job_cannot_start_another() {
        // Without the job this line takes two seconds and exits 0 (checked by hand). With it the
        // shell cannot start `ping` or the second `cmd`, and the exit status says so. The shell
        // starts several milliseconds after `spawn` returns, long after the job is in place.
        let mut child = std::process::Command::new("cmd")
            .args([
                "/V:ON",
                "/C",
                "ping -n 3 127.0.0.1 >nul & cmd /C exit 0 & exit /B !ERRORLEVEL!",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap();
        let _guard = confine(&child, ResourceLimits::default()).unwrap();
        let status = child.wait().unwrap();
        assert_ne!(
            status.code(),
            Some(0),
            "the second process must have been refused: {status:?}"
        );
    }
}
