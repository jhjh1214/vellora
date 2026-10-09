//! Length-prefixed framing over any `Read` / `Write`.

use std::io::{self, Read, Write};

use serde::{Serialize, de::DeserializeOwned};
use zeroize::Zeroizing;

use crate::{Error, Validate};

/// Largest payload of one frame. Control messages are tiny (a 4096-entry page
/// size list is 32 KiB); pixels never travel here, so 1 MiB is generous.
pub const MAX_FRAME_BYTES: u32 = 1 << 20;

/// Length prefix: `u32`, little-endian.
const HEADER_BYTES: usize = 4;

/// Validates `message`, encodes it and writes one frame, then flushes.
///
/// Header and payload go out in a single `write_all`, so a writer shared
/// behind a lock never interleaves half frames.
///
/// # Errors
///
/// [`Error::Invalid`] for a message that breaks a protocol rule (it is not
/// sent), [`Error::FrameTooLarge`] if it encodes beyond [`MAX_FRAME_BYTES`],
/// [`Error::Io`] if the pipe fails.
pub fn write_frame<W: Write, T: Serialize + Validate>(
    writer: &mut W,
    message: &T,
) -> Result<(), Error> {
    write_frame_with_max(writer, message, MAX_FRAME_BYTES)
}

/// Reads one frame and decodes and validates the message in it.
///
/// Returns `Ok(None)` when the peer closed the pipe cleanly between frames.
/// The length prefix is checked against [`MAX_FRAME_BYTES`] before any payload
/// is read, and the payload buffer grows only as bytes actually arrive, so a
/// peer cannot make us allocate by lying about the length.
///
/// # Errors
///
/// [`Error::FrameTooLarge`], [`Error::Decode`], [`Error::TrailingBytes`],
/// [`Error::Invalid`], or [`Error::Io`] (including `UnexpectedEof` when the
/// pipe closes inside a frame).
pub fn read_frame<R: Read, T: DeserializeOwned + Validate>(
    reader: &mut R,
) -> Result<Option<T>, Error> {
    read_frame_with_max(reader, MAX_FRAME_BYTES)
}

fn write_frame_with_max<W: Write, T: Serialize + Validate>(
    writer: &mut W,
    message: &T,
    max: u32,
) -> Result<(), Error> {
    message.validate()?;
    // A message may carry a password (`Open`), so every buffer that holds its bytes is wiped when
    // it goes. The exact size is asked for first so that the encoder never grows (and so never
    // leaves a copy of the password behind in a freed, smaller buffer).
    let size = postcard::experimental::serialized_size(message).map_err(Error::Encode)?;
    let len = u32::try_from(size)
        .ok()
        .filter(|len| *len <= max)
        .ok_or(Error::FrameTooLarge {
            len: size as u64,
            max,
        })?;
    let mut frame = Zeroizing::new(vec![0_u8; HEADER_BYTES + size]);
    frame[..HEADER_BYTES].copy_from_slice(&len.to_le_bytes());
    let written = postcard::to_slice(message, &mut frame[HEADER_BYTES..]).map_err(Error::Encode)?;
    debug_assert_eq!(written.len(), size);
    writer.write_all(&frame)?;
    writer.flush()?;
    Ok(())
}

fn read_frame_with_max<R: Read, T: DeserializeOwned + Validate>(
    reader: &mut R,
    max: u32,
) -> Result<Option<T>, Error> {
    let mut header = [0u8; HEADER_BYTES];
    if !read_header(reader, &mut header)? {
        return Ok(None);
    }
    let len = u32::from_le_bytes(header);
    if len > max {
        return Err(Error::FrameTooLarge {
            len: u64::from(len),
            max,
        });
    }

    // Wiped when it goes: the payload of an `Open` holds a password.
    let mut payload = Zeroizing::new(Vec::new());
    let got = reader.take(u64::from(len)).read_to_end(&mut payload)?;
    if got != len as usize {
        return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into());
    }

    let (message, rest) = postcard::take_from_bytes::<T>(&payload).map_err(Error::Decode)?;
    if !rest.is_empty() {
        return Err(Error::TrailingBytes { extra: rest.len() });
    }
    message.validate()?;
    Ok(Some(message))
}

/// Fills `header`; `false` means EOF before its first byte (a clean close).
fn read_header<R: Read>(reader: &mut R, header: &mut [u8; HEADER_BYTES]) -> Result<bool, Error> {
    let mut filled = 0;
    while filled < HEADER_BYTES {
        match reader.read(&mut header[filled..]) {
            Ok(0) if filled == 0 => return Ok(false),
            Ok(0) => return Err(io::Error::from(io::ErrorKind::UnexpectedEof).into()),
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(true)
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;

    use proptest::prelude::*;

    use super::*;
    use crate::*;

    fn rect() -> TileRect {
        TileRect {
            x: 256,
            y: 512,
            width: 256,
            height: 256,
        }
    }

    fn requests() -> Vec<Request> {
        vec![
            Request::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
            Request::Open {
                handle_token: u64::MAX,
                password: None,
            },
            Request::Open {
                handle_token: 1,
                password: Some(Password::new("hôtel 🔑")),
            },
            Request::Open {
                handle_token: 1,
                password: Some(Password::new("p".repeat(MAX_PASSWORD_BYTES))),
            },
            Request::RenderTile {
                req_id: RequestId(7),
                page: 9_999,
                scale: 1.5,
                rect: rect(),
                slot: SlotId(3),
                priority: Priority::Visible,
            },
            Request::RenderTile {
                req_id: RequestId(8),
                page: 0,
                scale: 0.25,
                rect: rect(),
                slot: SlotId(0),
                priority: Priority::Prefetch,
            },
            Request::RenderTile {
                req_id: RequestId(9),
                page: 1,
                scale: 0.25,
                rect: rect(),
                slot: SlotId(1),
                priority: Priority::Thumbnail,
            },
            Request::Cancel {
                req_id: RequestId(u64::MAX),
            },
            Request::Close,
            Request::GetOutline {
                req_id: RequestId(10),
                parent: None,
                after: None,
                already: 0,
                limit: 128,
            },
            Request::GetOutline {
                req_id: RequestId(11),
                parent: Some(u32::MAX),
                after: Some(7),
                already: u32::MAX,
                limit: 1,
            },
            Request::GetOutlinePath {
                req_id: RequestId(12),
                page: 4_000,
            },
            Request::GetPageLabels {
                req_id: RequestId(13),
                first: 9_000,
                count: 1024,
            },
            Request::FindPageLabel {
                req_id: RequestId(14),
                text: "A-3 é".into(),
            },
            Request::GetLinks {
                req_id: RequestId(16),
                page: 7,
                skip: 256,
                limit: 256,
            },
        ]
    }

    fn link(action: LinkAction) -> Link {
        Link {
            rect: [1.0, 2.0, 30.5, 40.25],
            action,
        }
    }

    fn entry(title: &str, destination: Option<Destination>) -> OutlineEntry {
        OutlineEntry {
            id: 42,
            title: title.into(),
            destination,
            has_children: true,
            open: false,
            style: TitleStyle {
                bold: true,
                italic: false,
            },
        }
    }

    fn responses() -> Vec<Response> {
        vec![
            Response::Hello {
                protocol_version: PROTOCOL_VERSION,
            },
            Response::Opened {
                page_count: 10_000,
                page_sizes: vec![
                    PageSize {
                        width: 612.0,
                        height: 792.0,
                    },
                    PageSize {
                        width: 595.25,
                        height: 841.75,
                    },
                ],
                repairs: vec![Repair {
                    code: "xref-unreadable".into(),
                    message: "cross-reference unreadable".into(),
                }],
            },
            Response::Opened {
                page_count: 0,
                page_sizes: vec![],
                repairs: vec![],
            },
            Response::TileReady {
                req_id: RequestId(7),
                slot: SlotId(3),
            },
            Response::Error {
                req_id: Some(RequestId(7)),
                kind: ErrorKind::RenderFailed,
                message: "page 9999 does not exist".into(),
            },
            Response::Error {
                req_id: None,
                kind: ErrorKind::VersionMismatch,
                message: String::new(),
            },
            Response::Outline {
                req_id: RequestId(10),
                items: vec![
                    entry(
                        "Chapter 1",
                        Some(Destination {
                            page: 3,
                            fit: Fit::Xyz {
                                left: Some(72.0),
                                top: None,
                                zoom: Some(1.5),
                            },
                        }),
                    ),
                    entry("", None),
                    entry(
                        "Figure",
                        Some(Destination {
                            page: 0,
                            fit: Fit::FitR {
                                left: 0.0,
                                bottom: 1.0,
                                right: 2.0,
                                top: 3.0,
                            },
                        }),
                    ),
                ],
                more: true,
            },
            Response::Outline {
                req_id: RequestId(11),
                items: vec![],
                more: false,
            },
            Response::OutlinePath {
                req_id: RequestId(12),
                path: vec![5, 9, 31],
            },
            Response::PageLabels {
                req_id: RequestId(13),
                first: 2,
                defined: true,
                labels: vec!["iii".into(), "A-1".into()],
            },
            Response::PageFound {
                req_id: RequestId(14),
                page: Some(8),
            },
            Response::PageFound {
                req_id: RequestId(15),
                page: None,
            },
        ]
    }

    fn link_responses() -> Vec<Response> {
        vec![
            Response::Links {
                req_id: RequestId(16),
                page: 7,
                links: vec![
                    link(LinkAction::GoTo(Destination {
                        page: 2,
                        fit: Fit::Fit,
                    })),
                    link(LinkAction::Unresolved),
                    link(LinkAction::Uri("https://example.org/é?x=1".into())),
                    link(LinkAction::Named(NamedAction::LastPage)),
                    link(LinkAction::Inert("Launch".into())),
                ],
                more: true,
            },
            Response::Links {
                req_id: RequestId(17),
                page: 0,
                links: vec![],
                more: false,
            },
        ]
    }

    fn round_trip<T>(message: &T) -> T
    where
        T: Serialize + DeserializeOwned + Validate,
    {
        let mut wire = Vec::new();
        write_frame(&mut wire, message).unwrap();
        let mut cursor = Cursor::new(wire);
        let back = read_frame(&mut cursor).unwrap().expect("one frame");
        assert!(
            read_frame::<_, T>(&mut cursor).unwrap().is_none(),
            "exactly one frame"
        );
        back
    }

    #[test]
    fn every_request_round_trips() {
        for message in requests() {
            assert_eq!(round_trip(&message), message);
        }
    }

    #[test]
    fn every_response_round_trips() {
        for message in responses().into_iter().chain(link_responses()) {
            assert_eq!(round_trip(&message), message);
        }
    }

    #[test]
    fn several_frames_in_a_row_then_clean_eof() {
        let mut wire = Vec::new();
        for message in requests() {
            write_frame(&mut wire, &message).unwrap();
        }
        let mut cursor = Cursor::new(wire);
        for expected in requests() {
            assert_eq!(
                read_frame::<_, Request>(&mut cursor).unwrap(),
                Some(expected)
            );
        }
        assert!(read_frame::<_, Request>(&mut cursor).unwrap().is_none());
    }

    #[test]
    fn frame_layout_is_u32_le_length_then_postcard() {
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Request::Hello {
                protocol_version: 0,
            },
        )
        .unwrap();
        // Variant 0, then the varint 0.
        assert_eq!(wire, [2, 0, 0, 0, 0, 0]);
    }

    #[test]
    fn priority_is_the_last_byte_of_a_tile_request_in_variant_order() {
        for (priority, byte) in [
            (Priority::Visible, 0),
            (Priority::Prefetch, 1),
            (Priority::Thumbnail, 2),
        ] {
            let mut wire = Vec::new();
            write_frame(
                &mut wire,
                &Request::RenderTile {
                    req_id: RequestId(1),
                    page: 0,
                    scale: 1.0,
                    rect: rect(),
                    slot: SlotId(0),
                    priority,
                },
            )
            .unwrap();
            assert_eq!(wire.last(), Some(&byte), "{priority:?}");
        }
    }

    #[test]
    fn oversized_frame_is_rejected_before_any_payload_is_read() {
        let mut wire = Vec::new();
        wire.extend_from_slice(&(MAX_FRAME_BYTES + 1).to_le_bytes());
        wire.extend_from_slice(&[0xAA; 64]);
        let mut cursor = Cursor::new(wire);
        let err = read_frame::<_, Request>(&mut cursor).unwrap_err();
        assert!(matches!(err, Error::FrameTooLarge { len, max }
            if len == u64::from(MAX_FRAME_BYTES) + 1 && max == MAX_FRAME_BYTES));
        assert_eq!(
            cursor.position(),
            4,
            "only the length prefix may be consumed"
        );

        let mut huge = Cursor::new(u32::MAX.to_le_bytes().to_vec());
        assert!(matches!(
            read_frame::<_, Request>(&mut huge),
            Err(Error::FrameTooLarge { .. })
        ));
    }

    #[test]
    fn a_frame_of_exactly_the_cap_is_read_not_refused() {
        let mut wire = Vec::new();
        write_frame_with_max(&mut wire, &Request::Close, 1).unwrap();
        let mut cursor = Cursor::new(wire);
        assert_eq!(
            read_frame_with_max::<_, Request>(&mut cursor, 1).unwrap(),
            Some(Request::Close)
        );
    }

    #[test]
    fn writer_refuses_a_message_over_the_cap_and_sends_nothing() {
        let message = Response::Error {
            req_id: None,
            kind: ErrorKind::Internal,
            message: "x".repeat(100),
        };
        let mut wire = Vec::new();
        let err = write_frame_with_max(&mut wire, &message, 50).unwrap_err();
        assert!(matches!(err, Error::FrameTooLarge { max: 50, .. }));
        assert_eq!(wire.len(), 0, "nothing may be sent");
    }

    #[test]
    fn a_full_page_size_chunk_fits_in_one_frame() {
        let message = Response::Opened {
            page_count: 100_000,
            page_sizes: vec![
                PageSize {
                    width: 612.0,
                    height: 792.0
                };
                MAX_PAGE_SIZES_PER_MESSAGE
            ],
            repairs: vec![],
        };
        assert_eq!(round_trip(&message), message);
    }

    #[test]
    fn truncated_input_is_an_error_not_a_close() {
        let mut wire = Vec::new();
        let open = Request::Open {
            handle_token: 1,
            password: Some(Password::new("secret")),
        };
        write_frame(&mut wire, &open).unwrap();
        for cut in 1..wire.len() {
            let mut cursor = Cursor::new(wire[..cut].to_vec());
            let err = read_frame::<_, Request>(&mut cursor).unwrap_err();
            assert!(
                matches!(&err, Error::Io(e) if e.kind() == io::ErrorKind::UnexpectedEof),
                "cut at {cut}: {err:?}"
            );
        }
    }

    #[test]
    fn garbage_payload_is_a_decode_error() {
        let mut wire = 3u32.to_le_bytes().to_vec();
        wire.extend_from_slice(&[0xFF, 0xFF, 0xFF]); // no such variant
        let err = read_frame::<_, Request>(&mut Cursor::new(wire)).unwrap_err();
        assert!(matches!(err, Error::Decode(_)), "{err:?}");

        let empty = 0u32.to_le_bytes().to_vec();
        let err = read_frame::<_, Request>(&mut Cursor::new(empty)).unwrap_err();
        assert!(matches!(err, Error::Decode(_)), "{err:?}");
    }

    #[test]
    fn trailing_bytes_inside_a_frame_are_refused() {
        let mut payload = postcard::to_stdvec(&Request::Close).unwrap();
        payload.push(0);
        let mut wire = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
        wire.extend_from_slice(&payload);
        let err = read_frame::<_, Request>(&mut Cursor::new(wire)).unwrap_err();
        assert!(matches!(err, Error::TrailingBytes { extra: 1 }), "{err:?}");
    }

    #[test]
    fn version_mismatch_is_detected_from_the_hello() {
        // A peer from the future still sends a Hello we can decode.
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Response::Hello {
                protocol_version: PROTOCOL_VERSION + 1,
            },
        )
        .unwrap();
        let hello = read_frame::<_, Response>(&mut Cursor::new(wire))
            .unwrap()
            .unwrap();
        let Response::Hello { protocol_version } = hello else {
            panic!("not a Hello")
        };
        let err = check_version(protocol_version).unwrap_err();
        assert!(matches!(
            err,
            Error::VersionMismatch { ours: PROTOCOL_VERSION, theirs } if theirs == PROTOCOL_VERSION + 1
        ));
        assert!(check_version(PROTOCOL_VERSION).is_ok());
    }

    fn render_tile(scale: f32, rect: TileRect) -> Request {
        Request::RenderTile {
            req_id: RequestId(1),
            page: 0,
            scale,
            rect,
            slot: SlotId(0),
            priority: Priority::Visible,
        }
    }

    #[test]
    fn render_tile_limits_are_enforced_on_both_sides_of_the_wire() {
        let ok = rect();
        let bad = [
            render_tile(0.0, ok),
            render_tile(-1.0, ok),
            render_tile(f32::NAN, ok),
            render_tile(f32::INFINITY, ok),
            render_tile(MAX_TILE_SCALE + 0.5, ok),
            render_tile(1.0, TileRect { width: 0, ..ok }),
            render_tile(1.0, TileRect { height: 0, ..ok }),
            render_tile(
                1.0,
                TileRect {
                    width: MAX_TILE_SIDE + 1,
                    ..ok
                },
            ),
            render_tile(
                1.0,
                TileRect {
                    height: MAX_TILE_SIDE + 1,
                    ..ok
                },
            ),
            render_tile(
                1.0,
                TileRect {
                    x: MAX_TILE_ORIGIN + 1,
                    ..ok
                },
            ),
            render_tile(
                1.0,
                TileRect {
                    y: MAX_TILE_ORIGIN + 1,
                    ..ok
                },
            ),
        ];
        for message in &bad {
            // Not sent...
            let mut wire = Vec::new();
            assert!(
                matches!(write_frame(&mut wire, message), Err(Error::Invalid(_))),
                "{message:?}"
            );
            assert_eq!(wire.len(), 0, "nothing may be sent");
            // ...and, if a hostile peer sends it anyway, not accepted.
            let payload = postcard::to_stdvec(message).unwrap();
            let mut raw = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
            raw.extend_from_slice(&payload);
            assert!(
                matches!(
                    read_frame::<_, Request>(&mut Cursor::new(raw)),
                    Err(Error::Invalid(_))
                ),
                "{message:?}"
            );
        }
        let edge = render_tile(
            MAX_TILE_SCALE,
            TileRect {
                x: MAX_TILE_ORIGIN,
                y: 0,
                width: 4096,
                height: 4096,
            },
        );
        assert_eq!(round_trip(&edge), edge);
    }

    fn open_with(password: &str) -> Request {
        Request::Open {
            handle_token: 1,
            password: Some(Password::new(password)),
        }
    }

    #[test]
    fn password_limits_are_enforced_on_both_sides_of_the_wire() {
        let bad = [
            open_with(&"p".repeat(MAX_PASSWORD_BYTES + 1)),
            // Two bytes a character: the limit counts bytes.
            open_with(&"é".repeat(MAX_PASSWORD_BYTES / 2 + 1)),
            open_with("pass\0word"),
            open_with("\0"),
        ];
        for message in &bad {
            let mut wire = Vec::new();
            assert!(
                matches!(write_frame(&mut wire, message), Err(Error::Invalid(_))),
                "{message:?}"
            );
            assert_eq!(wire.len(), 0, "nothing may be sent");
            let payload = postcard::to_stdvec(message).unwrap();
            let mut raw = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
            raw.extend_from_slice(&payload);
            assert!(
                matches!(
                    read_frame::<_, Request>(&mut Cursor::new(raw)),
                    Err(Error::Invalid(_))
                ),
                "{message:?}"
            );
        }
        // The empty password is a password (a user may submit nothing); the limit is inclusive.
        for ok in [
            open_with(""),
            open_with(&"é".repeat(MAX_PASSWORD_BYTES / 2)),
        ] {
            assert_eq!(round_trip(&ok), ok);
        }
    }

    #[test]
    fn a_password_never_shows_in_debug_output() {
        let request = open_with("hunter2-correct-horse");
        for text in [format!("{request:?}"), format!("{request:#?}")] {
            assert!(!text.contains("hunter2"), "{text}");
            assert!(text.contains("redacted"), "{text}");
        }
        let password = Password::new("hunter2");
        assert_eq!(password.expose(), "hunter2");
        assert!(!format!("{password:?}").contains("hunter2"));
    }

    #[test]
    fn open_has_the_documented_layout() {
        // Variant 1, the token as a varint, then the option tag.
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Request::Open {
                handle_token: 1,
                password: None,
            },
        )
        .unwrap();
        assert_eq!(wire, [3, 0, 0, 0, 1, 1, 0]);
        let mut wire = Vec::new();
        write_frame(&mut wire, &open_with("ab")).unwrap();
        // ... then Some, the text's length and its bytes.
        assert_eq!(wire, [6, 0, 0, 0, 1, 1, 1, 2, b'a', b'b']);
    }

    #[test]
    fn error_kinds_keep_their_wire_order() {
        // The variant order is the wire format: a new kind goes last.
        let kinds = [
            ErrorKind::VersionMismatch,
            ErrorKind::InvalidRequest,
            ErrorKind::OpenFailed,
            ErrorKind::RenderFailed,
            ErrorKind::Internal,
            ErrorKind::PasswordRequired,
            ErrorKind::WrongPassword,
            ErrorKind::ReadFailed,
        ];
        for (index, kind) in kinds.into_iter().enumerate() {
            let mut wire = Vec::new();
            write_frame(
                &mut wire,
                &Response::Error {
                    req_id: None,
                    kind,
                    message: String::new(),
                },
            )
            .unwrap();
            // Frame header (4), variant `Error` (3), no request id (0), the kind, empty text (0).
            assert_eq!(
                wire[wire.len() - 2],
                u8::try_from(index).unwrap(),
                "{kind:?}"
            );
        }
    }

    #[test]
    fn response_limits_are_enforced() {
        let size = PageSize {
            width: 1.0,
            height: 1.0,
        };
        let bad = [
            Response::Opened {
                page_count: 5,
                page_sizes: vec![size; 6],
                repairs: vec![],
            },
            Response::Opened {
                page_count: u32::MAX,
                page_sizes: vec![size; MAX_PAGE_SIZES_PER_MESSAGE + 1],
                repairs: vec![],
            },
            Response::Opened {
                page_count: 1,
                page_sizes: vec![PageSize {
                    width: f32::NAN,
                    height: 1.0,
                }],
                repairs: vec![],
            },
            Response::Opened {
                page_count: 1,
                page_sizes: vec![PageSize {
                    width: 1.0,
                    height: 0.0,
                }],
                repairs: vec![],
            },
            Response::Opened {
                page_count: 1,
                page_sizes: vec![size],
                repairs: vec![repair("a", "b"); MAX_REPAIRS + 1],
            },
            Response::Opened {
                page_count: 1,
                page_sizes: vec![size],
                repairs: vec![repair(&"c".repeat(MAX_REPAIR_CODE_BYTES + 1), "b")],
            },
            Response::Opened {
                page_count: 1,
                page_sizes: vec![size],
                repairs: vec![repair("a", &"m".repeat(MAX_REPAIR_MESSAGE_BYTES + 1))],
            },
            Response::Error {
                req_id: None,
                kind: ErrorKind::Internal,
                message: "x".repeat(MAX_ERROR_MESSAGE_BYTES + 1),
            },
        ];
        for message in &bad {
            assert!(
                matches!(message.validate(), Err(Error::Invalid(_))),
                "{message:?}"
            );
            let mut wire = Vec::new();
            assert!(matches!(
                write_frame(&mut wire, message),
                Err(Error::Invalid(_))
            ));
        }
    }

    /// Navigation messages came with version 4: they go after the older variants, which keep their
    /// numbers (`Close` is request 4, `Error` response 3).
    #[test]
    fn navigation_messages_follow_the_older_variants_on_the_wire() {
        let request = |message: &Request| {
            let mut wire = Vec::new();
            write_frame(&mut wire, message).unwrap();
            wire[4]
        };
        assert_eq!(request(&Request::Close), 4);
        assert_eq!(
            request(&Request::GetOutline {
                req_id: RequestId(1),
                parent: None,
                after: None,
                already: 0,
                limit: 1
            }),
            5
        );
        assert_eq!(
            request(&Request::GetOutlinePath {
                req_id: RequestId(1),
                page: 0
            }),
            6
        );
        assert_eq!(
            request(&Request::GetPageLabels {
                req_id: RequestId(1),
                first: 0,
                count: 1
            }),
            7
        );
        assert_eq!(
            request(&Request::FindPageLabel {
                req_id: RequestId(1),
                text: String::new()
            }),
            8
        );
        let response = |message: &Response| {
            let mut wire = Vec::new();
            write_frame(&mut wire, message).unwrap();
            wire[4]
        };
        assert_eq!(
            response(&Response::Error {
                req_id: None,
                kind: ErrorKind::Internal,
                message: String::new()
            }),
            3
        );
        assert_eq!(
            response(&Response::Outline {
                req_id: RequestId(1),
                items: vec![],
                more: false
            }),
            4
        );
        assert_eq!(
            response(&Response::OutlinePath {
                req_id: RequestId(1),
                path: vec![]
            }),
            5
        );
        assert_eq!(
            response(&Response::PageLabels {
                req_id: RequestId(1),
                first: 0,
                defined: false,
                labels: vec![]
            }),
            6
        );
        assert_eq!(
            response(&Response::PageFound {
                req_id: RequestId(1),
                page: None
            }),
            7
        );
    }

    #[test]
    fn navigation_request_limits_are_enforced_on_both_sides_of_the_wire() {
        let outline = |limit| Request::GetOutline {
            req_id: RequestId(1),
            parent: None,
            after: None,
            already: 0,
            limit,
        };
        let labels = |count| Request::GetPageLabels {
            req_id: RequestId(1),
            first: 0,
            count,
        };
        let find = |text: String| Request::FindPageLabel {
            req_id: RequestId(1),
            text,
        };
        let limit = u32::try_from(MAX_OUTLINE_ITEMS_PER_MESSAGE).unwrap();
        let count = u32::try_from(MAX_LABELS_PER_MESSAGE).unwrap();
        for good in [
            outline(1),
            outline(limit),
            labels(1),
            labels(count),
            find("x".repeat(MAX_LABEL_BYTES)),
        ] {
            assert_eq!(round_trip(&good), good);
        }
        for bad in [
            outline(0),
            outline(limit + 1),
            outline(u32::MAX),
            labels(0),
            labels(count + 1),
            find("x".repeat(MAX_LABEL_BYTES + 1)),
        ] {
            assert!(matches!(bad.validate(), Err(Error::Invalid(_))), "{bad:?}");
            let mut wire = Vec::new();
            assert!(matches!(
                write_frame(&mut wire, &bad),
                Err(Error::Invalid(_))
            ));
        }
    }

    #[test]
    fn navigation_response_limits_are_enforced() {
        let at = |page| {
            Some(Destination {
                page,
                fit: Fit::Fit,
            })
        };
        let item = || entry("t", at(1));
        let outline = |items| Response::Outline {
            req_id: RequestId(1),
            items,
            more: false,
        };
        let bad = [
            outline(vec![item(); MAX_OUTLINE_ITEMS_PER_MESSAGE + 1]),
            outline(vec![entry(&"t".repeat(MAX_OUTLINE_TITLE_BYTES + 1), None)]),
            outline(vec![entry(
                "t",
                Some(Destination {
                    page: 0,
                    fit: Fit::Xyz {
                        left: Some(f32::NAN),
                        top: None,
                        zoom: None,
                    },
                }),
            )]),
            outline(vec![entry(
                "t",
                Some(Destination {
                    page: 0,
                    fit: Fit::FitR {
                        left: 0.0,
                        bottom: 0.0,
                        right: f32::INFINITY,
                        top: 0.0,
                    },
                }),
            )]),
            Response::OutlinePath {
                req_id: RequestId(1),
                path: vec![1; MAX_OUTLINE_PATH + 1],
            },
            Response::PageLabels {
                req_id: RequestId(1),
                first: 0,
                defined: true,
                labels: vec!["a".into(); MAX_LABELS_PER_MESSAGE + 1],
            },
            Response::PageLabels {
                req_id: RequestId(1),
                first: 0,
                defined: true,
                labels: vec!["a".repeat(MAX_LABEL_BYTES + 1)],
            },
        ];
        for message in &bad {
            assert!(
                matches!(message.validate(), Err(Error::Invalid(_))),
                "{message:?}"
            );
        }
        // The same sizes minus one are fine.
        for good in [
            outline(vec![item(); MAX_OUTLINE_ITEMS_PER_MESSAGE]),
            outline(vec![entry(&"t".repeat(MAX_OUTLINE_TITLE_BYTES), None)]),
            Response::OutlinePath {
                req_id: RequestId(1),
                path: vec![1; MAX_OUTLINE_PATH],
            },
            Response::PageLabels {
                req_id: RequestId(1),
                first: 0,
                defined: true,
                labels: vec!["a".repeat(MAX_LABEL_BYTES); MAX_LABELS_PER_MESSAGE],
            },
        ] {
            assert_eq!(round_trip(&good), good);
        }
    }

    #[test]
    fn the_largest_outline_and_label_messages_fit_in_one_frame() {
        let full = Response::Outline {
            req_id: RequestId(u64::MAX),
            items: vec![
                OutlineEntry {
                    id: u32::MAX,
                    title: "é".repeat(MAX_OUTLINE_TITLE_BYTES / 2),
                    destination: Some(Destination {
                        page: u32::MAX,
                        fit: Fit::FitR {
                            left: 1.0,
                            bottom: 1.0,
                            right: 1.0,
                            top: 1.0,
                        },
                    }),
                    has_children: true,
                    open: true,
                    style: TitleStyle {
                        bold: true,
                        italic: true,
                    },
                };
                MAX_OUTLINE_ITEMS_PER_MESSAGE
            ],
            more: true,
        };
        assert_eq!(round_trip(&full), full);
        let labels = Response::PageLabels {
            req_id: RequestId(u64::MAX),
            first: u32::MAX,
            defined: true,
            labels: vec!["é".repeat(MAX_LABEL_BYTES / 2); MAX_LABELS_PER_MESSAGE],
        };
        assert_eq!(round_trip(&labels), labels);
    }

    /// Links came with version 5, after the navigation messages (`FindPageLabel` is request 8,
    /// `PageFound` response 7).
    #[test]
    fn links_follow_the_older_variants_on_the_wire() {
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Request::GetLinks {
                req_id: RequestId(1),
                page: 0,
                skip: 0,
                limit: 1,
            },
        )
        .unwrap();
        assert_eq!(wire[4], 9);
        let mut wire = Vec::new();
        write_frame(
            &mut wire,
            &Response::Links {
                req_id: RequestId(1),
                page: 0,
                links: vec![],
                more: false,
            },
        )
        .unwrap();
        assert_eq!(wire[4], 8);
    }

    #[test]
    fn link_limits_are_enforced_on_both_sides_of_the_wire() {
        let get = |limit| Request::GetLinks {
            req_id: RequestId(1),
            page: 0,
            skip: 0,
            limit,
        };
        let max = u32::try_from(MAX_LINKS_PER_MESSAGE).unwrap();
        for good in [get(1), get(max)] {
            assert_eq!(round_trip(&good), good);
        }
        for bad in [get(0), get(max + 1), get(u32::MAX)] {
            assert!(matches!(bad.validate(), Err(Error::Invalid(_))), "{bad:?}");
            let mut wire = Vec::new();
            assert!(matches!(
                write_frame(&mut wire, &bad),
                Err(Error::Invalid(_))
            ));
        }

        let links = |links| Response::Links {
            req_id: RequestId(1),
            page: 0,
            links,
            more: false,
        };
        let bad = [
            links(vec![
                link(LinkAction::Unresolved);
                MAX_LINKS_PER_MESSAGE + 1
            ]),
            links(vec![link(LinkAction::Uri("u".repeat(MAX_URI_BYTES + 1)))]),
            links(vec![link(LinkAction::Inert(
                "k".repeat(MAX_LINK_KIND_BYTES + 1),
            ))]),
            links(vec![Link {
                rect: [0.0, f32::NAN, 1.0, 1.0],
                action: LinkAction::Unresolved,
            }]),
            links(vec![link(LinkAction::GoTo(Destination {
                page: 0,
                fit: Fit::FitH {
                    top: Some(f32::INFINITY),
                },
            }))]),
        ];
        for message in &bad {
            assert!(
                matches!(message.validate(), Err(Error::Invalid(_))),
                "{message:?}"
            );
        }
        // The same sizes at the limit are fine, and the largest message fits one frame.
        let full = links(vec![
            link(LinkAction::Uri("é".repeat(MAX_URI_BYTES / 2)));
            MAX_LINKS_PER_MESSAGE
        ]);
        assert_eq!(round_trip(&full), full);
        let kinds = links(vec![link(LinkAction::Inert(
            "k".repeat(MAX_LINK_KIND_BYTES),
        ))]);
        assert_eq!(round_trip(&kinds), kinds);
    }

    fn text_char(ch: char, flags: u8) -> TextChar {
        TextChar {
            ch,
            rect: [1.0, 2.0, 7.5, 14.25],
            size: 12.0,
            flags,
            word: 3,
            line: 1,
        }
    }

    fn text_messages() -> (Vec<Request>, Vec<Response>) {
        (
            vec![Request::GetTextPage {
                req_id: RequestId(20),
                page: 4,
                skip: 8192,
                limit: 8192,
            }],
            vec![
                Response::TextPage {
                    req_id: RequestId(20),
                    page: 4,
                    skip: 2,
                    total: 5,
                    chars: vec![
                        text_char('é', 0),
                        text_char(' ', TEXT_GENERATED),
                        text_char('-', TEXT_HYPHEN),
                    ],
                },
                Response::TextPage {
                    req_id: RequestId(21),
                    page: 0,
                    skip: 0,
                    total: 0,
                    chars: vec![],
                },
            ],
        )
    }

    #[test]
    fn text_messages_round_trip_and_follow_the_older_variants_on_the_wire() {
        let (requests, responses) = text_messages();
        for message in requests {
            assert_eq!(round_trip(&message), message);
            let mut wire = Vec::new();
            write_frame(&mut wire, &message).unwrap();
            assert_eq!(wire[4], 10, "after GetLinks (9)");
        }
        for message in responses {
            assert_eq!(round_trip(&message), message);
            let mut wire = Vec::new();
            write_frame(&mut wire, &message).unwrap();
            assert_eq!(wire[4], 9, "after Links (8)");
        }
    }

    #[test]
    fn text_limits_are_enforced_on_both_sides_of_the_wire() {
        let get = |limit| Request::GetTextPage {
            req_id: RequestId(1),
            page: 0,
            skip: 0,
            limit,
        };
        let max = u32::try_from(MAX_TEXT_CHARS_PER_MESSAGE).unwrap();
        for good in [get(1), get(max)] {
            assert_eq!(round_trip(&good), good);
        }
        for bad in [get(0), get(max + 1), get(u32::MAX)] {
            assert!(matches!(bad.validate(), Err(Error::Invalid(_))), "{bad:?}");
            let mut wire = Vec::new();
            assert!(matches!(
                write_frame(&mut wire, &bad),
                Err(Error::Invalid(_))
            ));
        }

        let page = |skip, total, chars| Response::TextPage {
            req_id: RequestId(1),
            page: 0,
            skip,
            total,
            chars,
        };
        let one = || text_char('a', 0);
        let bad = [
            page(0, u32::MAX, vec![one(); MAX_TEXT_CHARS_PER_MESSAGE + 1]),
            // More characters than the page has after `skip`.
            page(4, 5, vec![one(), one()]),
            page(u32::MAX, u32::MAX, vec![one()]),
            page(
                0,
                1,
                vec![TextChar {
                    rect: [0.0, f32::NAN, 1.0, 1.0],
                    ..one()
                }],
            ),
            page(
                0,
                1,
                vec![TextChar {
                    size: f32::INFINITY,
                    ..one()
                }],
            ),
        ];
        for message in &bad {
            assert!(
                matches!(message.validate(), Err(Error::Invalid(_))),
                "{message:?}"
            );
        }
        // The largest message fits a frame, with the widest characters.
        let wide = TextChar {
            ch: '\u{10FFFF}',
            rect: [f32::MAX, f32::MIN_POSITIVE, 1.0e30, -1.0e30],
            size: f32::MAX,
            flags: 255,
            word: u32::MAX,
            line: u32::MAX,
        };
        let full = page(0, u32::MAX, vec![wide; MAX_TEXT_CHARS_PER_MESSAGE]);
        assert_eq!(round_trip(&full), full);
    }

    fn repair(code: &str, message: &str) -> Repair {
        Repair {
            code: code.into(),
            message: message.into(),
        }
    }

    #[test]
    fn repairs_at_the_limits_are_accepted() {
        let message = Response::Opened {
            page_count: 0,
            page_sizes: vec![],
            repairs: vec![
                repair(
                    &"c".repeat(MAX_REPAIR_CODE_BYTES),
                    &"m".repeat(MAX_REPAIR_MESSAGE_BYTES)
                );
                MAX_REPAIRS
            ],
        };
        assert_eq!(round_trip(&message), message);
    }

    #[test]
    fn an_error_message_at_the_limit_is_accepted() {
        let message = Response::Error {
            req_id: None,
            kind: ErrorKind::Internal,
            message: "x".repeat(MAX_ERROR_MESSAGE_BYTES),
        };
        assert_eq!(round_trip(&message), message);
    }

    proptest! {
        #[test]
        fn arbitrary_bytes_never_panic_the_reader(bytes in proptest::collection::vec(any::<u8>(), 0..512)) {
            let _ = read_frame::<_, Request>(&mut Cursor::new(bytes.clone()));
            let _ = read_frame::<_, Response>(&mut Cursor::new(bytes));
        }

        #[test]
        fn arbitrary_payloads_behind_a_valid_header_never_panic(payload in proptest::collection::vec(any::<u8>(), 0..512)) {
            let mut wire = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
            wire.extend_from_slice(&payload);
            let _ = read_frame::<_, Request>(&mut Cursor::new(wire.clone()));
            let _ = read_frame::<_, Response>(&mut Cursor::new(wire));
        }

        #[test]
        fn whatever_the_reader_accepts_is_valid_and_re_encodes_identically(payload in proptest::collection::vec(any::<u8>(), 1..64)) {
            let mut wire = u32::try_from(payload.len()).unwrap().to_le_bytes().to_vec();
            wire.extend_from_slice(&payload);
            if let Ok(Some(message)) = read_frame::<_, Request>(&mut Cursor::new(wire)) {
                prop_assert!(message.validate().is_ok());
                prop_assert_eq!(round_trip(&message), message);
            }
        }
    }
}
