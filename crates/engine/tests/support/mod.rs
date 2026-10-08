//! Test harness: runs the real `vellora-engine` executable the way the UI will, with inherited
//! handles for the document and the tile region and the protocol on its standard streams.

// Each test file uses a different part, and the items are public only to the files that include this.
#![allow(dead_code, unreachable_pub)]
// Test helper: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::match_wild_err_arm,
    clippy::cast_precision_loss,
    clippy::doc_markdown
)]

use std::env;
use std::fs::File;
use std::io::{BufReader, BufWriter, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, ExitStatus, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use vellora_engine::LaunchArgs;
use vellora_ipc::{PROTOCOL_VERSION, Password, Request, Response, read_frame, write_frame};
use vellora_shm::{HandleToken, SlotGeometry, TileRegion, share_with_child, stop_sharing};

/// How long a test waits for the engine before calling it hung.
const PATIENCE: Duration = Duration::from_secs(60);

/// The golden fixture: three generated pages, see `crates/render/src/test_support.rs`.
pub const GOLDEN_PDF: &[u8] = include_bytes!("../fixtures/golden.pdf");

/// Where `cargo xtask pdfium fetch` puts the library for this machine, or `$VELLORA_PDFIUM_LIB`.
pub fn pdfium_path() -> PathBuf {
    if let Some(path) = env::var_os(vellora_render::LIBRARY_ENV) {
        return PathBuf::from(path);
    }
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
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/pdfium")
        .join(platform)
        .join(library);
    // Fail, never skip: a green run must mean the engine really rendered with PDFium.
    assert!(
        path.is_file(),
        "PDFium not found at {}: run `cargo xtask pdfium fetch` (or set {})",
        path.display(),
        vellora_render::LIBRARY_ENV
    );
    path
}

/// The engine executable under test.
pub fn engine_command() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_vellora-engine"));
    command.env(vellora_render::LIBRARY_ENV, pdfium_path());
    command
}

/// A running engine and the UI's side of the conversation.
pub struct Session {
    child: Child,
    input: Option<BufWriter<ChildStdin>>,
    frames: Receiver<Result<Option<Response>, vellora_ipc::Error>>,
    /// The UI's mapping of the tile region.
    pub region: TileRegion,
    /// The number the engine knows the document by (what `Open` must carry).
    pub file_token: HandleToken,
}

impl Session {
    /// Starts an engine over `pdf` with 4 slots of 1 MiB.
    pub fn start(pdf: &[u8]) -> Self {
        Self::start_with(pdf, SlotGeometry::new(4, 1 << 20).unwrap(), None)
    }

    /// Starts an engine with an explicit tile region and, optionally, a document size limit.
    pub fn start_with(pdf: &[u8], geometry: SlotGeometry, max_document_bytes: Option<u64>) -> Self {
        Self::start_logged(pdf, geometry, max_document_bytes, Stdio::inherit(), None)
    }

    /// Starts an engine whose standard error goes to `stderr` and, if `log_level` is given, which
    /// logs at that level (`VELLORA_LOG`).
    pub fn start_logged(
        pdf: &[u8],
        geometry: SlotGeometry,
        max_document_bytes: Option<u64>,
        stderr: Stdio,
        log_level: Option<&str>,
    ) -> Self {
        let mut document = tempfile::tempfile().unwrap();
        document.write_all(pdf).unwrap();
        let (region, region_file) = TileRegion::create(geometry).unwrap();

        let file_token = share_with_child(&document).unwrap();
        let region_token = share_with_child(&region_file).unwrap();
        let launch = LaunchArgs {
            file: file_token,
            region: region_token,
            geometry,
            max_document_bytes: max_document_bytes
                .unwrap_or(vellora_engine::DEFAULT_MAX_DOCUMENT_BYTES),
            deadlines: vellora_engine::Deadlines::default(),
        };
        let mut command = engine_command();
        if let Some(level) = log_level {
            command.env("VELLORA_LOG", level);
        }
        let child = command
            .args(launch.to_args())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(stderr)
            .spawn()
            .unwrap();
        // Later children must not inherit the files.
        stop_sharing(&document).unwrap();
        stop_sharing(&region_file).unwrap();
        // The engine now owns its copies; the originals may close.
        drop((document, region_file));

        Self::attach(child, region, file_token)
    }

    fn attach(mut child: Child, region: TileRegion, file_token: HandleToken) -> Self {
        let input = BufWriter::new(child.stdin.take().unwrap());
        let mut output = BufReader::new(child.stdout.take().unwrap());
        let (sender, frames) = mpsc::channel();
        // A reader thread, so that a hung engine fails the test instead of blocking it forever.
        thread::spawn(move || {
            loop {
                let frame = read_frame::<_, Response>(&mut output);
                let done = !matches!(frame, Ok(Some(_)));
                if sender.send(frame).is_err() || done {
                    break;
                }
            }
        });
        Self {
            child,
            input: Some(input),
            frames,
            region,
            file_token,
        }
    }

    /// Sends one request.
    pub fn send(&mut self, request: &Request) {
        let input = self.input.as_mut().expect("input already closed");
        write_frame(input, request).unwrap();
    }

    /// Writes raw bytes to the engine's input, for malformed-frame tests.
    pub fn send_raw(&mut self, bytes: &[u8]) {
        let input = self.input.as_mut().expect("input already closed");
        input.write_all(bytes).unwrap();
        input.flush().unwrap();
    }

    /// Closes the engine's input, as a UI that exits does.
    pub fn close_input(&mut self) {
        self.input = None;
    }

    /// The next response; `None` when the engine closed its output.
    pub fn try_recv(&mut self) -> Option<Response> {
        match self.frames.recv_timeout(PATIENCE) {
            Ok(Ok(frame)) => frame,
            Ok(Err(error)) => panic!("unreadable response: {error}"),
            Err(_) => panic!("the engine did not answer within {PATIENCE:?}"),
        }
    }

    /// The next response, which must exist.
    pub fn recv(&mut self) -> Response {
        self.try_recv().expect("the engine closed its output")
    }

    /// Exchanges `Hello`s.
    pub fn handshake(&mut self) {
        assert_eq!(
            self.recv(),
            Response::Hello {
                protocol_version: PROTOCOL_VERSION
            }
        );
        self.send(&Request::Hello {
            protocol_version: PROTOCOL_VERSION,
        });
    }

    /// `Open` with the right token; returns the engine's answer.
    pub fn open(&mut self) -> Response {
        self.open_with_password(None)
    }

    /// `Open` with the right token and a password; returns the engine's answer.
    pub fn open_with_password(&mut self, password: Option<&str>) -> Response {
        self.send(&Request::Open {
            handle_token: self.file_token.get(),
            password: password.map(Password::new),
        });
        self.recv()
    }

    /// One slot's bytes.
    pub fn slot(&self, slot: u32) -> Vec<u8> {
        let mut bytes = vec![0; self.region.geometry().slot_bytes() as usize];
        self.region.read_slot(slot, &mut bytes).unwrap();
        bytes
    }

    /// Waits for the engine to exit.
    pub fn wait(&mut self) -> ExitStatus {
        self.close_input();
        // `Child::wait` has no timeout; poll so that a hung engine fails the test.
        let deadline = std::time::Instant::now() + PATIENCE;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "the engine did not exit"
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Pixels of a PNG written by the render tests: width, height, RGB bytes.
pub fn read_png(path: &PathBuf) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(BufReader::new(File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut data = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut data).unwrap();
    assert_eq!(
        (info.color_type, info.bit_depth),
        (png::ColorType::Rgb, png::BitDepth::Eight),
        "{}",
        path.display()
    );
    data.truncate(info.buffer_size());
    (info.width, info.height, data)
}

/// BGRx (what the engine writes) to RGB.
pub fn bgrx_to_rgb(bgrx: &[u8]) -> Vec<u8> {
    bgrx.as_chunks::<4>()
        .0
        .iter()
        .flat_map(|p| [p[2], p[1], p[0]])
        .collect()
}

/// How far `actual` is from `golden`, both RGB: the mean absolute channel difference, and the
/// share of pixels where any channel differs by more than 32 levels. Same measure and limits as
/// the render crate's golden test.
pub fn difference(actual: &[u8], golden: &[u8]) -> (f64, f64) {
    const LOUD: u8 = 32;
    assert_eq!(actual.len(), golden.len());
    let total: u64 = actual
        .iter()
        .zip(golden)
        .map(|(&a, &g)| u64::from(a.abs_diff(g)))
        .sum();
    let loud = actual
        .as_chunks::<3>()
        .0
        .iter()
        .zip(golden.as_chunks::<3>().0)
        .filter(|(a, g)| a.iter().zip(g.iter()).any(|(&a, &g)| a.abs_diff(g) > LOUD))
        .count();
    (
        total as f64 / actual.len() as f64,
        loud as f64 / (actual.len() / 3) as f64,
    )
}

/// The committed golden image of page `page` (zero-based) of [`GOLDEN_PDF`].
pub fn golden_image(page: usize) -> (u32, u32, Vec<u8>) {
    read_png(
        &PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../render/testdata/golden")
            .join(format!("page-{}.png", page + 1)),
    )
}
