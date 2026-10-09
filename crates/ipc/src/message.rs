//! The messages of the protocol and their validation.

use std::fmt;

use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

use crate::Error;

/// Version spoken by this build. Bumped on any wire-visible change.
pub const PROTOCOL_VERSION: u32 = 4;

/// Longest [`Password`] in bytes. The Standard Security Handler reads at most 127 bytes of a
/// revision 6 password and 32 of an older one, so this is generous.
pub const MAX_PASSWORD_BYTES: usize = 256;

/// Most page sizes one `Opened` may carry (the first chunk; the rest follow
/// lazily so opening a 10,000-page file does not wait for every page).
pub const MAX_PAGE_SIZES_PER_MESSAGE: usize = 4096;
/// Longest `Error` message text in bytes.
pub const MAX_ERROR_MESSAGE_BYTES: usize = 4096;
/// Most [`Repair`] entries one `Opened` may carry. A file with more damage gets a last entry that
/// says how many were left out.
pub const MAX_REPAIRS: usize = 32;
/// Longest [`Repair::code`] in bytes.
pub const MAX_REPAIR_CODE_BYTES: usize = 48;
/// Longest [`Repair::message`] in bytes.
pub const MAX_REPAIR_MESSAGE_BYTES: usize = 256;
/// Most outline items one `Outline` may carry. With titles of at most
/// [`MAX_OUTLINE_TITLE_BYTES`] this stays far below the frame limit.
pub const MAX_OUTLINE_ITEMS_PER_MESSAGE: usize = 128;
/// Longest outline item title in bytes.
pub const MAX_OUTLINE_TITLE_BYTES: usize = 4096;
/// Most ids one `OutlinePath` may carry (the engine's nesting limit is 64).
pub const MAX_OUTLINE_PATH: usize = 64;
/// Most labels one `PageLabels` may carry.
pub const MAX_LABELS_PER_MESSAGE: usize = 1024;
/// Longest page label in bytes (a prefix of 64 characters and a number).
pub const MAX_LABEL_BYTES: usize = 320;
/// Largest accepted tile scale (device pixels per point).
pub const MAX_TILE_SCALE: f32 = 64.0;
/// Longest accepted tile side in device pixels.
pub const MAX_TILE_SIDE: u32 = 4096;
/// Most pixels in one tile.
pub const MAX_TILE_AREA: u64 = 1 << 24;
/// Largest accepted tile origin coordinate in device pixels.
pub const MAX_TILE_ORIGIN: u32 = 1 << 24;

// The render limits live in `vellora-render` (`Error::InvalidRequest`); they are repeated here so
// the engine can refuse a hostile request before it reaches a queue. Keep the two in step.

/// Correlates a request with the responses that refer to it. Chosen by the UI.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct RequestId(pub u64);

/// Index of a tile slot in the client-owned shared-memory ring.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SlotId(pub u32);

/// Page dimensions in points (1/72 inch), after `/Rotate` and the crop box.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PageSize {
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
}

/// One thing the engine had to repair, or disagreed about, to show a damaged document.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Repair {
    /// Stable short identifier (kebab-case) for tests and logs, at most
    /// [`MAX_REPAIR_CODE_BYTES`] bytes.
    pub code: String,
    /// One line for the user, at most [`MAX_REPAIR_MESSAGE_BYTES`] bytes.
    pub message: String,
}

/// How a page is shown when a destination is followed (ISO 32000-2 Table 151). Coordinates are in
/// default user space units of the page; `None` leaves that value as it is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub enum Fit {
    /// `/XYZ`: the point (`left`, `top`) at the top left of the window, at `zoom` (1.0 is 100%).
    Xyz {
        /// Left edge.
        left: Option<f32>,
        /// Top edge.
        top: Option<f32>,
        /// Magnification, greater than zero.
        zoom: Option<f32>,
    },
    /// `/Fit`: the whole page.
    Fit,
    /// `/FitH`: the page's width, `top` at the top.
    FitH {
        /// Top edge.
        top: Option<f32>,
    },
    /// `/FitV`: the page's height, `left` at the left.
    FitV {
        /// Left edge.
        left: Option<f32>,
    },
    /// `/FitR`: the rectangle.
    FitR {
        /// Left edge.
        left: f32,
        /// Bottom edge.
        bottom: f32,
        /// Right edge.
        right: f32,
        /// Top edge.
        top: f32,
    },
    /// `/FitB`: the page's bounding box.
    FitB,
    /// `/FitBH`: the bounding box's width.
    FitBH {
        /// Top edge.
        top: Option<f32>,
    },
    /// `/FitBV`: the bounding box's height.
    FitBV {
        /// Left edge.
        left: Option<f32>,
    },
}

impl Fit {
    /// Whether every number in it is finite.
    fn is_finite(&self) -> bool {
        let all = |values: &[Option<f32>]| values.iter().flatten().all(|v| v.is_finite());
        match *self {
            Fit::Xyz { left, top, zoom } => all(&[left, top, zoom]),
            Fit::FitH { top } | Fit::FitBH { top } => all(&[top]),
            Fit::FitV { left } | Fit::FitBV { left } => all(&[left]),
            Fit::FitR {
                left,
                bottom,
                right,
                top,
            } => [left, bottom, right, top].iter().all(|v| v.is_finite()),
            Fit::Fit | Fit::FitB => true,
        }
    }
}

/// Where an outline item goes: a page of the document and how to show it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Destination {
    /// Zero-based page index.
    pub page: u32,
    /// How to show the page.
    pub fit: Fit,
}

/// One item of the document outline (the bookmarks).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OutlineEntry {
    /// Names the item in later requests (`parent`, `after`). Only meaningful for this document.
    pub id: u32,
    /// The title as plain text, at most [`MAX_OUTLINE_TITLE_BYTES`] bytes; empty if it has none.
    pub title: String,
    /// Where the item goes; `None` when it has no destination the viewer can follow.
    pub destination: Option<Destination>,
    /// Whether the item has children to ask for.
    pub has_children: bool,
    /// Whether the document asks for the children to be shown when it is opened.
    pub open: bool,
    /// How the title is drawn.
    pub style: TitleStyle,
}

/// How an outline item's title is drawn (the item's `/F` flags).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TitleStyle {
    /// Bold.
    pub bold: bool,
    /// Italic.
    pub italic: bool,
}

/// A document password as the user typed it (UTF-8 text).
///
/// The text is overwritten with zeros when the value is dropped, and `Debug` never shows it, so a
/// log line that formats a request cannot leak it. The framing layer wipes the buffers it
/// encodes into and decodes from; copies the operating system keeps (pipe buffers, swap) are out
/// of reach.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Password(String);

impl Password {
    /// Wraps `text`. Whether it is acceptable on the wire is [`Validate`]'s business, so that a
    /// message from the peer is held to the same rules as one built here.
    #[must_use]
    pub fn new(text: impl Into<String>) -> Self {
        Self(text.into())
    }

    /// The password text. Do not log it, and do not keep copies longer than the call needs.
    #[must_use]
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Password {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Password(<redacted>)")
    }
}

impl Drop for Password {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// How urgently a tile is wanted. The engine renders queued tiles in this order (first-in,
/// first-out within one priority), so the UI can keep prefetching cheap to abandon.
///
/// The variant order is part of the wire format.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Priority {
    /// On screen now.
    Visible,
    /// Near the viewport; likely to be needed soon.
    Prefetch,
    /// A page thumbnail.
    Thumbnail,
}

/// A pixel rectangle of the page at the requested scale, top-left origin.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileRect {
    /// Left edge in device pixels.
    pub x: u32,
    /// Top edge in device pixels.
    pub y: u32,
    /// Width in device pixels.
    pub width: u32,
    /// Height in device pixels.
    pub height: u32,
}

/// Broad class of an [`Response::Error`], for the UI to choose a reaction.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum ErrorKind {
    /// The peer's protocol version is not ours. Fatal.
    VersionMismatch,
    /// The request was well-formed on the wire but not acceptable.
    InvalidRequest,
    /// The document could not be opened.
    OpenFailed,
    /// A tile could not be rendered.
    RenderFailed,
    /// A bug or resource failure inside the engine.
    Internal,
    /// The document is encrypted and needs a password; none was sent. Answers `Open`, which may
    /// be sent again with a password.
    PasswordRequired,
    /// The password sent with `Open` does not open the document. `Open` may be sent again.
    WrongPassword,
    /// The outline or the page labels could not be read (damaged, or past a limit). Answers a
    /// navigation request; the document itself stays usable.
    ReadFailed,
}

/// Messages from the UI to the engine.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Request {
    /// First message. Variant 0 and this shape are frozen across versions.
    Hello {
        /// The sender's [`PROTOCOL_VERSION`].
        protocol_version: u32,
    },
    /// Open the document behind a handle passed at spawn time. If the engine refuses with
    /// [`ErrorKind::PasswordRequired`] or [`ErrorKind::WrongPassword`] the document stays
    /// unopened and the UI may send `Open` again with a password.
    Open {
        /// Opaque token naming the file handle the engine was given.
        handle_token: u64,
        /// The password to try, if the user gave one. Documents that need none ignore it.
        password: Option<Password>,
    },
    /// Render one tile into a shared-memory slot.
    RenderTile {
        /// Correlation id.
        req_id: RequestId,
        /// Zero-based page index.
        page: u32,
        /// Device pixels per point, in (0, [`MAX_TILE_SCALE`]].
        scale: f32,
        /// The part of the scaled page to render.
        rect: TileRect,
        /// Slot the engine writes the pixels into.
        slot: SlotId,
        /// How urgently the tile is wanted; it decides the order of queued tiles.
        priority: Priority,
    },
    /// Drop queued work for `req_id`; work already running finishes but its
    /// result is discarded. A cancelled request gets no answer at all, and the engine may still
    /// write its slot until the next `TileReady` it sends (rendering is one tile at a time).
    Cancel {
        /// The request to cancel.
        req_id: RequestId,
    },
    /// Close the document (and let the engine exit).
    Close,
    /// One page of one level of the outline. Answered with `Outline`. Navigation requests are
    /// read by a thread of their own, so a slow outline never delays a tile; `Cancel` does not
    /// apply to them.
    GetOutline {
        /// Correlation id.
        req_id: RequestId,
        /// The item whose children are wanted; `None` for the top level.
        parent: Option<u32>,
        /// Continue after this item of the level (the last one received); `None` starts the level.
        after: Option<u32>,
        /// How many items of this level the caller already has. It bounds a level whose links
        /// go round in a circle.
        already: u32,
        /// Most items wanted, in `1..=`[`MAX_OUTLINE_ITEMS_PER_MESSAGE`].
        limit: u32,
    },
    /// The outline items that lead to the section `page` is in, to expand the tree to it.
    /// Answered with `OutlinePath`.
    GetOutlinePath {
        /// Correlation id.
        req_id: RequestId,
        /// Zero-based page index.
        page: u32,
    },
    /// The labels of a run of pages. Answered with `PageLabels`.
    GetPageLabels {
        /// Correlation id.
        req_id: RequestId,
        /// Zero-based index of the first page.
        first: u32,
        /// How many pages, in `1..=`[`MAX_LABELS_PER_MESSAGE`].
        count: u32,
    },
    /// The page that a label names ("iv", "A-3"). Answered with `PageFound`.
    FindPageLabel {
        /// Correlation id.
        req_id: RequestId,
        /// The label as typed, at most [`MAX_LABEL_BYTES`] bytes.
        text: String,
    },
}

/// Messages from the engine to the UI.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Response {
    /// First message. Variant 0 and this shape are frozen across versions.
    Hello {
        /// The sender's [`PROTOCOL_VERSION`].
        protocol_version: u32,
    },
    /// The document is open.
    Opened {
        /// Total number of pages.
        page_count: u32,
        /// Sizes of the first pages (at most [`MAX_PAGE_SIZES_PER_MESSAGE`]
        /// and never more than `page_count`); the rest arrive in later chunks.
        page_sizes: Vec<PageSize>,
        /// Why the document needed repair (damaged xref, or `cos` and PDFium disagreed on the
        /// pages); empty for a document that needed none. At most [`MAX_REPAIRS`] entries.
        repairs: Vec<Repair>,
    },
    /// A tile is in its slot and may be read.
    TileReady {
        /// The request it answers.
        req_id: RequestId,
        /// The slot that now holds the pixels.
        slot: SlotId,
    },
    /// A request failed, or the connection did.
    Error {
        /// The failed request, if the error belongs to one.
        req_id: Option<RequestId>,
        /// Broad class of failure.
        kind: ErrorKind,
        /// Human-readable detail, at most [`MAX_ERROR_MESSAGE_BYTES`] bytes.
        message: String,
    },
    /// The answer to `GetOutline`.
    Outline {
        /// The request it answers.
        req_id: RequestId,
        /// The items in order, at most [`MAX_OUTLINE_ITEMS_PER_MESSAGE`]; empty when the document
        /// has no outline or the parent has no children.
        items: Vec<OutlineEntry>,
        /// The level goes on after the last item: ask again with `after` set to it.
        more: bool,
    },
    /// The answer to `GetOutlinePath`.
    OutlinePath {
        /// The request it answers.
        req_id: RequestId,
        /// Ids from a top-level item down to the item that starts the section, at most
        /// [`MAX_OUTLINE_PATH`]; empty when no item starts at or before the page.
        path: Vec<u32>,
    },
    /// The answer to `GetPageLabels`.
    PageLabels {
        /// The request it answers.
        req_id: RequestId,
        /// Zero-based index of the first page labelled.
        first: u32,
        /// Whether the document defines page labels. If not, `labels` hold the page numbers.
        defined: bool,
        /// One label per page from `first`, up to the last page. A page without a label gets its
        /// number (counting from 1). At most [`MAX_LABELS_PER_MESSAGE`].
        labels: Vec<String>,
    },
    /// The answer to `FindPageLabel`.
    PageFound {
        /// The request it answers.
        req_id: RequestId,
        /// The zero-based page, or `None` if no page has that label.
        page: Option<u32>,
    },
}

/// Rules a decoded message must satisfy beyond being well-formed `postcard`.
pub trait Validate {
    /// Checks the message; the framing layer calls it on every message read
    /// and written.
    ///
    /// # Errors
    ///
    /// [`Error::Invalid`] naming the rule that was broken.
    fn validate(&self) -> Result<(), Error>;
}

/// Refuses a peer whose `Hello` carries another protocol version.
///
/// # Errors
///
/// [`Error::VersionMismatch`] when `theirs` is not [`PROTOCOL_VERSION`].
pub fn check_version(theirs: u32) -> Result<(), Error> {
    if theirs == PROTOCOL_VERSION {
        Ok(())
    } else {
        Err(Error::VersionMismatch {
            ours: PROTOCOL_VERSION,
            theirs,
        })
    }
}

impl Validate for Request {
    fn validate(&self) -> Result<(), Error> {
        match self {
            Request::RenderTile { scale, rect, .. } => {
                // `!(a > b)` style comparisons also reject NaN.
                if !(scale.is_finite() && *scale > 0.0 && *scale <= MAX_TILE_SCALE) {
                    return Err(Error::Invalid("tile scale must be in (0, 64]"));
                }
                if rect.width == 0
                    || rect.height == 0
                    || rect.width > MAX_TILE_SIDE
                    || rect.height > MAX_TILE_SIDE
                {
                    return Err(Error::Invalid("tile sides must be 1..=4096"));
                }
                if u64::from(rect.width) * u64::from(rect.height) > MAX_TILE_AREA {
                    return Err(Error::Invalid("tile area exceeds 2^24 pixels"));
                }
                if rect.x > MAX_TILE_ORIGIN || rect.y > MAX_TILE_ORIGIN {
                    return Err(Error::Invalid("tile origin exceeds 2^24 pixels"));
                }
                Ok(())
            }
            Request::Open {
                password: Some(password),
                ..
            } => {
                if password.expose().len() > MAX_PASSWORD_BYTES {
                    return Err(Error::Invalid("password too long"));
                }
                // PDFium takes the password as a C string.
                if password.expose().contains('\0') {
                    return Err(Error::Invalid("password contains a NUL character"));
                }
                Ok(())
            }
            Request::GetOutline { limit, .. } => {
                let wanted = usize::try_from(*limit).unwrap_or(usize::MAX);
                if wanted == 0 || wanted > MAX_OUTLINE_ITEMS_PER_MESSAGE {
                    return Err(Error::Invalid("outline limit must be 1..=128"));
                }
                Ok(())
            }
            Request::GetPageLabels { count, .. } => {
                let wanted = usize::try_from(*count).unwrap_or(usize::MAX);
                if wanted == 0 || wanted > MAX_LABELS_PER_MESSAGE {
                    return Err(Error::Invalid("label count must be 1..=1024"));
                }
                Ok(())
            }
            Request::FindPageLabel { text, .. } => {
                if text.len() > MAX_LABEL_BYTES {
                    return Err(Error::Invalid("page label too long"));
                }
                Ok(())
            }
            Request::Hello { .. }
            | Request::Open { .. }
            | Request::Cancel { .. }
            | Request::Close
            | Request::GetOutlinePath { .. } => Ok(()),
        }
    }
}

impl Validate for Response {
    fn validate(&self) -> Result<(), Error> {
        match self {
            Response::Opened {
                page_count,
                page_sizes,
                repairs,
            } => {
                if repairs.len() > MAX_REPAIRS {
                    return Err(Error::Invalid("too many repairs in one message"));
                }
                if repairs.iter().any(|r| {
                    r.code.len() > MAX_REPAIR_CODE_BYTES
                        || r.message.len() > MAX_REPAIR_MESSAGE_BYTES
                }) {
                    return Err(Error::Invalid("repair text too long"));
                }
                if page_sizes.len() > MAX_PAGE_SIZES_PER_MESSAGE {
                    return Err(Error::Invalid("too many page sizes in one message"));
                }
                if u64::try_from(page_sizes.len()).map_or(true, |n| n > u64::from(*page_count)) {
                    return Err(Error::Invalid("more page sizes than pages"));
                }
                let sane = |v: f32| v.is_finite() && v > 0.0;
                if !page_sizes.iter().all(|s| sane(s.width) && sane(s.height)) {
                    return Err(Error::Invalid("page size must be finite and positive"));
                }
                Ok(())
            }
            Response::Error { message, .. } => {
                if message.len() > MAX_ERROR_MESSAGE_BYTES {
                    return Err(Error::Invalid("error message too long"));
                }
                Ok(())
            }
            Response::Outline { items, .. } => {
                if items.len() > MAX_OUTLINE_ITEMS_PER_MESSAGE {
                    return Err(Error::Invalid("too many outline items in one message"));
                }
                if items
                    .iter()
                    .any(|i| i.title.len() > MAX_OUTLINE_TITLE_BYTES)
                {
                    return Err(Error::Invalid("outline title too long"));
                }
                if items
                    .iter()
                    .filter_map(|i| i.destination)
                    .any(|d| !d.fit.is_finite())
                {
                    return Err(Error::Invalid("destination numbers must be finite"));
                }
                Ok(())
            }
            Response::OutlinePath { path, .. } => {
                if path.len() > MAX_OUTLINE_PATH {
                    return Err(Error::Invalid("outline path too long"));
                }
                Ok(())
            }
            Response::PageLabels { labels, .. } => {
                if labels.len() > MAX_LABELS_PER_MESSAGE {
                    return Err(Error::Invalid("too many labels in one message"));
                }
                if labels.iter().any(|l| l.len() > MAX_LABEL_BYTES) {
                    return Err(Error::Invalid("page label too long"));
                }
                Ok(())
            }
            Response::Hello { .. } | Response::TileReady { .. } | Response::PageFound { .. } => {
                Ok(())
            }
        }
    }
}
