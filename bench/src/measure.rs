//! The measurements: open → first page through the real engine process, peak memory of that
//! process, and the cost of a commit followed by a re-open.
//!
//! # What is timed
//!
//! - **cos open:** `ObjectStore::open` over bytes already in memory (the cross-reference chain
//!   only; no page is read). The file read is not included.
//! - **open → first page:** from `Client::open` (spawns the engine, which maps the file) to the
//!   `TileReady` of page 1 at 100 %, a full letter-size tile. This is what the UI waits for.
//! - **commit:** building the incremental section with `cos`, then appending it to the file
//!   and `fsync`. The file is cut back to its original length afterwards, so runs repeat.
//! - **re-open after commit:** a new `Client::open` of the appended file, to the first tile.
//!
//! Each measurement starts a fresh engine process. The file is in the OS cache after the
//! generator or the first run wrote or read it, so these are warm-cache numbers.

use std::env;
use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use vellora_cos::{Limits, ObjectStore};
use vellora_engine_client::{Client, ClientConfig, Event, TileRequest};
use vellora_ipc::{Priority, SlotId, TileRect};
use vellora_shm::SlotGeometry;

use crate::Result;
use crate::generate::{Kind, commit_section};

/// How long to wait for the engine before calling a measurement failed.
const PATIENCE: Duration = Duration::from_secs(120);
/// Size of the "inserted image" in the large commit.
const LARGE_COMMIT_BYTES: usize = 5 << 20;
/// One slot holds a full letter-size page at 100 %: 612 x 792 x 4 bytes, rounded up.
const SLOT_BYTES: u32 = 2 << 20;

/// Where the engine executable and the PDFium library are.
pub(crate) struct Setup {
    engine: PathBuf,
    pdfium: PathBuf,
}

impl Setup {
    /// Explicit paths win; otherwise the engine is the sibling of this executable (both are in
    /// `target/release`) and PDFium is `VELLORA_PDFIUM_LIB` or the build `cargo xtask pdfium
    /// fetch` installed.
    pub(crate) fn locate(engine: Option<PathBuf>, pdfium: Option<PathBuf>) -> Result<Self> {
        let engine = if let Some(path) = engine {
            path
        } else {
            let name = format!("vellora-engine{}", env::consts::EXE_SUFFIX);
            env::current_exe()?.with_file_name(name)
        };
        let pdfium = pdfium
            .or_else(|| env::var_os("VELLORA_PDFIUM_LIB").map(PathBuf::from))
            .unwrap_or_else(default_pdfium);
        for (what, path) in [("engine", &engine), ("PDFium", &pdfium)] {
            if !path.is_file() {
                return Err(format!(
                    "{what} not found at {} (build it with `cargo build --release -p vellora-engine`, fetch PDFium with `cargo xtask pdfium fetch`)",
                    path.display()
                )
                .into());
            }
        }
        Ok(Self { engine, pdfium })
    }
}

fn default_pdfium() -> PathBuf {
    let (platform, library) = if cfg!(windows) {
        ("win-x64", "bin/pdfium.dll")
    } else if cfg!(target_os = "macos") {
        let platform = if cfg!(target_arch = "aarch64") {
            "mac-arm64"
        } else {
            "mac-x64"
        };
        (platform, "lib/libpdfium.dylib")
    } else {
        ("linux-x64", "lib/libpdfium.so")
    };
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../third_party/pdfium")
        .join(platform)
        .join(library)
}

/// One open of a document in a fresh engine.
struct Opened {
    /// Spawn to `Opened`.
    to_open: Duration,
    /// Spawn to the first tile.
    to_first_page: Duration,
    /// Peak memory of the engine process, if the platform can say.
    peak: Option<Peak>,
    pages: u32,
}

/// Peak (or, where the OS offers nothing better, current) memory of a process.
#[derive(Clone, Copy)]
struct Peak {
    bytes: u64,
    is_peak: bool,
}

fn open_to_first_page(setup: &Setup, path: &Path) -> Result<Opened> {
    let mut config = ClientConfig::new(&setup.engine, SlotGeometry::new(2, SLOT_BYTES)?);
    config.env.push((
        vellora_render_env().into(),
        setup.pdfium.clone().into_os_string(),
    ));
    let started = Instant::now();
    let client = Client::open(config, path)?;
    let mut to_open = None;
    let mut pages = 0;
    let mut request = None;
    let deadline = started + PATIENCE;
    let to_first_page = loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .ok_or("the engine did not deliver the first page in time")?;
        let mut done = None;
        for event in client.wait_events(left) {
            match event {
                Event::Opened { page_count, .. } if to_open.is_none() => {
                    to_open = Some(started.elapsed());
                    pages = page_count;
                    request = Some(client.request_tile(&TileRequest {
                        page: 0,
                        scale: 1.0,
                        rect: TileRect {
                            x: 0,
                            y: 0,
                            width: 612,
                            height: 792,
                        },
                        slot: SlotId(0),
                        priority: Priority::Visible,
                    })?);
                }
                Event::TileReady { request: r, .. } if Some(r) == request => {
                    done = Some(started.elapsed());
                }
                Event::RequestFailed { message, .. } => {
                    return Err(format!("the engine reported: {message}").into());
                }
                Event::EngineCrashed { crash, .. } => {
                    return Err(format!("the engine crashed: {crash:?}").into());
                }
                Event::Failed { reason } => return Err(reason.into()),
                _ => {}
            }
        }
        if let Some(done) = done {
            break done;
        }
    };
    // A timing of a blank page would be a timing of nothing: the pages are never all white.
    let mut pixels = vec![0_u8; SLOT_BYTES as usize];
    client.read_slot(SlotId(0), &mut pixels)?;
    if pixels[..612 * 792 * 4]
        .as_chunks::<4>()
        .0
        .iter()
        .all(|px| px[..3] == [0xFF, 0xFF, 0xFF])
    {
        return Err("the first page rendered blank".into());
    }
    let peak = client.engine_id().and_then(peak_memory);
    client.close();
    Ok(Opened {
        to_open: to_open.unwrap_or(to_first_page),
        to_first_page,
        peak,
        pages,
    })
}

/// The environment variable the engine reads for PDFium (`vellora_render::LIBRARY_ENV`; the
/// harness does not link the renderer).
const fn vellora_render_env() -> &'static str {
    "VELLORA_PDFIUM_LIB"
}

#[cfg(windows)]
fn peak_memory(pid: u32) -> Option<Peak> {
    let script = format!("(Get-Process -Id {pid}).PeakWorkingSet64");
    let out = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .output()
        .ok()?;
    let bytes = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(Peak {
        bytes,
        is_peak: true,
    })
}

#[cfg(target_os = "linux")]
fn peak_memory(pid: u32) -> Option<Peak> {
    let status = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    let kib: u64 = status
        .lines()
        .find_map(|line| line.strip_prefix("VmHWM:"))?
        .trim()
        .trim_end_matches("kB")
        .trim()
        .parse()
        .ok()?;
    Some(Peak {
        bytes: kib * 1024,
        is_peak: true,
    })
}

#[cfg(not(any(windows, target_os = "linux")))]
fn peak_memory(pid: u32) -> Option<Peak> {
    // No peak counter without platform calls (`unsafe` is forbidden here): sample the current RSS.
    let out = std::process::Command::new("ps")
        .args(["-o", "rss=", "-p", &pid.to_string()])
        .output()
        .ok()?;
    let kib: u64 = String::from_utf8_lossy(&out.stdout).trim().parse().ok()?;
    Some(Peak {
        bytes: kib * 1024,
        is_peak: false,
    })
}

/// The commit costs for one section size.
struct Commit {
    label: &'static str,
    section_bytes: usize,
    build: Duration,
    append: Duration,
    reopen: Opened,
}

/// Everything measured for one document.
pub(crate) struct Results {
    name: &'static str,
    file_bytes: u64,
    cos_open: Vec<Duration>,
    opens: Vec<Opened>,
    commits: Vec<Commit>,
}

fn commit(
    setup: &Setup,
    path: &Path,
    original: &[u8],
    label: &'static str,
    payload: usize,
) -> Result<Commit> {
    let started = Instant::now();
    let section = commit_section(original, payload)?;
    let build = started.elapsed();

    let started = Instant::now();
    let mut file = OpenOptions::new().append(true).open(path)?;
    file.write_all(&section)?;
    file.sync_all()?;
    let append = started.elapsed();
    drop(file);

    let reopened = open_to_first_page(setup, path);
    // Restore the file whatever happened, so that the next run starts from the same bytes.
    let restore = File::options().write(true).open(path)?;
    restore.set_len(original.len() as u64)?;
    Ok(Commit {
        label,
        section_bytes: section.len(),
        build,
        append,
        reopen: reopened?,
    })
}

pub(crate) fn measure(setup: &Setup, kind: Kind, path: &Path, runs: usize) -> Result<Results> {
    eprintln!("measuring {} ({runs} runs) ...", kind.name());
    let original = std::fs::read(path)?;

    let mut cos_open = Vec::new();
    for _ in 0..runs {
        let started = Instant::now();
        let store = ObjectStore::open(&original, Limits::default())?;
        cos_open.push(started.elapsed());
        drop(store);
    }

    let mut opens = Vec::new();
    for _ in 0..runs {
        opens.push(open_to_first_page(setup, path)?);
    }

    let commits = vec![
        commit(setup, path, &original, "metadata edit", 0)?,
        commit(setup, path, &original, "5 MiB insert", LARGE_COMMIT_BYTES)?,
    ];
    Ok(Results {
        name: kind.name(),
        file_bytes: original.len() as u64,
        cos_open,
        opens,
        commits,
    })
}

fn median(mut values: Vec<Duration>) -> Duration {
    values.sort();
    values.get(values.len() / 2).copied().unwrap_or_default()
}

fn ms(duration: Duration) -> String {
    format!("{:.0} ms", duration.as_secs_f64() * 1000.0)
}

#[allow(clippy::cast_precision_loss)] // a rounded display value
fn mib(bytes: u64) -> String {
    format!("{:.0} MiB", bytes as f64 / (1024.0 * 1024.0))
}

fn peak_text(peak: Option<Peak>) -> String {
    match peak {
        Some(Peak {
            bytes,
            is_peak: true,
        }) => mib(bytes),
        Some(Peak {
            bytes,
            is_peak: false,
        }) => format!("{} (current)", mib(bytes)),
        None => "n/a".into(),
    }
}

/// Prints the results as a table: medians over `runs` for the opens, one run per commit.
pub(crate) fn print_table(results: &[Results], runs: usize) {
    println!(
        "\nvellora bench (release build, warm file cache; median of {runs} runs where repeated)\n"
    );
    for r in results {
        let pages = r.opens.first().map_or(0, |o| o.pages);
        println!("{}: {} pages, {}", r.name, pages, mib(r.file_bytes));
        println!(
            "  cos open (xref only)        {}",
            ms(median(r.cos_open.clone()))
        );
        println!(
            "  engine: open                {}",
            ms(median(r.opens.iter().map(|o| o.to_open).collect()))
        );
        println!(
            "  engine: open -> first page  {}",
            ms(median(r.opens.iter().map(|o| o.to_first_page).collect()))
        );
        println!(
            "  engine: peak memory         {}",
            peak_text(
                r.opens
                    .iter()
                    .filter_map(|o| o.peak)
                    .max_by_key(|p| p.bytes)
            )
        );
        for c in &r.commits {
            println!(
                "  commit ({}, {} B section): build {}, append+fsync {}, re-open -> first page {} (total {}), peak {}",
                c.label,
                c.section_bytes,
                ms(c.build),
                ms(c.append),
                ms(c.reopen.to_first_page),
                ms(c.build + c.append + c.reopen.to_first_page),
                peak_text(c.reopen.peak),
            );
        }
        println!();
    }
}
