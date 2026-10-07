//! Typed errors for framing, decoding and validation.

use std::io;

/// Everything that can go wrong sending or receiving a protocol message.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The pipe failed, or the peer closed it in the middle of a frame.
    #[error("i/o error on the engine pipe: {0}")]
    Io(#[from] io::Error),
    /// A frame announced (or a message encoded to) more than the allowed size.
    /// On the read side this is raised from the length prefix alone, before any
    /// payload is read or allocated.
    #[error("frame of {len} bytes exceeds the {max}-byte limit")]
    FrameTooLarge {
        /// Announced or encoded payload length.
        len: u64,
        /// The limit that applies.
        max: u32,
    },
    /// The payload is not a valid `postcard` encoding of the expected message.
    #[error("malformed message: {0}")]
    Decode(postcard::Error),
    /// A message could not be encoded (only possible for a type that violates
    /// `serde`'s data model, so this signals a bug).
    #[error("cannot encode message: {0}")]
    Encode(postcard::Error),
    /// A valid message was followed by extra bytes inside the same frame.
    #[error("{extra} unexpected trailing bytes in frame")]
    TrailingBytes {
        /// Number of bytes left after the message.
        extra: usize,
    },
    /// The message decoded but breaks a protocol rule (out-of-range value,
    /// oversized list, non-finite number).
    #[error("invalid message: {0}")]
    Invalid(&'static str),
    /// The peer speaks another protocol version. Fatal and user-visible.
    #[error("protocol version mismatch: this side speaks {ours}, the peer speaks {theirs}")]
    VersionMismatch {
        /// Our [`PROTOCOL_VERSION`](crate::PROTOCOL_VERSION).
        ours: u32,
        /// The version in the peer's `Hello`.
        theirs: u32,
    },
}
