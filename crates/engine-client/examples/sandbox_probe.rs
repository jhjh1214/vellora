//! A stand-in for the engine that tries what a compromised engine would try and reports each
//! outcome on its standard output, one `name: result` line per probe, for `tests/sandbox.rs`
//! (Windows only; elsewhere it does nothing). Not part of the product: it is an example only so
//! that `cargo test` builds it next to the test that starts it, through the engine client's own
//! spawn path.
//!
//! What it is asked to attack comes from the environment:
//! `VELLORA_PROBE_FILE` (a file in the user's profile), `VELLORA_PROBE_ADDR` and
//! `VELLORA_PROBE_UDP` (a listening TCP and a UDP socket on this machine). After the last line it prints `done` and stays alive until its standard input
//! closes, so the parent can look at what the process still holds.

#![allow(unsafe_code)]

#[cfg(windows)]
#[allow(clippy::too_many_lines)] // a straight list of probes, clearer in one piece
fn main() {
    use std::io::{Read, Seek, SeekFrom, Write};
    use std::{env, fs, net, process, time};

    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::Security::{
        GetTokenInformation, TOKEN_GROUPS, TOKEN_QUERY, TokenCapabilities, TokenIsAppContainer,
    };
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    fn report(name: &str, result: impl std::fmt::Display) {
        println!("{name}: {result}");
    }

    fn describe(error: &std::io::Error) -> String {
        format!(
            "{:?} (os error {})",
            error.kind(),
            error.raw_os_error().unwrap_or(0)
        )
    }

    // Whether the token is an AppContainer, and how many capabilities it holds.
    let mut token: HANDLE = std::ptr::null_mut();
    // SAFETY: the out-pointer is valid; the pseudo-handle of the current process needs no closing.
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &raw mut token) } == 0 {
        report("container", "no token");
    } else {
        let mut is_container: u32 = 0;
        let mut returned = 0;
        // SAFETY: the buffer is a `u32`, the size given.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenIsAppContainer,
                (&raw mut is_container).cast(),
                4,
                &raw mut returned,
            )
        };
        report(
            "container",
            if ok != 0 && is_container == 1 {
                "yes"
            } else {
                "no"
            },
        );

        let mut buffer = [0u8; 4096];
        // SAFETY: the buffer is as large as the size given.
        let ok = unsafe {
            GetTokenInformation(
                token,
                TokenCapabilities,
                buffer.as_mut_ptr().cast(),
                4096,
                &raw mut returned,
            )
        };
        let capabilities = if ok != 0 {
            // SAFETY: on success the buffer starts with a `TOKEN_GROUPS`; it is read unaligned.
            unsafe { std::ptr::read_unaligned(buffer.as_ptr().cast::<TOKEN_GROUPS>()).GroupCount }
        } else {
            u32::MAX
        };
        report("capabilities", capabilities);
        // SAFETY: the token handle was opened above and is closed once.
        unsafe { CloseHandle(token) };
    }

    // The document arrives as an inherited handle and is readable through it.
    let handle = env::args()
        .skip_while(|arg| arg != "--file-handle")
        .nth(1)
        .and_then(|value| value.parse().ok())
        .map(vellora_shm::HandleToken::new);
    match handle.map(vellora_shm::adopt) {
        Some(Ok(mut file)) => {
            let mut head = [0u8; 8];
            // The parent wrote the file through this very handle, so the offset is at the end.
            match file
                .seek(SeekFrom::Start(0))
                .and_then(|_| file.read_exact(&mut head))
            {
                Ok(()) => report("document", String::from_utf8_lossy(&head)),
                Err(error) => report("document", format!("unreadable: {}", describe(&error))),
            }
        }
        Some(Err(error)) => report("document", format!("not adopted: {error}")),
        None => report("document", "no handle on the command line"),
    }

    // Files in the user's profile.
    match env::var_os("VELLORA_PROBE_FILE") {
        Some(path) => match fs::File::open(path) {
            Ok(_) => report("profile file", "opened"),
            Err(error) => report("profile file", format!("refused: {}", describe(&error))),
        },
        None => report("profile file", "not asked"),
    }

    // The network, loopback included.
    match env::var("VELLORA_PROBE_ADDR")
        .ok()
        .and_then(|addr| addr.parse::<net::SocketAddr>().ok())
    {
        Some(addr) => {
            match net::TcpStream::connect_timeout(&addr, time::Duration::from_secs(3)) {
                Ok(_) => report("tcp connect", "connected"),
                Err(error) => report("tcp connect", format!("refused: {}", describe(&error))),
            }
            // A container may still create and bind a socket; what it cannot do is carry traffic.
            match net::UdpSocket::bind("127.0.0.1:0") {
                Ok(socket) => {
                    report("udp bind", "bound");
                    match env::var("VELLORA_PROBE_UDP")
                        .ok()
                        .and_then(|a| a.parse::<net::SocketAddr>().ok())
                    {
                        Some(target) => match socket.send_to(b"hello", target) {
                            Ok(_) => report("udp send", "sent"),
                            Err(error) => {
                                report("udp send", format!("refused: {}", describe(&error)));
                            }
                        },
                        None => report("udp send", "not asked"),
                    }
                }
                Err(error) => report("udp bind", format!("refused: {}", describe(&error))),
            }
        }
        None => report("tcp connect", "not asked"),
    }

    // A child process.
    let system = env::var_os("SystemRoot").unwrap_or_else(|| r"C:\Windows".into());
    let cmd = std::path::Path::new(&system)
        .join("System32")
        .join("cmd.exe");
    match process::Command::new(cmd).args(["/C", "exit", "0"]).spawn() {
        Ok(mut child) => {
            let _ = child.wait();
            report("child process", "started");
        }
        Err(error) => report("child process", format!("refused: {}", describe(&error))),
    }

    // What the container may read: the system fonts PDFium uses.
    let font = std::path::Path::new(&system)
        .join("Fonts")
        .join("arial.ttf");
    match fs::read(&font) {
        Ok(bytes) => report("system font", format!("read {} bytes", bytes.len())),
        Err(error) => report("system font", format!("refused: {}", describe(&error))),
    }

    // Nothing of the user's environment arrives.
    report(
        "USERPROFILE",
        if env::var_os("USERPROFILE").is_some() {
            "present"
        } else {
            "absent"
        },
    );

    report("done", "yes");
    let _ = std::io::stdout().flush();
    // Held open until the parent closes our standard input.
    let _ = std::io::stdin().read_to_end(&mut Vec::new());
}

#[cfg(not(windows))]
fn main() {}
