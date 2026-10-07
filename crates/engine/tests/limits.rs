//! The engine under the limits the UI puts on it (M0 task 19): a memory bomb, a render that
//! outlives its hard deadline, and an orderly end for comparison. Each test starts the real
//! executable through `vellora_engine_client::EngineProcess`, the code the UI uses.
//!
//! They need the PDFium build from `cargo xtask pdfium fetch` and fail, not skip, without it.

// Test code: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::io::{BufReader, BufWriter, Write};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::{Duration, Instant};

use support::{GOLDEN_PDF, pdfium_path};
use vellora_engine::Deadlines;
use vellora_engine_client::{Crash, EngineProcess, ResourceLimits, SpawnConfig, Termination};
use vellora_ipc::{
    PROTOCOL_VERSION, Priority, Request, RequestId, Response, SlotId, TileRect, read_frame,
    write_frame,
};
use vellora_shm::{SlotGeometry, TileRegion};

/// How long a test waits for an answer from an engine before calling it hung.
const PATIENCE: Duration = Duration::from_secs(120);

/// Content that decodes to about 900 MiB of white space from a 6 MB file. PDFium decodes a
/// content stream whole (it refuses beyond 1 GiB), so rendering this page needs the better part of
/// a gigabyte, and a good while to scan it.
const BOMB_MATCHES: u64 = 3_657_000;

/// A one-page PDF whose only content stream is a deflate bomb of zero bytes: one literal zero,
/// then `BOMB_MATCHES` copies of "258 bytes, distance 1".
fn bomb_pdf() -> Vec<u8> {
    let stream = bomb_stream();
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    let mut object = |pdf: &mut Vec<u8>, body: &[u8]| {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n", offsets.len()).as_bytes());
        pdf.extend_from_slice(body);
        pdf.extend_from_slice(b"\nendobj\n");
    };
    object(&mut pdf, b"<< /Type /Catalog /Pages 2 0 R >>");
    object(&mut pdf, b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>");
    object(
        &mut pdf,
        b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 200] /Contents 4 0 R >>",
    );
    let mut body = format!(
        "<< /Filter /FlateDecode /Length {} >>\nstream\n",
        stream.len()
    )
    .into_bytes();
    body.extend_from_slice(&stream);
    body.extend_from_slice(b"\nendstream");
    object(&mut pdf, &body);

    let xref = pdf.len();
    pdf.extend_from_slice(b"xref\n0 5\n0000000000 65535 f \n");
    for offset in &offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!("trailer\n<< /Size 5 /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    pdf
}

/// The zlib stream of the bomb, written by hand: a single final block with the fixed Huffman code
/// (RFC 1951 section 3.2.6), so that no compressor is needed and the result is exact.
fn bomb_stream() -> Vec<u8> {
    struct Bits {
        bytes: Vec<u8>,
        used: u32,
    }
    impl Bits {
        /// Appends the low `count` bits of `value`, lowest first (header fields).
        fn low_first(&mut self, value: u32, count: u32) {
            for bit in 0..count {
                self.push((value >> bit) & 1 == 1);
            }
        }
        /// Appends a Huffman code, whose bits go highest first.
        fn high_first(&mut self, code: u32, count: u32) {
            for bit in (0..count).rev() {
                self.push((code >> bit) & 1 == 1);
            }
        }
        fn push(&mut self, bit: bool) {
            if self.used == 0 {
                self.bytes.push(0);
            }
            if let (true, Some(last)) = (bit, self.bytes.last_mut()) {
                *last |= 1 << self.used;
            }
            self.used = (self.used + 1) % 8;
        }
    }

    let mut bits = Bits {
        bytes: vec![0x78, 0x01],
        used: 0,
    };
    bits.low_first(1, 1); // BFINAL
    bits.low_first(1, 2); // BTYPE = 01, fixed Huffman
    bits.high_first(0x30, 8); // literal 0
    for _ in 0..BOMB_MATCHES {
        bits.high_first(0xC5, 8); // length symbol 285: 258 bytes, no extra bits
        bits.high_first(0, 5); // distance symbol 0: 1 byte back
    }
    bits.high_first(0, 7); // end of block (symbol 256)

    // Adler-32 of N zero bytes: `a` stays 1 and `b` adds 1 per byte.
    let length = 1 + 258 * BOMB_MATCHES;
    #[allow(clippy::cast_possible_truncation)]
    let adler = (((length % 65_521) as u32) << 16) | 1;
    bits.bytes.extend_from_slice(&adler.to_be_bytes());
    bits.bytes
}

/// An engine started the way the UI starts it, and the UI's end of the conversation.
struct Client {
    process: EngineProcess,
    input: Option<BufWriter<std::process::ChildStdin>>,
    frames: Receiver<Option<Response>>,
    _region: TileRegion,
}

impl Client {
    fn start(pdf: &[u8], deadlines: Deadlines, limits: impl FnOnce(u64) -> ResourceLimits) -> Self {
        let geometry = SlotGeometry::new(4, 1 << 20).unwrap();
        let mut document = tempfile::tempfile().unwrap();
        document.write_all(pdf).unwrap();
        let (region, region_file) = TileRegion::create(geometry).unwrap();

        let engine = std::path::Path::new(env!("CARGO_BIN_EXE_vellora-engine"));
        let mut config = SpawnConfig::new(engine, &document, &region_file, geometry);
        config.deadlines = deadlines;
        let mapped = pdf.len() as u64 + u64::from(geometry.slot_count()) * (1 << 20);
        config.limits = limits(mapped);
        config.env.push((
            vellora_render::LIBRARY_ENV.into(),
            pdfium_path().into_os_string(),
        ));
        let mut process = EngineProcess::spawn(&config).unwrap();

        let input = BufWriter::new(process.take_stdin().unwrap());
        let mut output = BufReader::new(process.take_stdout().unwrap());
        let (sender, frames) = mpsc::channel();
        // A reader thread, so that a hung engine fails the test instead of blocking it forever.
        thread::spawn(move || {
            loop {
                let frame = read_frame::<_, Response>(&mut output).ok().flatten();
                let done = frame.is_none();
                if sender.send(frame).is_err() || done {
                    break;
                }
            }
        });
        Self {
            process,
            input: Some(input),
            frames,
            _region: region,
        }
    }

    fn send(&mut self, request: &Request) {
        write_frame(self.input.as_mut().unwrap(), request).unwrap();
    }

    /// The next response; `None` when the engine closed its output.
    fn recv(&mut self) -> Option<Response> {
        self.frames
            .recv_timeout(PATIENCE)
            .expect("the engine neither answered nor closed its output")
    }

    /// Handshake and `Open`; the document, which has `pages` pages, must open.
    fn open(&mut self, pages: u32) {
        assert!(matches!(self.recv(), Some(Response::Hello { .. })));
        self.send(&Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        });
        self.send(&Request::Open {
            handle_token: self.process.document_token().get(),
        });
        assert!(
            matches!(self.recv(), Some(Response::Opened { page_count, .. }) if page_count == pages),
            "the document must open"
        );
    }

    fn render_first_page(&mut self) {
        self.send(&Request::RenderTile {
            priority: Priority::Visible,
            req_id: RequestId(1),
            page: 0,
            scale: 1.0,
            rect: TileRect {
                x: 0,
                y: 0,
                width: 100,
                height: 100,
            },
            slot: SlotId(0),
        });
    }
}

#[cfg(not(target_os = "macos"))]
/// The memory cap of the bomb tests: what is mapped plus half a gigabyte, well under what the
/// bomb needs and well over what a healthy engine uses.
fn tight(mapped: u64) -> ResourceLimits {
    ResourceLimits {
        memory_bytes: Some(mapped + (512 << 20)),
        cpu_seconds: None,
    }
}

/// Deadlines that never get in the way of a slow render.
fn patient() -> Deadlines {
    Deadlines {
        soft: Duration::from_secs(60),
        hard: Duration::from_secs(120),
    }
}

/// An orderly end: `Close` makes the engine leave with success, which `EngineProcess` reports as
/// such, and nothing about it looks like a crash.
#[test]
fn a_closed_session_ends_with_success() {
    let mut engine = Client::start(GOLDEN_PDF, Deadlines::default(), ResourceLimits::for_mapped);
    engine.open(3);
    engine.send(&Request::Close);
    assert_eq!(
        engine
            .process
            .wait_timeout(Duration::from_secs(30))
            .unwrap(),
        Some(Termination::Success)
    );
}

/// The control for the memory test: without a limit the same bomb renders. If this fails, the
/// bomb is not what the next test says it is.
#[test]
fn the_bomb_renders_when_nothing_limits_it() {
    let mut engine = Client::start(&bomb_pdf(), patient(), |_| ResourceLimits::default());
    engine.open(1);
    engine.render_first_page();
    let started = Instant::now();
    assert_eq!(
        engine.recv(),
        Some(Response::TileReady {
            req_id: RequestId(1),
            slot: SlotId(0)
        })
    );
    eprintln!("the bomb page rendered in {:?}", started.elapsed());
}

/// (a) A document that needs far more memory than the engine may have: the engine dies and the
/// client sees its pipe close and reports a typed crash, never a render or an error answer.
///
/// macOS does not enforce a memory limit (see `vellora_engine_client::limits`), so there is
/// nothing to test there.
#[cfg(not(target_os = "macos"))]
#[test]
fn a_memory_bomb_kills_the_engine_and_the_client_reports_a_crash() {
    const { assert!(ResourceLimits::ENFORCES_MEMORY) };
    let mut engine = Client::start(&bomb_pdf(), patient(), tight);
    engine.open(1);
    engine.render_first_page();

    // The only acceptable next event is the pipe closing.
    assert_eq!(engine.recv(), None, "the engine must not survive the bomb");
    let crash = engine.process.crash(Duration::from_secs(30));
    eprintln!("memory bomb: {crash:?}");
    assert!(!crash.killed_by_client, "{crash:?}");
    assert_ne!(crash.termination, Termination::Success, "{crash:?}");
    // PDFium's allocator ends the process with its own out-of-memory exception code.
    #[cfg(windows)]
    assert_eq!(
        crash.termination,
        Termination::Exception(0xE000_0008),
        "{crash:?}"
    );
}

/// (b) A render that outlives its hard deadline: the engine aborts itself within the deadline (the
/// render alone takes much longer, see the control above), the client sees the pipe close and
/// reports an abort.
#[test]
fn a_render_past_its_hard_deadline_ends_the_engine() {
    let deadlines = Deadlines {
        soft: Duration::from_millis(100),
        hard: Duration::from_millis(300),
    };
    let mut engine = Client::start(&bomb_pdf(), deadlines, |_| ResourceLimits::default());
    engine.open(1);
    let started = Instant::now();
    engine.render_first_page();

    assert_eq!(
        engine.recv(),
        None,
        "a tile past its deadline must not be answered"
    );
    let elapsed = started.elapsed();
    let crash: Crash = engine.process.crash(Duration::from_secs(30));
    eprintln!("hard deadline: ended after {elapsed:?}, {crash:?}");

    assert!(
        elapsed >= deadlines.hard,
        "ended before the deadline: {elapsed:?}"
    );
    assert!(
        elapsed < deadlines.hard + Duration::from_secs(5),
        "ended long after the deadline: {elapsed:?}"
    );
    assert!(!crash.killed_by_client, "{crash:?}");
    // What `std::process::abort` looks like to the parent.
    #[cfg(unix)]
    assert_eq!(crash.termination, Termination::Signal(6), "{crash:?}");
    #[cfg(windows)]
    assert_eq!(
        crash.termination,
        Termination::Exception(0xC000_0409),
        "{crash:?}"
    );
}
