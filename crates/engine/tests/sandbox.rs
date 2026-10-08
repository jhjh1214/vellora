//! The engine behaves the same inside the OS sandbox as outside it (M1 task 4, ADR-0017).
//!
//! The UI's `Client` starts the engine in its sandbox (an `AppContainer` on Windows); the test
//! `Session` starts the same executable bare. A page of text in a font that is not embedded, so
//! PDFium has to find it among the system fonts, must come out pixel for pixel the same in both.
//! A control shows the test is sensitive: with a font that exists nowhere, the page looks different.
//!
//! Needs the PDFium build from `cargo xtask pdfium fetch`; fails, never skips, without it.

// Test code: a failure should panic with its message (clippy only exempts `#[test]` functions).
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fs;
use std::time::{Duration, Instant};

use support::{Session, pdfium_path};
use vellora_engine_client::{Client, ClientConfig, Event, PDFIUM_ENV, TileRequest};
use vellora_ipc::{Priority, Request, RequestId, Response, SlotId, TileRect};
use vellora_shm::SlotGeometry;

const WIDTH: u32 = 240;
const HEIGHT: u32 = 80;

/// How long a test waits for the engine before calling it hung.
const PATIENCE: Duration = Duration::from_secs(60);

/// The client grants the sandbox read access to the file this variable names, so it must be the
/// variable the engine reads.
#[test]
fn the_client_and_the_renderer_agree_on_where_pdfium_is() {
    assert_eq!(PDFIUM_ENV, vellora_render::LIBRARY_ENV);
    if cfg!(windows) {
        assert_eq!(
            vellora_engine_client::PDFIUM_LIBRARY_WINDOWS,
            vellora_render::library_file_name()
        );
    }
}

/// A one-page PDF with a line of text set in the non-embedded TrueType font `font`.
fn text_pdf(font: &str) -> Vec<u8> {
    let content = "BT /F1 28 Tf 10 30 Td (Sandbox 0123 Wxyz) Tj ET";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {WIDTH} {HEIGHT}] /Contents 4 0 R \
             /Resources << /Font << /F1 5 0 R >> >> >>"
        ),
        format!(
            "<< /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        ),
        format!(
            "<< /Type /Font /Subtype /TrueType /BaseFont /{font} /Encoding /WinAnsiEncoding >>"
        ),
    ];
    let mut pdf = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, body) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        pdf.extend_from_slice(format!("{} 0 obj\n{body}\nendobj\n", index + 1).as_bytes());
    }
    let xref = pdf.len();
    pdf.extend_from_slice(
        format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
    );
    for offset in offsets {
        pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    pdf.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    pdf
}

/// The page rendered by the bare engine.
fn render_bare(pdf: &[u8]) -> Vec<u8> {
    let mut engine = Session::start(pdf);
    engine.handshake();
    assert!(matches!(engine.open(), Response::Opened { .. }));
    engine.send(&Request::RenderTile {
        priority: Priority::Visible,
        req_id: RequestId(1),
        page: 0,
        scale: 1.0,
        rect: TileRect {
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT,
        },
        slot: SlotId(0),
    });
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
    let mut pixels = engine.slot(0);
    pixels.truncate(WIDTH as usize * HEIGHT as usize * 4);
    pixels
}

/// The page rendered by the engine as the UI starts it.
fn render_sandboxed(pdf: &[u8]) -> Vec<u8> {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("text.pdf");
    fs::write(&path, pdf).unwrap();
    let mut config = ClientConfig::new(
        env!("CARGO_BIN_EXE_vellora-engine"),
        SlotGeometry::new(4, 1 << 20).unwrap(),
    );
    config
        .env
        .push((vellora_render::LIBRARY_ENV.into(), pdfium_path().into()));
    config.max_restarts = 0;
    let client = Client::open(config, &path).unwrap();
    let deadline = Instant::now() + PATIENCE;
    let mut requested = None;
    loop {
        let left = deadline
            .checked_duration_since(Instant::now())
            .expect("no tile within the patience");
        for event in client.wait_events(left) {
            match event {
                Event::Opened { .. } => {
                    requested = Some(
                        client
                            .request_tile(&TileRequest {
                                page: 0,
                                scale: 1.0,
                                rect: TileRect {
                                    x: 0,
                                    y: 0,
                                    width: WIDTH,
                                    height: HEIGHT,
                                },
                                slot: SlotId(0),
                                priority: Priority::Visible,
                            })
                            .unwrap(),
                    );
                }
                Event::TileReady { request, .. } if Some(request) == requested => {
                    let mut pixels = vec![0; client.geometry().slot_bytes() as usize];
                    client.read_slot(SlotId(0), &mut pixels).unwrap();
                    pixels.truncate(WIDTH as usize * HEIGHT as usize * 4);
                    client.close();
                    return pixels;
                }
                Event::EngineCrashed { .. }
                | Event::Failed { .. }
                | Event::RequestFailed { .. }
                | Event::EngineTimeout { .. } => panic!("the sandboxed engine failed: {event:?}"),
                _ => {}
            }
        }
    }
}

/// Counts the pixels that are not white, so a blank page cannot pass for a match.
fn inked(pixels: &[u8]) -> usize {
    pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|pixel| pixel[..3].iter().any(|&channel| channel < 128))
        .count()
}

#[test]
fn text_in_a_system_font_renders_the_same_inside_the_sandbox() {
    // Segoe UI ships with every Windows; elsewhere PDFium substitutes a font on both sides.
    let pdf = text_pdf("SegoeUI");
    let bare = render_bare(&pdf);
    let sandboxed = render_sandboxed(&pdf);
    assert!(inked(&bare) > 200, "the page must show text");
    assert!(
        bare == sandboxed,
        "the page differs inside the sandbox: {} of {} bytes",
        bare.iter().zip(&sandboxed).filter(|(a, b)| a != b).count(),
        bare.len()
    );

    // Control: a font found nowhere gives different pixels on Windows, so the equality above
    // means the sandboxed engine found the same system font.
    if cfg!(windows) {
        let substitute = render_bare(&text_pdf("NoSuchFontAnywhereAtAll"));
        assert_ne!(
            substitute, bare,
            "the font was not picked up from the system"
        );
    }
}
