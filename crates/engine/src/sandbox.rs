//! The engine's own sandbox on Linux (M1 task 5, ADR-0018).
//!
//! [`apply`] is called by `main` before the PDFium thread is started (that thread loads the
//! library, so its directory stays readable) and before any document byte is read. Both Landlock
//! and seccomp filters bind the calling thread and the threads it creates afterwards, not the
//! threads that already exist. It does three things:
//!
//! 1. **Landlock**: every file-system right is handled, and only reading under the font,
//!    library and PDFium directories (and the process's own `/proc/self`) is granted back. The
//!    engine can no longer open, create, rename or delete anything else. Descriptors that are
//!    already open (the
//!    document, the tile region, the standard streams) keep working: Landlock checks opens, not
//!    reads.
//! 2. **No new privileges** (set by both mechanisms).
//! 3. **seccomp-bpf**: a deny list. Starting programs, creating processes, sockets and everything
//!    needed to reach other processes (`ptrace`, signals to other pids, `process_vm_*`) or to
//!    change the machine (`mount`, `bpf`, kernel modules, `io_uring`, ...) fail with `EPERM`.
//!    Threads are still allowed (`clone` with `CLONE_THREAD`); `clone3` answers `ENOSYS`, which
//!    makes the C library fall back to `clone`.
//!
//! A kernel without Landlock or seccomp does not stop the engine: the report says what is missing
//! and the caller logs it.

use std::collections::BTreeMap;
use std::path::Path;

use landlock::{
    ABI, Access, AccessFs, PathBeneath, PathFd, Ruleset, RulesetAttr, RulesetCreatedAttr,
    RulesetError, RulesetStatus,
};
use seccompiler::{
    BpfProgram, SeccompAction, SeccompCmpArgLen, SeccompCmpOp, SeccompCondition, SeccompFilter,
    SeccompRule, TargetArch, apply_filter,
};

/// Directories the engine may read: the fonts PDFium maps to non-embedded fonts, the libraries
/// the C library may still load lazily (locale and character-set modules), and the process's own
/// `/proc` entry (the standard library reads it for the CPU count and thread names). Not the
/// whole of `/proc`: the UI's entry there is readable by the same user and holds its environment.
const READABLE: &[&str] = &[
    "/usr/share/fonts",
    "/usr/local/share/fonts",
    "/usr/share/X11/fonts",
    "/etc/fonts",
    "/usr/lib",
    "/usr/lib64",
    "/lib",
    "/lib64",
    "/proc/self",
];

/// `CLONE_THREAD` (`linux/sched.h`): set when `clone` makes a thread rather than a process.
const CLONE_THREAD: u64 = 0x0001_0000;

/// What the file-system rules achieved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Landlock {
    /// Every right the engine asked for is enforced.
    Full,
    /// The kernel enforces a subset of them (an older Landlock ABI).
    Partial,
    /// The kernel has no Landlock (or it is disabled): the engine runs without file-system rules.
    Unavailable,
}

/// What [`apply`] did. A field is `Err` with the reason when that part could not be set up.
#[derive(Debug)]
pub struct Report {
    /// The file-system rules.
    pub landlock: Result<Landlock, String>,
    /// The system-call filter.
    pub seccomp: Result<(), String>,
}

impl Report {
    /// Whether everything is enforced in full.
    #[must_use]
    pub fn is_complete(&self) -> bool {
        matches!(self.landlock, Ok(Landlock::Full)) && self.seccomp.is_ok()
    }
}

/// Applies the file-system rules and the system-call filter to the calling thread and to every
/// thread it creates afterwards. lso_readable are directories read access is granted on in
/// addition to [READABLE] (the directory of the PDFium library, which the renderer thread loads
/// after this call). Never fails: each part reports its own outcome.
#[must_use]
pub fn apply(also_readable: &[&Path]) -> Report {
    Report {
        landlock: restrict_files(also_readable).map_err(|error| error.to_string()),
        seccomp: filter_syscalls(),
    }
}

fn restrict_files(also_readable: &[&Path]) -> Result<Landlock, RulesetError> {
    // ABI 3 adds truncation; older kernels get the subset they know (best effort).
    let abi = ABI::V3;
    let read = AccessFs::from_read(abi);
    // A directory that does not exist on this system (no X11 fonts) is simply left out.
    let rules = READABLE
        .iter()
        .map(Path::new)
        .chain(also_readable.iter().copied())
        .filter_map(|path| PathFd::new(path).ok())
        .map(|fd| Ok::<_, RulesetError>(PathBeneath::new(fd, read)));
    let status = Ruleset::default()
        .handle_access(AccessFs::from_all(abi))?
        .create()?
        .add_rules(rules)?
        .restrict_self()?;
    Ok(match status.ruleset {
        RulesetStatus::FullyEnforced => Landlock::Full,
        RulesetStatus::PartiallyEnforced => Landlock::Partial,
        RulesetStatus::NotEnforced => Landlock::Unavailable,
    })
}

/// System calls that always fail with `EPERM`.
fn denied() -> BTreeMap<i64, Vec<SeccompRule>> {
    let always = [
        // Starting programs.
        libc::SYS_execve,
        libc::SYS_execveat,
        // The network and other sockets.
        libc::SYS_socket,
        libc::SYS_socketpair,
        libc::SYS_connect,
        libc::SYS_bind,
        libc::SYS_listen,
        libc::SYS_accept,
        libc::SYS_accept4,
        // Reaching other processes.
        libc::SYS_ptrace,
        libc::SYS_process_vm_readv,
        libc::SYS_process_vm_writev,
        libc::SYS_pidfd_open,
        libc::SYS_pidfd_getfd,
        libc::SYS_pidfd_send_signal,
        libc::SYS_tkill,
        libc::SYS_rt_sigqueueinfo,
        libc::SYS_rt_tgsigqueueinfo,
        // Changing the machine, namespaces, and ways around this filter.
        libc::SYS_mount,
        libc::SYS_umount2,
        libc::SYS_pivot_root,
        libc::SYS_chroot,
        libc::SYS_setns,
        libc::SYS_unshare,
        libc::SYS_open_by_handle_at,
        libc::SYS_bpf,
        libc::SYS_perf_event_open,
        libc::SYS_keyctl,
        libc::SYS_add_key,
        libc::SYS_request_key,
        libc::SYS_userfaultfd,
        libc::SYS_kexec_load,
        libc::SYS_init_module,
        libc::SYS_finit_module,
        libc::SYS_delete_module,
        libc::SYS_reboot,
        libc::SYS_io_uring_setup,
        libc::SYS_io_uring_enter,
        libc::SYS_io_uring_register,
    ];
    let mut rules: BTreeMap<i64, Vec<SeccompRule>> =
        always.into_iter().map(|nr| (nr, Vec::new())).collect();
    // Only x86-64 has these; elsewhere the C library implements them with `clone`.
    #[cfg(target_arch = "x86_64")]
    {
        rules.insert(libc::SYS_fork, Vec::new());
        rules.insert(libc::SYS_vfork, Vec::new());
    }
    // A new process, not a new thread: `clone` without `CLONE_THREAD`.
    if let Ok(condition) = SeccompCondition::new(
        0,
        SeccompCmpArgLen::Dword,
        SeccompCmpOp::MaskedEq(CLONE_THREAD),
        0,
    ) && let Ok(rule) = SeccompRule::new(vec![condition])
    {
        rules.insert(libc::SYS_clone, vec![rule]);
    }
    // Signals only to this process (`abort` raises one): `kill` and `tgkill` with any other pid.
    let own = u64::from(std::process::id());
    for nr in [libc::SYS_kill, libc::SYS_tgkill] {
        if let Ok(condition) =
            SeccompCondition::new(0, SeccompCmpArgLen::Dword, SeccompCmpOp::Ne, own)
            && let Ok(rule) = SeccompRule::new(vec![condition])
        {
            rules.insert(nr, vec![rule]);
        }
    }
    rules
}

fn filter_syscalls() -> Result<(), String> {
    let arch = TargetArch::try_from(std::env::consts::ARCH)
        .map_err(|error| format!("no seccomp support for this architecture: {error}"))?;
    install(
        denied(),
        SeccompAction::Errno(libc::EPERM.cast_unsigned()),
        arch,
    )?;
    // `clone3` takes a structure the filter cannot look into. The C library falls back to `clone`
    // (inspected above) when it gets `ENOSYS`.
    let mut clone3 = BTreeMap::new();
    clone3.insert(libc::SYS_clone3, Vec::new());
    install(
        clone3,
        SeccompAction::Errno(libc::ENOSYS.cast_unsigned()),
        arch,
    )
}

fn install(
    rules: BTreeMap<i64, Vec<SeccompRule>>,
    on_match: SeccompAction,
    arch: TargetArch,
) -> Result<(), String> {
    let filter = SeccompFilter::new(rules, SeccompAction::Allow, on_match, arch)
        .map_err(|error| error.to_string())?;
    let program: BpfProgram = filter
        .try_into()
        .map_err(|error: seccompiler::BackendError| error.to_string())?;
    apply_filter(&program).map_err(|error| error.to_string())
}
