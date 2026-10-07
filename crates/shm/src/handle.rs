//! Passing open files from the UI to the engine as inherited handles.
//!
//! The engine never receives a path (ADR-0004): the UI opens the document and the shared region
//! and hands the engine the open files. A child process inherits a file if the parent marks it
//! inheritable and then spawns it; the child learns the file's number on its command line and
//! adopts it with [`adopt`].
//!
//! ```text
//! parent:  let token = share_with_child(&file)?;   // mark inheritable
//!          Command::new(engine).arg("--file-handle").arg(token.to_string()).spawn()?;
//!          stop_sharing(&file)?;                   // later children no longer get it
//! child:   let file = adopt(token)?;
//! ```
//!
//! Between `share_with_child` and `stop_sharing`, any process the parent spawns on another thread
//! inherits the file as well. Spawn engines from one place (the engine client serialises its
//! spawns) and the window stays small.

use std::fmt;
use std::fs::File;
use std::num::ParseIntError;
use std::str::FromStr;

use crate::Error;

/// The number that names an inherited file in the child: a file descriptor on Unix, a handle
/// value on Windows. It means nothing in any other process.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct HandleToken(u64);

impl HandleToken {
    /// Wraps a raw number, as parsed from a command line or received in a message.
    #[must_use]
    pub const fn new(raw: u64) -> Self {
        Self(raw)
    }

    /// The raw number.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl fmt::Display for HandleToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl FromStr for HandleToken {
    type Err = ParseIntError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        text.parse().map(Self)
    }
}

/// Marks `file` inheritable by child processes and returns the number the child adopts it by.
///
/// # Errors
///
/// [`Error::Io`] if the operating system refuses.
pub fn share_with_child(file: &File) -> Result<HandleToken, Error> {
    os::set_inheritable(file, true)
        .map_err(|source| Error::io("cannot make the file inheritable", source))?;
    Ok(os::token_of(file))
}

/// Undoes [`share_with_child`], once the child has been spawned.
///
/// # Errors
///
/// [`Error::Io`] if the operating system refuses.
pub fn stop_sharing(file: &File) -> Result<(), Error> {
    os::set_inheritable(file, false)
        .map_err(|source| Error::io("cannot stop sharing the file", source))
}

/// Takes ownership of the inherited open file named by `token`.
///
/// The number comes from the command line of a process the engine does not control, so it is
/// checked before it is trusted: it must not be a standard stream, must name an open handle, and
/// that handle must be a regular file. The file is made non-inheritable again, so the engine's
/// own children (it has none) would not receive it.
///
/// Call it at most once per token: the returned [`File`] owns the handle and closes it on drop.
///
/// # Errors
///
/// [`Error::InvalidHandle`] if `token` is not a usable open file.
pub fn adopt(token: HandleToken) -> Result<File, Error> {
    let file = os::adopt(token)?;
    let is_file = file.metadata().is_ok_and(|meta| meta.is_file());
    if !is_file {
        return Err(Error::InvalidHandle {
            token: token.get(),
            reason: "it is not a regular file",
        });
    }
    // Best effort: the engine spawns nothing, so a failure here changes nothing.
    let _ = os::set_inheritable(&file, false);
    Ok(file)
}

#[cfg(unix)]
mod os {
    use std::fs::File;
    use std::io;
    use std::os::fd::{AsRawFd, FromRawFd};

    use super::HandleToken;
    use crate::Error;

    fn flags(fd: libc::c_int) -> io::Result<libc::c_int> {
        // SAFETY: `fcntl` with `F_GETFD` only reads the flags of a descriptor number; it cannot
        // touch memory, and an invalid number fails with `EBADF`.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(flags)
        }
    }

    pub(super) fn set_inheritable(file: &File, inheritable: bool) -> io::Result<()> {
        let fd = file.as_raw_fd();
        let current = flags(fd)?;
        let wanted = if inheritable {
            current & !libc::FD_CLOEXEC
        } else {
            current | libc::FD_CLOEXEC
        };
        // SAFETY: `fd` is owned by `file`, which the caller keeps alive for this call, and
        // `F_SETFD` only changes the descriptor's flags.
        if unsafe { libc::fcntl(fd, libc::F_SETFD, wanted) } < 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    // A descriptor number is never negative, so the sign loss cannot happen.
    #[allow(clippy::cast_sign_loss)]
    pub(super) fn token_of(file: &File) -> HandleToken {
        HandleToken::new(file.as_raw_fd() as u64)
    }

    pub(super) fn adopt(token: HandleToken) -> Result<File, Error> {
        let invalid = |reason| Error::InvalidHandle {
            token: token.get(),
            reason,
        };
        let fd =
            libc::c_int::try_from(token.get()).map_err(|_| invalid("not a descriptor number"))?;
        // 0, 1 and 2 are the standard streams: stdin and stdout carry the IPC pipes.
        if fd <= 2 {
            return Err(invalid("it is a standard stream"));
        }
        flags(fd).map_err(|_| invalid("it is not an open descriptor"))?;
        // SAFETY: `fd` was just shown to be an open descriptor and is not a standard stream. By
        // the contract of `adopt` it was passed to this process to be owned here and is adopted
        // once, so no other `File` or `OwnedFd` closes it.
        Ok(unsafe { File::from_raw_fd(fd) })
    }
}

#[cfg(windows)]
mod os {
    use std::fs::File;
    use std::io;
    use std::os::windows::io::{AsRawHandle, FromRawHandle, RawHandle};

    use windows_sys::Win32::Foundation::{
        FALSE, GetHandleInformation, HANDLE_FLAG_INHERIT, INVALID_HANDLE_VALUE,
        SetHandleInformation,
    };
    use windows_sys::Win32::Storage::FileSystem::{FILE_TYPE_DISK, GetFileType};

    use super::HandleToken;
    use crate::Error;

    pub(super) fn set_inheritable(file: &File, inheritable: bool) -> io::Result<()> {
        let handle: RawHandle = file.as_raw_handle();
        let value = if inheritable { HANDLE_FLAG_INHERIT } else { 0 };
        // SAFETY: `handle` is owned by `file`, which the caller keeps alive for this call, and
        // `SetHandleInformation` only changes the handle's flags.
        if unsafe { SetHandleInformation(handle, HANDLE_FLAG_INHERIT, value) } == FALSE {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }

    pub(super) fn token_of(file: &File) -> HandleToken {
        HandleToken::new(file.as_raw_handle().expose_provenance() as u64)
    }

    pub(super) fn adopt(token: HandleToken) -> Result<File, Error> {
        let invalid = |reason| Error::InvalidHandle {
            token: token.get(),
            reason,
        };
        let address = usize::try_from(token.get()).map_err(|_| invalid("not a handle value"))?;
        let handle: RawHandle = std::ptr::with_exposed_provenance_mut(address);
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return Err(invalid("it is not a handle value"));
        }
        let mut flags = 0_u32;
        // SAFETY: `GetHandleInformation` writes one `u32` through the valid pointer to `flags`
        // and reports an unknown handle value as an error instead of dereferencing it.
        if unsafe { GetHandleInformation(handle, &raw mut flags) } == FALSE {
            return Err(invalid("it is not an open handle"));
        }
        // SAFETY: the handle was just shown to be open; `GetFileType` only queries it.
        if unsafe { GetFileType(handle) } != FILE_TYPE_DISK {
            return Err(invalid("it is not a disk file"));
        }
        // SAFETY: the handle is open and, by the contract of `adopt`, was passed to this process
        // to be owned here and is adopted once, so nothing else closes it.
        Ok(unsafe { File::from_raw_handle(handle) })
    }
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use super::*;

    /// Relinquishes `file`'s ownership and returns the token that [`adopt`] takes back.
    fn release(file: File) -> HandleToken {
        #[cfg(unix)]
        {
            use std::os::fd::IntoRawFd;
            #[allow(clippy::cast_sign_loss)]
            HandleToken::new(file.into_raw_fd() as u64)
        }
        #[cfg(windows)]
        {
            use std::os::windows::io::IntoRawHandle;
            HandleToken::new(file.into_raw_handle().expose_provenance() as u64)
        }
    }

    #[test]
    fn tokens_print_and_parse() {
        let token = HandleToken::new(1234);
        assert_eq!(token.to_string(), "1234");
        assert_eq!("1234".parse::<HandleToken>().unwrap(), token);
        assert!("".parse::<HandleToken>().is_err());
        assert!("-1".parse::<HandleToken>().is_err());
        assert!("0x10".parse::<HandleToken>().is_err());
    }

    #[test]
    fn an_open_file_is_adopted_and_stays_usable() {
        let mut original = tempfile::tempfile().unwrap();
        original.write_all(b"hello").unwrap();
        let token = release(original);

        let mut adopted = adopt(token).unwrap();
        adopted.seek(SeekFrom::Start(0)).unwrap();
        let mut text = String::new();
        adopted.read_to_string(&mut text).unwrap();
        assert_eq!(text, "hello");
    }

    #[test]
    fn sharing_can_be_switched_on_and_off() {
        let file = tempfile::tempfile().unwrap();
        let first = share_with_child(&file).unwrap();
        let second = share_with_child(&file).unwrap();
        assert_eq!(first, second, "the token names the file, not the call");
        stop_sharing(&file).unwrap();
        stop_sharing(&file).unwrap();
    }

    #[test]
    fn numbers_that_are_not_usable_files_are_refused() {
        for raw in [0, 1, 2, u64::MAX, u64::from(u32::MAX), 1 << 40] {
            let error = adopt(HandleToken::new(raw)).unwrap_err();
            assert!(
                matches!(error, Error::InvalidHandle { token, .. } if token == raw),
                "{raw}: {error}"
            );
        }
    }
}
