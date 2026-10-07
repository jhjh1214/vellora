//! # vellora-ipc — UI ↔ engine protocol
//!
//! **Responsibility:**
//!
//! - message types for the engine protocol (requests, responses, events),
//!   framed as length-prefixed `postcard` messages over pipes
//! - a protocol version handshake
//! - descriptors for shared-memory tile slots (the pixels themselves never
//!   travel through the pipe)
//!
//! **Boundaries:**
//!
//! - Pure data and framing. No I/O policy, no PDF knowledge, no PDFium.
//! - Both ends are Rust (`vellora-engine` and `vellora-engine-client`), so no
//!   cross-language schema is needed (ADR-0005).
//! - Every message read from the peer is validated (size caps) before
//!   allocation.
//!
//! **Wire format (version [`PROTOCOL_VERSION`]):** a frame is a `u32`
//! little-endian payload length followed by that many bytes of `postcard`.
//! The UI sends [`Request`]s, the engine sends [`Response`]s. Each side's first
//! message is its `Hello`, which is variant 0 of its enum and carries only the
//! version, so a peer that speaks another version can still be recognised
//! and refused ([`check_version`]).
//!
//! **Compatibility rule:** there is none inside a version. Changing, removing
//! or reordering a message or field bumps [`PROTOCOL_VERSION`]; `Hello` must
//! stay first and keep its shape forever.

mod error;
mod frame;
mod message;

pub use error::Error;
pub use frame::{MAX_FRAME_BYTES, read_frame, write_frame};
pub use message::{
    ErrorKind, MAX_ERROR_MESSAGE_BYTES, MAX_PAGE_SIZES_PER_MESSAGE, MAX_TILE_AREA, MAX_TILE_ORIGIN,
    MAX_TILE_SCALE, MAX_TILE_SIDE, PROTOCOL_VERSION, PageSize, Request, RequestId, Response,
    SlotId, TileRect, Validate, check_version,
};
