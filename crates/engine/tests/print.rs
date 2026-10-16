//! M1 task 16a: printing through the real engine executable. Bands are rendered the way a printer
//! wants them (annotations that print, optionally in grey), after everything the reader waits for,
//! and the document's print permission is reported.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::fmt::Write as _;
use std::fs;
use std::path::Path;

use support::{GOLDEN_PDF, Session};
use vellora_ipc::{Priority, Request, RequestId, Response, SlotId, TileRect};

fn id(n: u64) -> RequestId {
    RequestId(n)
}

/// A 100 x 100 page with a red square in the middle and three square annotations of 10 x 10: black
/// at the top left (not printable), blue at the top right (printable), green at the bottom left
/// (hidden).
fn annotated_pdf() -> Vec<u8> {
    let appearance = |colour: &str| {
        let content = format!("{colour} rg 0 0 10 10 re f");
        format!(
            "<< /Type /XObject /Subtype /Form /BBox [0 0 10 10] /Length {} >>\nstream\n{content}\nendstream",
            content.len()
        )
    };
    let annotation = |rect: &str, flags: u32, appearance: u32| {
        format!(
            "<< /Type /Annot /Subtype /Square /Rect [{rect}] /F {flags} /AP << /N {appearance} 0 R >> >>"
        )
    };
    let page_content = "1 0 0 rg 30 30 40 40 re f";
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_owned(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents 4 0 R /Annots [5 0 R 6 0 R 7 0 R] >>".to_owned(),
        format!(
            "<< /Length {} >>\nstream\n{page_content}\nendstream",
            page_content.len()
        ),
        annotation("0 90 10 100", 0, 8),
        annotation("90 90 100 100", 4, 9),
        annotation("0 0 10 10", 2, 10),
        appearance("0 0 0"),
        appearance("0 0 1"),
        appearance("0 1 0"),
    ];
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(out.len());
        writeln!(out, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
    }
    let xref = out.len();
    writeln!(out, "xref\n0 {}\n0000000000 65535 f ", objects.len() + 1).unwrap();
    for offset in offsets {
        writeln!(out, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        out,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    out.into_bytes()
}

fn opened(pdf: &[u8]) -> Session {
    let mut engine = Session::start(pdf);
    engine.handshake();
    assert!(
        matches!(engine.open(), Response::Opened { .. }),
        "the document must open"
    );
    engine
}

fn whole_page() -> TileRect {
    TileRect {
        x: 0,
        y: 0,
        width: 100,
        height: 100,
    }
}

fn print_band(n: u64, slot: u32, rect: TileRect, scale: f32, grayscale: bool) -> Request {
    Request::RenderPrint {
        req_id: id(n),
        page: 0,
        scale,
        rect,
        slot: SlotId(slot),
        grayscale,
    }
}

fn pixel(bytes: &[u8], x: usize, y: usize) -> [u8; 3] {
    let at = (y * 100 + x) * 4;
    [bytes[at], bytes[at + 1], bytes[at + 2]]
}

#[test]
fn a_print_band_leaves_out_what_does_not_print_and_a_screen_tile_keeps_it() {
    let mut engine = opened(&annotated_pdf());
    engine.send(&print_band(1, 0, whole_page(), 1.0, false));
    assert_eq!(
        engine.recv(),
        Response::TileReady {
            req_id: id(1),
            slot: SlotId(0)
        }
    );
    engine.send(&Request::RenderTile {
        req_id: id(2),
        page: 0,
        scale: 1.0,
        rect: whole_page(),
        slot: SlotId(1),
        priority: Priority::Visible,
    });
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
    let (print, screen) = (engine.slot(0), engine.slot(1));
    // The page is the same either way (blue, green, red as stored: BGR).
    assert_eq!(pixel(&print, 50, 50), [0, 0, 255]);
    assert_eq!(pixel(&screen, 50, 50), [0, 0, 255]);
    // The annotation without the print flag is on the screen and not on the paper.
    assert_eq!(pixel(&screen, 5, 5), [0, 0, 0]);
    assert_eq!(pixel(&print, 5, 5), [255, 255, 255]);
    // The one with the flag is on both; the hidden one on neither.
    assert_eq!(pixel(&print, 95, 5), [255, 0, 0]);
    assert_eq!(pixel(&screen, 95, 5), [255, 0, 0]);
    assert_eq!(pixel(&print, 5, 95), [255, 255, 255]);
    assert_eq!(pixel(&screen, 5, 95), [255, 255, 255]);
}

#[test]
fn a_grey_print_band_has_equal_channels() {
    let mut engine = opened(&annotated_pdf());
    engine.send(&print_band(1, 2, whole_page(), 1.0, true));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
    let grey = engine.slot(2);
    for (x, y) in [(50, 50), (95, 5), (20, 20)] {
        let [b, g, r] = pixel(&grey, x, y);
        assert!(b == g && g == r, "({x}, {y}): {b} {g} {r}");
    }
    assert_ne!(
        pixel(&grey, 50, 50),
        [255, 255, 255],
        "the red square has ink"
    );
}

#[test]
fn a_band_is_a_part_of_the_page_at_the_scale_asked_for() {
    let mut engine = opened(&annotated_pdf());
    // The page at 4 device pixels per point; the band is its 200 x 100 piece from (100, 100).
    let band = TileRect {
        x: 100,
        y: 100,
        width: 200,
        height: 100,
    };
    engine.send(&print_band(1, 0, band, 4.0, false));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
    let bytes = engine.slot(0);
    let at = |x: usize, y: usize| {
        let i = (y * 200 + x) * 4;
        [bytes[i], bytes[i + 1], bytes[i + 2]]
    };
    // The red square covers 120..280 on both axes at this scale, so in the band (which starts at
    // 100) it covers x 20..180 and, from y 20, the whole height.
    assert_eq!(at(10, 50), [255, 255, 255], "left of the square");
    assert_eq!(at(30, 50), [0, 0, 255], "inside it");
    assert_eq!(at(170, 50), [0, 0, 255], "inside it, near the right edge");
    assert_eq!(at(190, 50), [255, 255, 255], "right of the square");
    assert_eq!(at(30, 5), [255, 255, 255], "above it");
}

#[test]
fn a_band_that_does_not_fit_its_slot_is_refused() {
    let mut engine = opened(&annotated_pdf());
    // 1,024 x 1,024 x 4 bytes is four slots.
    let big = TileRect {
        x: 0,
        y: 0,
        width: 1024,
        height: 1024,
    };
    engine.send(&print_band(1, 0, big, 1.0, false));
    match engine.recv() {
        Response::Error { req_id, kind, .. } => {
            assert_eq!(
                (req_id, kind),
                (Some(id(1)), vellora_ipc::ErrorKind::InvalidRequest)
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
    // A page that is not there.
    engine.send(&Request::RenderPrint {
        req_id: id(2),
        page: 7,
        scale: 1.0,
        rect: whole_page(),
        slot: SlotId(0),
        grayscale: false,
    });
    assert!(matches!(
        engine.recv(),
        Response::Error {
            req_id: Some(RequestId(2)),
            ..
        }
    ));
}

#[test]
fn bands_wait_for_every_tile_the_reader_wants() {
    let mut engine = opened(&annotated_pdf());
    // Big bands keep the worker busy while the rest is queued behind the first.
    let big = TileRect {
        x: 0,
        y: 0,
        width: 512,
        height: 512,
    };
    for n in 1..=3 {
        engine.send(&print_band(n, u32::try_from(n).unwrap(), big, 16.0, false));
    }
    engine.send(&Request::RenderTile {
        req_id: id(10),
        page: 0,
        scale: 1.0,
        rect: whole_page(),
        slot: SlotId(0),
        priority: Priority::Visible,
    });
    let mut order = Vec::new();
    while order.len() < 4 {
        match engine.recv() {
            Response::TileReady { req_id, .. } => order.push(req_id.0),
            other => panic!("unexpected {other:?}"),
        }
    }
    let at = |n: u64| order.iter().position(|x| *x == n).unwrap();
    assert!(at(10) < at(2) && at(10) < at(3), "{order:?}");
}

#[test]
fn a_print_band_can_be_cancelled_while_queued() {
    let mut engine = opened(&annotated_pdf());
    let big = TileRect {
        x: 0,
        y: 0,
        width: 512,
        height: 512,
    };
    for n in 1..=3 {
        engine.send(&print_band(n, u32::try_from(n).unwrap(), big, 16.0, false));
    }
    engine.send(&Request::Cancel { req_id: id(3) });
    let mut answered = Vec::new();
    // Whatever was cancelled in time is never answered; an ordinary request after it is.
    engine.send(&Request::RenderTile {
        req_id: id(10),
        page: 0,
        scale: 1.0,
        rect: whole_page(),
        slot: SlotId(0),
        priority: Priority::Thumbnail,
    });
    // The print lane is below the thumbnail lane: request 10 comes before the queued bands 2/3.
    while !answered.contains(&10) {
        match engine.recv() {
            Response::TileReady { req_id, .. } => answered.push(req_id.0),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert!(!answered.contains(&3), "{answered:?}");
}

fn permissions(engine: &mut Session) -> (bool, bool, bool, bool) {
    engine.send(&Request::GetPermissions { req_id: id(50) });
    match engine.recv() {
        Response::Permissions {
            req_id,
            encrypted,
            owner,
            print,
            print_high_quality,
        } => {
            assert_eq!(req_id, id(50));
            (encrypted, owner, print, print_high_quality)
        }
        other => panic!("expected permissions, got {other:?}"),
    }
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../cos/tests/fixtures/encryption")
            .join(name),
    )
    .unwrap()
}

#[test]
fn a_document_that_is_not_encrypted_may_be_printed() {
    let mut engine = opened(GOLDEN_PDF);
    assert_eq!(permissions(&mut engine), (false, false, true, true));
}

#[test]
fn an_encrypted_document_that_allows_everything_may_be_printed() {
    let mut engine = opened(&fixture("r6-aes-256.pdf"));
    assert_eq!(permissions(&mut engine), (true, false, true, true));
}

#[test]
fn the_print_bits_of_the_permissions_are_reported() {
    for (name, expected) in [
        ("r6-aes-256-no-print.pdf", (true, false, false, false)),
        ("r2-rc4-40-no-print.pdf", (true, false, false, false)),
        // Printing, but not at full quality.
        ("r3-rc4-128-low-res-print.pdf", (true, false, true, false)),
    ] {
        let mut engine = opened(&fixture(name));
        assert_eq!(permissions(&mut engine), expected, "{name}");
    }
}

#[test]
fn the_owner_password_allows_printing_whatever_the_permissions_say() {
    for name in ["r6-aes-256-no-print.pdf", "r3-rc4-128-low-res-print.pdf"] {
        let mut engine = Session::start(&fixture(name));
        engine.handshake();
        assert!(matches!(
            engine.open_with_password(Some("owner-pw")),
            Response::Opened { .. }
        ));
        assert_eq!(
            permissions(&mut engine),
            (true, true, true, true),
            "{name} opened with the owner password"
        );
    }
}

#[test]
fn permissions_are_refused_before_a_document_is_open() {
    let mut engine = Session::start(&annotated_pdf());
    engine.handshake();
    engine.send(&Request::GetPermissions { req_id: id(51) });
    match engine.recv() {
        Response::Error { req_id, kind, .. } => {
            assert_eq!(
                (req_id, kind),
                (Some(id(51)), vellora_ipc::ErrorKind::InvalidRequest)
            );
        }
        other => panic!("expected a refusal, got {other:?}"),
    }
}
