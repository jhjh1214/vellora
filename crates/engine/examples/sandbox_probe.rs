//! Tries what a compromised engine would try, once before and once after `sandbox::apply`, and
//! prints one `name: before=<outcome> after=<outcome>` line per probe for `tests/sandbox_linux.rs`.
//! Linux only (elsewhere it does nothing). Not part of the product: an example only so that
//! `cargo test` builds it next to the test that runs it.
//!
//! The "before" column is the control: a refusal after `apply` means something only if the same
//! action worked before it.

#[cfg(target_os = "linux")]
fn main() {
    use std::fs;
    use std::io;
    use std::net::UdpSocket;
    use std::process::{Command, Stdio};
    use std::thread;

    fn outcome<T>(result: io::Result<T>) -> String {
        match result {
            Ok(_) => "ok".into(),
            Err(error) => format!("err({})", error.raw_os_error().unwrap_or(0)),
        }
    }

    type Probe = Box<dyn Fn() -> String + Sync>;
    let probes: Vec<(&str, Probe)> = vec![
        // A file the engine has no business with.
        (
            "read a file outside the allowed set",
            Box::new(|| outcome(fs::File::open("/etc/passwd"))),
        ),
        (
            "create a file",
            Box::new(|| {
                let path =
                    std::env::temp_dir().join(format!("vellora-probe-{}", std::process::id()));
                let result = fs::File::create(&path);
                let _ = fs::remove_file(&path);
                outcome(result)
            }),
        ),
        // The UI's environment, readable by the same user.
        (
            "read the parent's /proc entry",
            Box::new(|| {
                let parent = std::os::unix::process::parent_id();
                outcome(fs::read(format!("/proc/{parent}/environ")))
            }),
        ),
        (
            "a socket",
            Box::new(|| outcome(UdpSocket::bind("127.0.0.1:0"))),
        ),
        (
            "start a program",
            Box::new(|| outcome(Command::new("/bin/true").stdin(Stdio::null()).status())),
        ),
        // Controls that must keep working.
        (
            "read the library directory",
            Box::new(|| outcome(fs::read_dir("/usr/lib").map(Iterator::count))),
        ),
        (
            "read its own /proc entry",
            Box::new(|| outcome(fs::read("/proc/self/status"))),
        ),
        (
            "start a thread",
            Box::new(|| {
                outcome(
                    thread::spawn(|| 1)
                        .join()
                        .map_err(|_| io::Error::other("panic")),
                )
            }),
        ),
    ];

    let before: Vec<String> = probes.iter().map(|(_, probe)| probe()).collect();
    let report = vellora_engine::sandbox::apply(&[]);
    println!("landlock: {:?}", report.landlock);
    println!("seccomp: {:?}", report.seccomp);
    // Threads created after `apply` are filtered too: run the probes on a fresh one.
    let after: Vec<String> = thread::scope(|scope| {
        scope
            .spawn(|| probes.iter().map(|(_, probe)| probe()).collect())
            .join()
            .unwrap_or_default()
    });
    for (((name, _), before), after) in probes.iter().zip(&before).zip(&after) {
        println!("{name}: before={before} after={after}");
    }
}

#[cfg(not(target_os = "linux"))]
fn main() {}
