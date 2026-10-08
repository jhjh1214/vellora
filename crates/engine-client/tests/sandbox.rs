//! The Windows sandbox of the engine (M1 task 4, ADR-0017): what a compromised engine cannot do.
//!
//! `examples/sandbox_probe.rs` stands in for the engine, is started through `EngineProcess::spawn`
//! (the code the UI uses), tries each forbidden thing and reports the outcome. Every refusal is
//! paired with a control that shows the same action works outside the sandbox (this test process
//! reads the file, connects to the socket) or inside it (the document handle, the system fonts), so
//! a refusal cannot be an accident of the setup.
//!
//! No PDFium is needed. Windows only.

#![cfg(windows)]
// Test code: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::env;
use std::fs;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

use vellora_engine_client::{EngineProcess, SpawnConfig, Termination};
use vellora_shm::{SlotGeometry, TileRegion, share_with_child, stop_sharing};

/// Where `cargo test` put the probe: next to the test executables, in `examples/`.
fn probe() -> PathBuf {
    let exe = env::current_exe().unwrap();
    let target = exe.parent().and_then(Path::parent).unwrap();
    let path = target
        .join("examples")
        .join(format!("sandbox_probe{}", env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "{} is missing: run `cargo test -p vellora-engine-client` (it builds the examples)",
        path.display()
    );
    path
}

/// Runs the probe in the sandbox and returns its report as `name -> result`.
fn run_probe() -> HashMap<String, String> {
    // A file in the user's profile (the temporary directory is under it), with the control that
    // this process can read it.
    let dir = tempfile::tempdir().unwrap();
    let secret = dir.path().join("secret.txt");
    fs::write(&secret, "the user's own file").unwrap();
    assert_eq!(fs::read_to_string(&secret).unwrap(), "the user's own file");

    // A socket on this machine, with the control that this process can connect to it.
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    std::net::TcpStream::connect(address).expect("control: this process can connect");
    listener.set_nonblocking(true).unwrap();
    listener
        .accept()
        .expect("control: and the listener sees it");
    let receiver = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    let udp_address = receiver.local_addr().unwrap();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .unwrap();
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .send_to(b"control", udp_address)
        .unwrap();
    let mut datagram = [0u8; 16];
    assert_eq!(
        receiver.recv_from(&mut datagram).unwrap().0,
        7,
        "control: and so does a datagram"
    );

    // The write end of a pipe that stays inheritable but is not on the list the engine client
    // passes. If the engine got it, the read end would not see end-of-file once this process lets
    // go of its own copy.
    let (mut leak_reader, leak_writer) = std::io::pipe().unwrap();
    let leak_writer = fs::File::from(std::os::windows::io::OwnedHandle::from(leak_writer));
    share_with_child(&leak_writer).unwrap();

    let mut document = tempfile::tempfile().unwrap();
    document.write_all(b"%PDF-1.4\n").unwrap();
    let geometry = SlotGeometry::new(1, 4096).unwrap();
    let (_region, region_file) = TileRegion::create(geometry).unwrap();

    let probe = probe();
    let mut config = SpawnConfig::new(&probe, &document, &region_file, geometry);
    config
        .env
        .push(("VELLORA_PROBE_FILE".into(), secret.into()));
    config
        .env
        .push(("VELLORA_PROBE_ADDR".into(), address.to_string().into()));
    config
        .env
        .push(("VELLORA_PROBE_UDP".into(), udp_address.to_string().into()));
    let mut process = EngineProcess::spawn(&config).unwrap();
    stop_sharing(&leak_writer).unwrap();

    // The report ends with `done`; the probe then waits for its standard input to close.
    let mut report = String::new();
    let mut output = BufReader::new(process.take_stdout().unwrap());
    loop {
        let mut line = String::new();
        let read = output.read_line(&mut line).unwrap();
        report.push_str(&line);
        if read == 0 || line.starts_with("done: ") {
            break;
        }
    }
    assert!(
        report.contains("done: yes"),
        "the probe ended abnormally ({:?}); it said:\n{report}",
        process.wait_timeout(Duration::from_secs(5))
    );

    // The engine did not inherit the pipe: with our copy closed, the reader sees end-of-file.
    drop(leak_writer);
    let (sender, eof) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = sender.send(leak_reader.read_to_end(&mut Vec::new()).is_ok());
    });
    let leak_closed = eof.recv_timeout(Duration::from_secs(5)).is_ok();

    drop(process.take_stdin());
    assert_eq!(
        process.wait_timeout(Duration::from_secs(30)).unwrap(),
        Some(Termination::Success),
        "the probe ended abnormally; it said:\n{report}"
    );
    assert!(
        leak_closed,
        "the engine inherited a handle that was not on the list"
    );
    // The listener saw nothing from the engine.
    assert!(
        listener.accept().is_err(),
        "the sandboxed engine reached a socket on this machine"
    );
    assert!(
        receiver.recv_from(&mut datagram).is_err(),
        "a datagram from the sandboxed engine arrived"
    );
    eprintln!("probe report:\n{report}");
    report
        .lines()
        .filter_map(|line| line.split_once(": "))
        .map(|(name, result)| (name.to_owned(), result.to_owned()))
        .collect()
}

#[test]
fn a_sandboxed_engine_cannot_reach_what_a_compromised_one_would_want() {
    let report = run_probe();
    let said = |name: &str| {
        report
            .get(name)
            .unwrap_or_else(|| panic!("no line for {name}"))
    };

    // It really is in an AppContainer, with no capability.
    assert_eq!(said("container"), "yes");
    assert_eq!(said("capabilities"), "0");

    // Controls: what the engine needs still works.
    assert_eq!(
        said("document"),
        "%PDF-1.4",
        "the inherited document handle"
    );
    assert!(
        said("system font").starts_with("read "),
        "PDFium needs the system fonts: {}",
        said("system font")
    );

    // 1. A file in the user's profile: access denied.
    assert!(
        said("profile file").starts_with("refused: PermissionDenied (os error 5)"),
        "{}",
        said("profile file")
    );
    // 2. The network, even this machine's loopback. A container can still create and bind a
    //    socket (`udp bind`); it cannot connect one or get a datagram out, and `run_probe` checks
    //    that the listeners saw nothing.
    assert!(
        said("tcp connect").starts_with("refused:"),
        "{}",
        said("tcp connect")
    );
    // 3. A child process.
    assert!(
        said("child process").starts_with("refused:"),
        "{}",
        said("child process")
    );

    // And the quieter thing: nothing of the user's environment arrives (the handle list is checked
    // in `run_probe`).
    assert_eq!(said("USERPROFILE"), "absent");
}
