//! End-to-end tests: the real engine executable, started with inherited handles, driven over its
//! standard streams with the real protocol.
//!
//! They need the PDFium build from `cargo xtask pdfium fetch` and fail, not skip, without it.

mod support;

use std::process::Stdio;

use support::{GOLDEN_PDF, Session, bgrx_to_rgb, difference, engine_command, golden_image};
use vellora_ipc::{
    ErrorKind, PROTOCOL_VERSION, PageSize, Request, RequestId, Response, SlotId, TileRect,
};
use vellora_shm::SlotGeometry;

/// Same tolerance as the render crate's golden test: an average error under half a grey level,
/// and under 0.5% of pixels off by more than 32 levels.
const MEAN_LIMIT: f64 = 0.5;
const LOUD_LIMIT: f64 = 0.005;

fn tile(req_id: u64, page: u32, scale: f32, rect: (u32, u32, u32, u32), slot: u32) -> Request {
    Request::RenderTile {
        req_id: RequestId(req_id),
        page,
        scale,
        rect: TileRect {
            x: rect.0,
            y: rect.1,
            width: rect.2,
            height: rect.3,
        },
        slot: SlotId(slot),
    }
}

#[allow(clippy::panic)] // a test helper: the panic message is the failure report
fn expect_error(response: Response) -> (Option<RequestId>, ErrorKind, String) {
    match response {
        Response::Error {
            req_id,
            kind,
            message,
        } => (req_id, kind, message),
        other => panic!("expected an error, got {other:?}"),
    }
}

/// An opened session over the golden document.
fn opened() -> Session {
    let mut engine = Session::start(GOLDEN_PDF);
    engine.handshake();
    assert!(matches!(engine.open(), Response::Opened { .. }));
    engine
}

#[test]
fn opens_a_document_and_renders_tiles_that_match_the_golden_images() {
    let mut engine = Session::start(GOLDEN_PDF);
    engine.handshake();

    let Response::Opened {
        page_count,
        page_sizes,
        repaired,
    } = engine.open()
    else {
        panic!("not opened");
    };
    assert_eq!(page_count, 3);
    assert!(!repaired, "a clean file must not be reported as repaired");
    // Page 2 is 160 x 200 points turned by 90 degrees.
    assert_eq!(
        page_sizes,
        [
            PageSize {
                width: 220.0,
                height: 140.0
            },
            PageSize {
                width: 200.0,
                height: 160.0
            },
            PageSize {
                width: 180.0,
                height: 120.0
            },
        ]
    );

    for (index, size) in page_sizes.iter().enumerate() {
        let scale = 1.5_f32;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let (width, height) = (
            (size.width * scale).ceil() as u32,
            (size.height * scale).ceil() as u32,
        );
        let slot = u32::try_from(index).unwrap();
        engine.send(&tile(
            100 + u64::from(slot),
            slot,
            scale,
            (0, 0, width, height),
            slot,
        ));
        assert_eq!(
            engine.recv(),
            Response::TileReady {
                req_id: RequestId(100 + u64::from(slot)),
                slot: SlotId(slot)
            }
        );

        let pixels = engine.slot(slot);
        let used = width as usize * height as usize * 4;
        let actual = bgrx_to_rgb(&pixels[..used]);
        let (golden_width, golden_height, golden) = golden_image(index);
        assert_eq!(
            (golden_width, golden_height),
            (width, height),
            "page {index}"
        );
        let (mean, loud) = difference(&actual, &golden);
        assert!(
            mean <= MEAN_LIMIT && loud <= LOUD_LIMIT,
            "page {index}: mean difference {mean:.3} (limit {MEAN_LIMIT}), {:.3}% of pixels differ \
             loudly (limit {}%)",
            loud * 100.0,
            LOUD_LIMIT * 100.0
        );
    }

    engine.send(&Request::Close);
    assert!(engine.wait().success());
}

#[test]
fn a_version_mismatch_is_reported_and_ends_the_session() {
    let mut engine = Session::start(GOLDEN_PDF);
    assert_eq!(
        engine.recv(),
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    engine.send(&Request::Hello {
        protocol_version: PROTOCOL_VERSION + 1,
    });
    let (_, kind, message) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::VersionMismatch);
    assert!(message.contains("version"), "{message}");
    assert!(engine.try_recv().is_none());
    assert!(!engine.wait().success());
}

#[test]
fn the_first_message_must_be_hello() {
    let mut engine = Session::start(GOLDEN_PDF);
    engine.recv();
    engine.send(&Request::Close);
    let (_, kind, _) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::InvalidRequest);
    assert!(!engine.wait().success());
}

#[test]
fn requests_that_do_not_make_sense_are_refused_and_the_session_survives() {
    let mut engine = Session::start(GOLDEN_PDF);
    engine.handshake();

    // A tile before anything is open.
    engine.send(&tile(1, 0, 1.0, (0, 0, 10, 10), 0));
    let (req_id, kind, message) = expect_error(engine.recv());
    assert_eq!(
        (req_id, kind),
        (Some(RequestId(1)), ErrorKind::InvalidRequest)
    );
    assert!(message.contains("no document"), "{message}");

    // The wrong handle token.
    engine.send(&Request::Open {
        handle_token: engine.file_token.get() + 1,
    });
    let (_, kind, _) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::InvalidRequest);

    // A second Hello.
    engine.send(&Request::Hello {
        protocol_version: PROTOCOL_VERSION,
    });
    let (_, kind, _) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::InvalidRequest);

    assert!(matches!(engine.open(), Response::Opened { .. }));

    // Opening twice.
    engine.send(&Request::Open {
        handle_token: engine.file_token.get(),
    });
    let (_, kind, message) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::InvalidRequest);
    assert!(message.contains("already open"), "{message}");

    // Every request that points outside what exists.
    let bad = [
        (tile(2, 3, 1.0, (0, 0, 10, 10), 0), "out of range"),
        (tile(3, 0, 1.0, (0, 0, 10, 10), 4), "slot 4 is out of range"),
        // 1024 x 1024 x 4 bytes is 4 MiB, the slots hold 1 MiB.
        (tile(4, 0, 1.0, (0, 0, 1024, 1024), 0), "does not fit"),
    ];
    for (request, expected) in bad {
        engine.send(&request);
        let (_, kind, message) = expect_error(engine.recv());
        assert_eq!(kind, ErrorKind::InvalidRequest, "{expected}");
        assert!(
            message.contains(expected),
            "{expected:?} not in {message:?}"
        );
    }

    // The protocol's own validation refuses these before the engine's logic sees them. The UI's
    // writer would not send them, so they are framed by hand (postcard: variant 2 is RenderTile).
    let frame = |payload: &[u8]| {
        let mut bytes = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        bytes.extend_from_slice(payload);
        bytes
    };
    let one = 1.0_f32.to_le_bytes();
    let zero_scale = [&[2, 5, 0][..], &0.0_f32.to_le_bytes(), &[0, 0, 10, 10, 0]].concat();
    let zero_width = [&[2, 6, 0][..], &one, &[0, 0, 0, 10, 0]].concat();
    for (payload, expected) in [(zero_scale, "scale"), (zero_width, "sides")] {
        engine.send_raw(&frame(&payload));
        let (_, kind, message) = expect_error(engine.recv());
        assert_eq!(kind, ErrorKind::InvalidRequest);
        assert!(
            message.contains(expected),
            "{expected:?} not in {message:?}"
        );
    }

    // After all that, the engine still renders.
    engine.send(&tile(7, 0, 1.0, (0, 0, 20, 20), 1));
    assert_eq!(
        engine.recv(),
        Response::TileReady {
            req_id: RequestId(7),
            slot: SlotId(1)
        }
    );
}

#[test]
fn a_slot_is_only_written_for_the_slot_requested() {
    let mut engine = opened();
    engine.send(&tile(1, 0, 1.0, (0, 0, 10, 10), 2));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
    assert!(engine.slot(2).iter().any(|&byte| byte != 0));
    for other in [0, 1, 3] {
        assert!(
            engine.slot(other).iter().all(|&byte| byte == 0),
            "slot {other}"
        );
    }
}

#[test]
fn a_file_that_is_not_a_pdf_fails_to_open_and_the_session_survives() {
    let mut engine = Session::start(b"this is not a PDF");
    engine.handshake();
    let (req_id, kind, _) = expect_error(engine.open());
    assert_eq!((req_id, kind), (None, ErrorKind::OpenFailed));
    // The file is still there: asking again gives the same typed answer, not a crash.
    let (_, kind, _) = expect_error(engine.open());
    assert_eq!(kind, ErrorKind::OpenFailed);
    engine.send(&Request::Close);
    assert!(engine.wait().success());
}

#[test]
fn an_empty_file_fails_to_open() {
    let mut engine = Session::start(b"");
    engine.handshake();
    let (_, kind, _) = expect_error(engine.open());
    assert_eq!(kind, ErrorKind::OpenFailed);
}

#[test]
fn a_document_over_the_size_limit_is_refused_before_it_is_mapped() {
    let mut engine =
        Session::start_with(GOLDEN_PDF, SlotGeometry::new(1, 4096).unwrap(), Some(100));
    engine.handshake();
    let (_, kind, message) = expect_error(engine.open());
    assert_eq!(kind, ErrorKind::OpenFailed);
    assert!(message.contains("limit"), "{message}");
}

#[test]
fn a_damaged_cross_reference_opens_and_is_reported_as_repaired() {
    let mut pdf = GOLDEN_PDF.to_vec();
    // Point `startxref` at nothing; both PDFium and `cos` then rebuild the table by scanning.
    let marker = b"startxref\n";
    let at = pdf
        .windows(marker.len())
        .rposition(|window| window == marker)
        .unwrap()
        + marker.len();
    let end = at + pdf[at..].iter().position(|&b| b == b'\n').unwrap();
    pdf.splice(at..end, *b"99999");

    let mut engine = Session::start(&pdf);
    engine.handshake();
    let Response::Opened {
        page_count,
        repaired,
        ..
    } = engine.open()
    else {
        panic!("a recoverable file must open");
    };
    assert_eq!(page_count, 3);
    assert!(repaired, "a rebuilt cross-reference must be reported");

    // And it still renders.
    engine.send(&tile(1, 1, 1.0, (0, 0, 50, 50), 0));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
}

#[test]
fn cancel_is_accepted_without_an_answer() {
    let mut engine = opened();
    engine.send(&Request::Cancel {
        req_id: RequestId(99),
    });
    // The next answer belongs to the next request, not to the cancel.
    engine.send(&tile(1, 0, 1.0, (0, 0, 10, 10), 0));
    assert_eq!(
        engine.recv(),
        Response::TileReady {
            req_id: RequestId(1),
            slot: SlotId(0)
        }
    );
}

#[test]
fn a_malformed_frame_is_answered_and_the_session_goes_on() {
    let mut engine = opened();
    // A well-framed payload that is not a message: length 3, then three bytes of junk.
    engine.send_raw(&[3, 0, 0, 0, 0xFF, 0xFF, 0xFF]);
    let (_, kind, _) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::InvalidRequest);
    engine.send(&tile(1, 0, 1.0, (0, 0, 10, 10), 0));
    assert!(matches!(engine.recv(), Response::TileReady { .. }));
}

#[test]
fn an_oversized_frame_ends_the_session_without_being_read() {
    let mut engine = opened();
    // Announce 4 GiB - 1 and send nothing: the engine must refuse on the prefix alone.
    engine.send_raw(&u32::MAX.to_le_bytes());
    let (_, kind, _) = expect_error(engine.recv());
    assert_eq!(kind, ErrorKind::InvalidRequest);
    assert!(engine.try_recv().is_none());
    assert!(!engine.wait().success());
}

#[test]
fn close_ends_the_session_and_so_does_the_ui_going_away() {
    let mut engine = opened();
    engine.send(&Request::Close);
    assert!(engine.wait().success());

    let mut engine = opened();
    engine.close_input();
    assert!(engine.wait().success(), "end of input is a normal end");
}

#[test]
fn startup_failures_are_reported_over_the_protocol() {
    // Valid arguments that name handles this process does not have.
    let mut child = engine_command()
        .args([
            "--file-handle",
            "123456",
            "--region-handle",
            "123457",
            "--region-slots",
            "1",
            "--region-slot-bytes",
            "4096",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    let mut output = std::io::BufReader::new(child.stdout.take().unwrap());
    let hello: Response = vellora_ipc::read_frame(&mut output).unwrap().unwrap();
    assert_eq!(
        hello,
        Response::Hello {
            protocol_version: PROTOCOL_VERSION
        }
    );
    let error: Response = vellora_ipc::read_frame(&mut output).unwrap().unwrap();
    let (_, kind, message) = expect_error(error);
    assert_eq!(kind, ErrorKind::Internal);
    assert!(message.contains("could not start"), "{message}");
    assert!(!child.wait().unwrap().success());
}

#[test]
fn bad_command_lines_exit_with_status_two() {
    for args in [&[][..], &["--bogus"], &["--file-handle", "7"]] {
        let output = engine_command().args(args).output().unwrap();
        assert_eq!(output.status.code(), Some(2), "{args:?}");
        assert!(output.stdout.is_empty(), "stdout belongs to the protocol");
        assert!(!output.stderr.is_empty(), "{args:?}");
    }
}

#[test]
fn version_is_printed() {
    let output = engine_command().arg("--version").output().unwrap();
    assert!(output.status.success());
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.starts_with("vellora-engine "), "{text}");
}
