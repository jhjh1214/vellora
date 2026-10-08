//! Starting the engine inside a Windows `AppContainer` (M1 task 4, ADR-0017).
//!
//! The engine runs PDFium and the image codecs on hostile input, so it gets the least privilege
//! Windows offers a process that must still read one file and write to one pipe:
//!
//! - **`AppContainer`, no capabilities.** The token carries an `AppContainer` SID and no capability
//!   SIDs: no network (not even loopback), no user files, no registry outside its own, no access to
//!   other processes' objects. It can read what is readable by `ALL APPLICATION PACKAGES` (the
//!   system directories, the fonts) and what we grant its SID: the engine executable and the PDFium
//!   library, one file at a time.
//! - **An explicit handle list.** Only the document, the tile region and the three standard
//!   streams are inherited (`PROC_THREAD_ATTRIBUTE_HANDLE_LIST`), whatever else the UI process
//!   holds that happens to be inheritable.
//! - **Created suspended, jailed, then resumed.** The job object (memory cap, no child processes,
//!   kill on close, UI restrictions) is assigned before the first instruction runs.
//! - **A minimal environment** and a neutral working directory, so nothing about the user's shell
//!   or profile reaches the engine.
//!
//! The `AppContainer` SID is derived from a fixed name; no profile is registered, so nothing is
//! written to the registry or the user's profile except the two file-level ACL entries above.

use std::collections::HashSet;
use std::ffi::{OsStr, OsString};
use std::fs::File;
use std::io;
use std::iter::repeat_n;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::ExitStatusExt;
use std::path::{Path, PathBuf};
use std::process::ExitStatus;
use std::ptr;
use std::sync::Mutex;

use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, ERROR_INVALID_PARAMETER, HANDLE, HANDLE_FLAG_INHERIT,
    INVALID_HANDLE_VALUE, LocalFree, SetHandleInformation, TRUE, WAIT_FAILED, WAIT_OBJECT_0,
    WAIT_TIMEOUT,
};
use windows_sys::Win32::Security::Authorization::{
    EXPLICIT_ACCESS_W, GRANT_ACCESS, GetNamedSecurityInfoW, SE_FILE_OBJECT, SetEntriesInAclW,
    SetNamedSecurityInfoW, TRUSTEE_IS_SID, TRUSTEE_IS_UNKNOWN, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
use windows_sys::Win32::Security::{
    ACL, DACL_SECURITY_INFORMATION, FreeSid, PSECURITY_DESCRIPTOR, PSID, SECURITY_ATTRIBUTES,
    SECURITY_CAPABILITIES,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_SHARE_READ,
    FILE_SHARE_WRITE, GetFileType, OPEN_EXISTING,
};
use windows_sys::Win32::System::Console::{GetStdHandle, STD_ERROR_HANDLE};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CREATE_NO_WINDOW, CREATE_SUSPENDED, CREATE_UNICODE_ENVIRONMENT, CreateProcessW,
    DeleteProcThreadAttributeList, EXTENDED_STARTUPINFO_PRESENT, GetCurrentProcess,
    GetExitCodeProcess, INFINITE, InitializeProcThreadAttributeList, LPPROC_THREAD_ATTRIBUTE_LIST,
    PROC_THREAD_ATTRIBUTE_HANDLE_LIST, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    PROCESS_INFORMATION, ResumeThread, STARTF_USESTDHANDLES, STARTUPINFOEXW, TerminateProcess,
    UpdateProcThreadAttribute, WaitForSingleObject,
};

use crate::limits::{self, Guard, ResourceLimits};
use crate::process::{PDFIUM_ENV, PDFIUM_LIBRARY_WINDOWS};

/// The name the `AppContainer` SID is derived from. Fixed, so the ACL entries we add for it stay
/// valid across runs and versions.
const CONTAINER_NAME: &str = "Vellora.Engine";

/// Environment variables the engine may inherit. Everything else is dropped; the caller's extra
/// variables (`SpawnConfig::env`) are added on top. `VELLORA_*` carries the engine's own settings.
const INHERITED_ENV: &[&str] = &[
    "SystemRoot",
    // `CreateProcess` fails with ERROR_ENVVAR_NOT_FOUND for an AppContainer without it (it places
    // the container's own temporary and data directories relative to it).
    "LOCALAPPDATA",
    "SystemDrive",
    "windir",
    "OS",
    "PROCESSOR_ARCHITECTURE",
    "NUMBER_OF_PROCESSORS",
    "RUST_BACKTRACE",
];

/// Files whose ACL already grants the container read and execute in this process.
static GRANTED: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);

/// Adds the step that failed to an error, keeping its kind and OS code visible in the message.
fn at(step: &'static str) -> impl FnOnce(io::Error) -> io::Error {
    move |error| io::Error::new(error.kind(), format!("{step}: {error}"))
}

/// A process started by [`spawn`]. Offers what the engine client needs from
/// `std::process::Child`, with the same signatures.
#[derive(Debug)]
pub(crate) struct Child {
    process: OwnedHandle,
    id: u32,
}

impl Child {
    /// The operating-system process id.
    pub(crate) fn id(&self) -> u32 {
        self.id
    }

    /// The exit status if the process has ended.
    pub(crate) fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        self.wait_for(0)
    }

    /// Waits for the process to end.
    pub(crate) fn wait(&mut self) -> io::Result<ExitStatus> {
        self.wait_for(INFINITE)?
            .ok_or_else(|| io::Error::other("wait ended without an exit status"))
    }

    /// Ends the process. Does nothing, successfully, if it has already ended.
    pub(crate) fn kill(&mut self) -> io::Result<()> {
        // SAFETY: the handle is a process handle owned by `self`, with terminate access.
        if unsafe { TerminateProcess(self.process.as_raw_handle(), 1) } != 0 {
            return Ok(());
        }
        let error = io::Error::last_os_error();
        // Terminating a process that is already gone fails with "access denied".
        match self.try_wait() {
            Ok(Some(_)) => Ok(()),
            _ => Err(error),
        }
    }

    fn wait_for(&mut self, milliseconds: u32) -> io::Result<Option<ExitStatus>> {
        // SAFETY: the handle is a process handle owned by `self`, with synchronise access.
        match unsafe { WaitForSingleObject(self.process.as_raw_handle(), milliseconds) } {
            WAIT_OBJECT_0 => {
                let mut code = 0;
                // SAFETY: as above, with query access; `code` outlives the call.
                if unsafe { GetExitCodeProcess(self.process.as_raw_handle(), &raw mut code) } == 0 {
                    return Err(io::Error::last_os_error());
                }
                Ok(Some(ExitStatus::from_raw(code)))
            }
            WAIT_TIMEOUT => Ok(None),
            WAIT_FAILED => Err(io::Error::last_os_error()),
            other => Err(io::Error::other(format!("unexpected wait result {other}"))),
        }
    }
}

impl AsRawHandle for Child {
    fn as_raw_handle(&self) -> RawHandle {
        self.process.as_raw_handle()
    }
}

/// What to start.
pub(crate) struct Spec<'a> {
    pub(crate) engine: &'a Path,
    pub(crate) args: &'a [String],
    pub(crate) env: &'a [(OsString, OsString)],
    /// Inheritable handles to pass on, beyond the standard streams.
    pub(crate) inherit: &'a [RawHandle],
    pub(crate) limits: ResourceLimits,
}

/// A started engine: the process, its pipes and the job that confines it.
pub(crate) struct Started {
    pub(crate) child: Child,
    pub(crate) stdin: File,
    pub(crate) stdout: File,
    pub(crate) guard: Guard,
}

/// Starts the engine in the `AppContainer`, under the job, and lets it run.
///
/// # Errors
///
/// The operating system's error, from deriving the container, granting it the executable, creating
/// the process or the job. The process, if created, is killed first.
pub(crate) fn spawn(spec: &Spec<'_>) -> io::Result<Started> {
    let engine = std::path::absolute(spec.engine)?;
    let container = ContainerSid::new().map_err(at("deriving the container SID"))?;
    // The loader needs the container to read the executable and the library; see `grant_read`.
    for path in readable_files(&engine, spec.env) {
        grant_read(&path, container.0);
    }

    let (stdin_child, stdin_parent) = pipe().map_err(at("creating the pipes"))?;
    let (stdout_parent, stdout_child) = pipe().map_err(at("creating the pipes"))?;
    keep_private(&stdin_parent)?;
    keep_private(&stdout_parent)?;
    let stderr = inheritable_stderr().map_err(at("preparing standard error"))?;

    let mut handles: Vec<HANDLE> = vec![
        stdin_child.as_raw_handle(),
        stdout_child.as_raw_handle(),
        stderr.as_raw_handle(),
    ];
    handles.extend_from_slice(spec.inherit);

    let (process, thread) = create_suspended(
        spec,
        &engine,
        container.0,
        &handles,
        [
            stdin_child.as_raw_handle(),
            stdout_child.as_raw_handle(),
            stderr.as_raw_handle(),
        ],
    )
    .map_err(at("creating the process"))?;
    let mut child = Child {
        id: process.1,
        process: process.0,
    };

    // The job goes on before the first instruction runs.
    let guard = match limits::confine(&child, spec.limits) {
        Ok(guard) => guard,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            return Err(at("putting the process in its job")(error));
        }
    };
    // SAFETY: `thread` is the primary thread's handle from `CreateProcessW`, owned here.
    let resumed = unsafe { ResumeThread(thread.as_raw_handle()) };
    if resumed == u32::MAX {
        let error = io::Error::last_os_error();
        let _ = child.kill();
        let _ = child.wait();
        return Err(error);
    }
    Ok(Started {
        child,
        stdin: File::from(stdin_parent),
        stdout: File::from(stdout_parent),
        guard,
    })
}

/// The `AppContainer` SID derived from [`CONTAINER_NAME`].
struct ContainerSid(PSID);

impl ContainerSid {
    fn new() -> io::Result<Self> {
        let name = wide(OsStr::new(CONTAINER_NAME))?;
        let mut sid: PSID = ptr::null_mut();
        // SAFETY: `name` is a NUL-terminated string and `sid` receives the result.
        let status =
            unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &raw mut sid) };
        if status < 0 {
            // An HRESULT; `from_raw_os_error` shows the message of the matching Win32 code.
            return Err(io::Error::from_raw_os_error(status & 0xFFFF));
        }
        Ok(Self(sid))
    }
}

impl Drop for ContainerSid {
    fn drop(&mut self) {
        // SAFETY: the SID came from `DeriveAppContainerSidFromAppContainerName`, which documents
        // `FreeSid` as the way to release it, and is freed once.
        unsafe { FreeSid(self.0) };
    }
}

/// An anonymous pipe: the end the child inherits and the end the parent keeps. Both ends are
/// created inheritable; the parent's is then made private.
fn pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    // The pair is (read end, write end).
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_u32::<SECURITY_ATTRIBUTES>(),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: TRUE,
    };
    let (mut read, mut write): (HANDLE, HANDLE) = (ptr::null_mut(), ptr::null_mut());
    // SAFETY: both out-pointers are valid and `attributes` outlives the call.
    if unsafe { CreatePipe(&raw mut read, &raw mut write, &raw const attributes, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: two fresh handles, owned from here on.
    let (read, write) = unsafe {
        (
            OwnedHandle::from_raw_handle(read),
            OwnedHandle::from_raw_handle(write),
        )
    };
    Ok((read, write))
}

/// Stops `handle` from being inherited, so the engine never holds the UI's end of its own pipe.
fn keep_private(handle: &OwnedHandle) -> io::Result<()> {
    // SAFETY: the handle is valid and owned by the caller.
    if unsafe { SetHandleInformation(handle.as_raw_handle(), HANDLE_FLAG_INHERIT, 0) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The UI's standard error as an inheritable copy, or the null device when there is none or it is
/// not a file or a pipe (a console cannot be handed to a container).
fn inheritable_stderr() -> io::Result<OwnedHandle> {
    // SAFETY: `GetStdHandle` only reads the process's standard handle table.
    let own = unsafe { GetStdHandle(STD_ERROR_HANDLE) };
    // SAFETY: `GetFileType` accepts any handle value and reports "unknown" for an invalid one.
    // 1 = disk file, 3 = pipe.
    let usable = !own.is_null()
        && own != INVALID_HANDLE_VALUE
        && matches!(unsafe { GetFileType(own) }, 1 | 3);
    if usable {
        let mut copy: HANDLE = ptr::null_mut();
        // SAFETY: duplicates a handle of this process into this process, inheritable.
        let duplicated = unsafe {
            DuplicateHandle(
                GetCurrentProcess(),
                own,
                GetCurrentProcess(),
                &raw mut copy,
                0,
                TRUE,
                DUPLICATE_SAME_ACCESS,
            )
        };
        if duplicated != 0 {
            // SAFETY: a fresh handle, owned from here on.
            return Ok(unsafe { OwnedHandle::from_raw_handle(copy) });
        }
    }
    let nul = wide(OsStr::new("NUL"))?;
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_u32::<SECURITY_ATTRIBUTES>(),
        lpSecurityDescriptor: ptr::null_mut(),
        bInheritHandle: TRUE,
    };
    // SAFETY: `nul` is NUL-terminated and `attributes` outlives the call.
    let handle = unsafe {
        CreateFileW(
            nul.as_ptr(),
            0x4000_0000, // GENERIC_WRITE
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            &raw const attributes,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: a fresh handle, owned from here on.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
}

/// `CreateProcessW` with the container token and the handle list, suspended. Returns the process
/// (handle and id) and the primary thread's handle.
fn create_suspended(
    spec: &Spec<'_>,
    engine: &Path,
    container: PSID,
    handles: &[HANDLE],
    [stdin, stdout, stderr]: [HANDLE; 3],
) -> io::Result<((OwnedHandle, u32), OwnedHandle)> {
    let application = wide(engine.as_os_str())?;
    let mut command_line = command_line(engine, spec.args)?;
    let environment = environment(spec.env)?;
    let directory = system_directory();
    let directory = wide(directory.as_os_str())?;

    let capabilities = SECURITY_CAPABILITIES {
        AppContainerSid: container,
        Capabilities: ptr::null_mut(),
        CapabilityCount: 0,
        Reserved: 0,
    };
    let attributes = AttributeList::new(2)?;
    // SAFETY: `capabilities` and `handles` outlive the process creation below, which is the only
    // use of the attribute list.
    unsafe {
        attributes.set(
            PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
            (&raw const capabilities).cast(),
            size_of::<SECURITY_CAPABILITIES>(),
        )?;
        attributes.set(
            PROC_THREAD_ATTRIBUTE_HANDLE_LIST,
            handles.as_ptr().cast(),
            size_of_val(handles),
        )?;
    }

    // SAFETY: the structure is plain data for which all-zero means "unset".
    let mut startup: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    startup.StartupInfo.cb = size_u32::<STARTUPINFOEXW>();
    startup.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup.StartupInfo.hStdInput = stdin;
    startup.StartupInfo.hStdOutput = stdout;
    startup.StartupInfo.hStdError = stderr;
    startup.lpAttributeList = attributes.as_ptr();

    // SAFETY: the structure is plain data that `CreateProcessW` fills in.
    let mut info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    // SAFETY: every pointer is to a NUL-terminated or sized buffer that outlives the call, the
    // command line is mutable as the function requires, and the extended startup information is
    // flagged as such.
    let created = unsafe {
        CreateProcessW(
            application.as_ptr(),
            command_line.as_mut_ptr(),
            ptr::null(),
            ptr::null(),
            TRUE,
            EXTENDED_STARTUPINFO_PRESENT
                | CREATE_SUSPENDED
                | CREATE_UNICODE_ENVIRONMENT
                | CREATE_NO_WINDOW,
            environment.as_ptr().cast(),
            directory.as_ptr(),
            (&raw const startup).cast(),
            &raw mut info,
        )
    };
    if created == 0 {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: two fresh handles, owned from here on.
    Ok(unsafe {
        (
            (
                OwnedHandle::from_raw_handle(info.hProcess),
                info.dwProcessId,
            ),
            OwnedHandle::from_raw_handle(info.hThread),
        )
    })
}

/// A `PROC_THREAD_ATTRIBUTE_LIST`.
struct AttributeList {
    buffer: Vec<u8>,
}

impl AttributeList {
    fn new(count: u32) -> io::Result<Self> {
        let mut size = 0;
        // SAFETY: with a null list the call only reports the size needed (and fails on purpose).
        unsafe { InitializeProcThreadAttributeList(ptr::null_mut(), count, 0, &raw mut size) };
        let mut buffer = vec![0u8; size];
        // SAFETY: `buffer` has the size the first call asked for.
        let initialised = unsafe {
            InitializeProcThreadAttributeList(buffer.as_mut_ptr().cast(), count, 0, &raw mut size)
        };
        if initialised == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(Self { buffer })
    }

    fn as_ptr(&self) -> LPPROC_THREAD_ATTRIBUTE_LIST {
        self.buffer.as_ptr().cast_mut().cast()
    }

    /// # Safety
    ///
    /// `value` must point to `size` readable bytes that stay valid until the process is created.
    unsafe fn set(
        &self,
        attribute: u32,
        value: *const core::ffi::c_void,
        size: usize,
    ) -> io::Result<()> {
        // SAFETY: the list is initialised; the caller vouches for `value`.
        let updated = unsafe {
            UpdateProcThreadAttribute(
                self.as_ptr(),
                0,
                attribute as usize,
                value,
                size,
                ptr::null_mut(),
                ptr::null(),
            )
        };
        if updated == 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

impl Drop for AttributeList {
    fn drop(&mut self) {
        // SAFETY: the list was initialised in `new` and is deleted once.
        unsafe { DeleteProcThreadAttributeList(self.as_ptr()) };
    }
}

/// The files the container needs beyond what every container may read: the engine and the PDFium
/// library, which the engine looks for where `$VELLORA_PDFIUM_LIB` says (the caller's `env` wins
/// over ours) or next to itself.
fn readable_files(engine: &Path, env: &[(OsString, OsString)]) -> Vec<PathBuf> {
    let mut files = vec![engine.to_owned()];
    let override_path = env
        .iter()
        .rev()
        .find(|(name, _)| name.eq_ignore_ascii_case(PDFIUM_ENV))
        .map(|(_, value)| value.clone())
        .or_else(|| std::env::var_os(PDFIUM_ENV));
    match override_path {
        Some(path) => files.push(PathBuf::from(path)),
        None => files.extend(engine.parent().map(|dir| dir.join(PDFIUM_LIBRARY_WINDOWS))),
    }
    files
}

/// Lets the container read `path`: one ACL entry for its SID, read and execute. Best effort and
/// once per file and process: where the file is already readable to every container (an installed
/// application under `Program Files`) the user cannot change the ACL and does not need to, and
/// where it is really unreadable the process creation reports it.
fn grant_read(path: &Path, container: PSID) {
    {
        let mut granted = GRANTED
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if !granted
            .get_or_insert_with(HashSet::new)
            .insert(path.to_owned())
        {
            return;
        }
    }
    let Ok(name) = wide(path.as_os_str()) else {
        return;
    };
    let mut old: *mut ACL = ptr::null_mut();
    let mut descriptor: PSECURITY_DESCRIPTOR = ptr::null_mut();
    // SAFETY: `name` is NUL-terminated; the out-pointers are valid. The descriptor owns `old`.
    let read = unsafe {
        GetNamedSecurityInfoW(
            name.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            ptr::null_mut(),
            ptr::null_mut(),
            &raw mut old,
            ptr::null_mut(),
            &raw mut descriptor,
        )
    };
    if read != 0 {
        return;
    }
    let entry = EXPLICIT_ACCESS_W {
        grfAccessPermissions: FILE_GENERIC_READ | FILE_GENERIC_EXECUTE,
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: 0, // NO_INHERITANCE: the entry is for this file only
        Trustee: TRUSTEE_W {
            pMultipleTrustee: ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: TRUSTEE_IS_UNKNOWN,
            ptstrName: container.cast(),
        },
    };
    let mut new: *mut ACL = ptr::null_mut();
    // SAFETY: one valid entry and the file's current ACL; `new` receives a fresh ACL.
    let merged = unsafe { SetEntriesInAclW(1, &raw const entry, old, &raw mut new) };
    if merged == 0 {
        // SAFETY: `name` as above; `new` is the merged ACL. Failure is accepted (see above).
        unsafe {
            SetNamedSecurityInfoW(
                name.as_ptr().cast_mut(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                ptr::null_mut(),
                ptr::null_mut(),
                new,
                ptr::null_mut(),
            );
        }
        // SAFETY: `new` was allocated by `SetEntriesInAclW` for the caller to free with `LocalFree`.
        unsafe { LocalFree(new.cast()) };
    }
    // SAFETY: the security descriptor from `GetNamedSecurityInfoW` is freed with `LocalFree`.
    unsafe { LocalFree(descriptor) };
}

/// `"engine" arg arg ...`, quoted the way `CommandLineToArgvW` and the C runtime read it.
fn command_line(engine: &Path, args: &[String]) -> io::Result<Vec<u16>> {
    let mut line = vec![u16::from(b'"')];
    line.extend(no_nul(engine.as_os_str())?);
    line.push(u16::from(b'"'));
    for arg in args {
        line.push(u16::from(b' '));
        let arg: Vec<u16> = no_nul(OsStr::new(arg))?;
        let plain = !arg.is_empty()
            && !arg
                .iter()
                .any(|&c| c == u16::from(b' ') || c == u16::from(b'\t') || c == u16::from(b'"'));
        if plain {
            line.extend(arg);
            continue;
        }
        line.push(u16::from(b'"'));
        let mut backslashes = 0;
        for unit in arg {
            if unit == u16::from(b'\\') {
                backslashes += 1;
                continue;
            }
            if unit == u16::from(b'"') {
                // Backslashes before a quote are doubled, and the quote itself is escaped.
                line.extend(repeat_n(u16::from(b'\\'), backslashes * 2 + 1));
            } else {
                line.extend(repeat_n(u16::from(b'\\'), backslashes));
            }
            backslashes = 0;
            line.push(unit);
        }
        line.extend(repeat_n(u16::from(b'\\'), backslashes * 2));
        line.push(u16::from(b'"'));
    }
    line.push(0);
    Ok(line)
}

/// The engine's environment block: the allowlisted variables of this process, `VELLORA_*`, and
/// `extra`, sorted by name as `CreateProcess` expects, double-NUL terminated.
fn environment(extra: &[(OsString, OsString)]) -> io::Result<Vec<u16>> {
    let mut variables: Vec<(OsString, OsString)> = std::env::vars_os()
        .filter(|(name, _)| {
            let name = name.to_string_lossy();
            INHERITED_ENV
                .iter()
                .any(|keep| name.eq_ignore_ascii_case(keep))
                || name.to_ascii_uppercase().starts_with("VELLORA_")
        })
        .collect();
    for (name, value) in extra {
        variables.retain(|(existing, _)| !existing.eq_ignore_ascii_case(name));
        variables.push((name.clone(), value.clone()));
    }
    variables.sort_by_key(|(name, _)| name.to_string_lossy().to_uppercase());
    let mut block = Vec::new();
    for (name, value) in variables {
        block.extend(no_nul(&name)?);
        block.push(u16::from(b'='));
        block.extend(no_nul(&value)?);
        block.push(0);
    }
    // An empty block still needs its two terminators.
    block.push(0);
    if block.len() == 1 {
        block.push(0);
    }
    Ok(block)
}

/// `%SystemRoot%\System32`: a directory every container can enter, as the engine's working
/// directory (the UI's may be somewhere it cannot).
fn system_directory() -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
    Path::new(&root).join("System32")
}

fn no_nul(text: &OsStr) -> io::Result<Vec<u16>> {
    let units: Vec<u16> = text.encode_wide().collect();
    if units.contains(&0) {
        return Err(io::Error::from_raw_os_error(
            i32::try_from(ERROR_INVALID_PARAMETER).unwrap_or(87),
        ));
    }
    Ok(units)
}

/// `text` as a NUL-terminated UTF-16 string.
fn wide(text: &OsStr) -> io::Result<Vec<u16>> {
    let mut units = no_nul(text)?;
    units.push(0);
    Ok(units)
}

fn size_u32<T>() -> u32 {
    u32::try_from(size_of::<T>()).unwrap_or(u32::MAX)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use std::os::windows::ffi::OsStringExt;

    use super::*;

    fn text(units: &[u16]) -> String {
        OsString::from_wide(units).to_string_lossy().into_owned()
    }

    #[test]
    fn plain_arguments_are_not_quoted_and_awkward_ones_are() {
        let line = command_line(
            Path::new(r"C:\a b\engine.exe"),
            &[
                "--file-handle".into(),
                "12".into(),
                String::new(),
                "two words".into(),
                r#"say "hi""#.into(),
                r"ends\".into(),
                r"with space\".into(),
            ],
        )
        .unwrap();
        assert_eq!(
            text(&line[..line.len() - 1]),
            r#""C:\a b\engine.exe" --file-handle 12 "" "two words" "say \"hi\"" ends\ "with space\\""#
        );
        assert_eq!(line.last(), Some(&0));
    }

    #[test]
    fn the_environment_is_sorted_filtered_and_double_terminated() {
        let block =
            environment(&[("VELLORA_X".into(), "1".into()), ("a".into(), "b".into())]).unwrap();
        let text = text(&block);
        assert!(text.ends_with("\0\0"), "{text:?}");
        let names: Vec<&str> = text
            .split('\0')
            .filter(|entry| !entry.is_empty())
            .map(|entry| entry.split('=').next().unwrap())
            .collect();
        let mut sorted = names.clone();
        sorted.sort_by_key(|name| name.to_uppercase());
        assert_eq!(names, sorted);
        assert!(names.contains(&"a") && names.contains(&"VELLORA_X"));
        assert!(
            !names
                .iter()
                .any(|name| name.eq_ignore_ascii_case("USERPROFILE")),
            "{names:?}"
        );
    }

    #[test]
    fn a_nul_in_an_argument_is_refused() {
        assert!(command_line(Path::new("engine.exe"), &["a\0b".into()]).is_err());
    }
}
