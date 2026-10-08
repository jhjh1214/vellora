//! Length-prefixed framing over any `Read` / `Write`.

use std::io::{self, Read, Write};

use serde::{Serialize, de::DeserializeOwned};

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
    let payload = postcard::to_stdvec(message).map_err(Error::Encode)?;
    let len = u32::try_from(payload.len())
        .ok()
        .filter(|len| *len <= max)
        .ok_or(Error::FrameTooLarge {
            len: payload.len() as u64,
            max,
        })?;
    let mut frame = Vec::with_capacity(HEADER_BYTES + payload.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.extend_from_slice(&payload);
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

    let mut payload = Vec::new();
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
        ]
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
        for message in responses() {
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
        write_frame(&mut wire, &Request::Open { handle_token: 1 }).unwrap();
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
