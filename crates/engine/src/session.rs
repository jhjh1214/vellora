//! The request loop: one conversation with the UI over a pair of byte streams.
//!
//! The engine speaks first (`Hello`), reads the UI's `Hello`, and then answers requests one at a
//! time, in order, until the UI says `Close` or goes away. Requests are served synchronously;
//! queueing, priorities and cancellation of queued work arrive with the scheduler (M0 task 18),
//! which is why `Cancel` is accepted and does nothing yet.

use std::fs::File;
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};

use vellora_ipc::{
    ErrorKind, MAX_ERROR_MESSAGE_BYTES, PROTOCOL_VERSION, Request, RequestId, Response, SlotId,
    check_version, read_frame, write_frame,
};
use vellora_render::{self as render, Renderer, TileRect, TileRequest};
use vellora_shm::{HandleToken, TileRegion};

use crate::document::Document;

/// How a conversation ended without an error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Exit {
    /// The UI sent `Close`.
    Closed,
    /// The UI closed the pipe (it exited or crashed). Nothing more can be answered.
    PeerGone,
}

/// Why a conversation ended in failure. The UI has been told, where it was still listening.
#[derive(Debug, thiserror::Error)]
pub enum ServeError {
    /// The pipe failed, or the stream is no longer in step with the protocol.
    #[error(transparent)]
    Ipc(#[from] vellora_ipc::Error),
    /// The UI's first message was not a `Hello`.
    #[error("the first message was not Hello")]
    NoHello,
}

/// One engine process's state: the renderer, the shared tile region and at most one document.
pub struct Engine {
    renderer: Renderer,
    region: TileRegion,
    file: File,
    file_token: HandleToken,
    max_document_bytes: u64,
    document: Option<Document>,
}

impl Engine {
    /// An engine that will open `file` (known to the UI as `file_token`) when asked, and write
    /// tiles into `region`.
    #[must_use]
    pub fn new(
        renderer: Renderer,
        file: File,
        file_token: HandleToken,
        region: TileRegion,
        max_document_bytes: u64,
    ) -> Self {
        Self {
            renderer,
            region,
            file,
            file_token,
            max_document_bytes,
            document: None,
        }
    }

    /// Runs the conversation: handshake, then requests until `Close` or end of input.
    ///
    /// # Errors
    ///
    /// [`ServeError`] if the handshake fails or the stream breaks. Problems with a single request
    /// are not errors here: they are answered with [`Response::Error`] and the loop goes on.
    pub fn serve<R: Read, W: Write>(
        &mut self,
        mut input: R,
        mut output: W,
    ) -> Result<Exit, ServeError> {
        let hello = Response::Hello {
            protocol_version: PROTOCOL_VERSION,
        };
        if !send(&mut output, &hello)? {
            return Ok(Exit::PeerGone);
        }

        match read_frame::<_, Request>(&mut input)? {
            None => return Ok(Exit::PeerGone),
            Some(Request::Hello { protocol_version }) => {
                if let Err(error) = check_version(protocol_version) {
                    let _ = send(
                        &mut output,
                        &error_response(None, ErrorKind::VersionMismatch, &error),
                    );
                    return Err(error.into());
                }
            }
            Some(_) => {
                let _ = send(
                    &mut output,
                    &error_response(
                        None,
                        ErrorKind::InvalidRequest,
                        &"the first message must be Hello",
                    ),
                );
                return Err(ServeError::NoHello);
            }
        }

        loop {
            let request = match read_frame::<_, Request>(&mut input) {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(Exit::PeerGone),
                // The frame was read whole and is bad: say so, and carry on with the next one.
                Err(
                    error @ (vellora_ipc::Error::Invalid(_)
                    | vellora_ipc::Error::Decode(_)
                    | vellora_ipc::Error::TrailingBytes { .. }),
                ) => {
                    if !send(
                        &mut output,
                        &error_response(None, ErrorKind::InvalidRequest, &error),
                    )? {
                        return Ok(Exit::PeerGone);
                    }
                    continue;
                }
                // The stream is broken or out of step: nothing after this point can be trusted.
                Err(error) => {
                    let _ = send(
                        &mut output,
                        &error_response(None, ErrorKind::InvalidRequest, &error),
                    );
                    return Err(error.into());
                }
            };

            let (reply, flow) = self.handle_guarded(&request);
            if let Some(reply) = reply
                && !send(&mut output, &reply)?
            {
                return Ok(Exit::PeerGone);
            }
            if flow == Flow::Stop {
                return Ok(Exit::Closed);
            }
        }
    }

    /// [`handle`](Self::handle), with a panic turned into an `Internal` error: one bad request
    /// must not end the session (`panic = "unwind"` in the release profile exists for this).
    fn handle_guarded(&mut self, request: &Request) -> (Option<Response>, Flow) {
        let req_id = match request {
            Request::RenderTile { req_id, .. } => Some(*req_id),
            _ => None,
        };
        catch_unwind(AssertUnwindSafe(|| self.handle(request))).unwrap_or_else(|_| {
            tracing::error!("a request handler panicked");
            (
                Some(error_response(
                    req_id,
                    ErrorKind::Internal,
                    &"internal error",
                )),
                Flow::Continue,
            )
        })
    }

    fn handle(&mut self, request: &Request) -> (Option<Response>, Flow) {
        match *request {
            Request::Hello { .. } => (
                Some(error_response(
                    None,
                    ErrorKind::InvalidRequest,
                    &"Hello is only valid as the first message",
                )),
                Flow::Continue,
            ),
            Request::Open { handle_token } => (Some(self.open(handle_token)), Flow::Continue),
            Request::RenderTile {
                req_id,
                page,
                scale,
                rect,
                slot,
            } => (
                Some(self.render_tile(req_id, page, scale, rect, slot)),
                Flow::Continue,
            ),
            // Nothing is queued yet: every request has been answered before the next is read.
            Request::Cancel { .. } => (None, Flow::Continue),
            Request::Close => (None, Flow::Stop),
        }
    }

    fn open(&mut self, handle_token: u64) -> Response {
        if self.document.is_some() {
            return error_response(
                None,
                ErrorKind::InvalidRequest,
                &"a document is already open",
            );
        }
        if handle_token != self.file_token.get() {
            return error_response(
                None,
                ErrorKind::InvalidRequest,
                &"the handle token does not name the document this engine was started with",
            );
        }
        match Document::open(&self.renderer, &self.file, self.max_document_bytes) {
            Ok(document) => {
                let response = Response::Opened {
                    page_count: document.page_count,
                    page_sizes: document.page_sizes.clone(),
                    repaired: document.repaired,
                };
                self.document = Some(document);
                response
            }
            // The file stays available, so the UI may try again (for example after a fix).
            Err(error) => error_response(None, ErrorKind::OpenFailed, &error),
        }
    }

    fn render_tile(
        &mut self,
        req_id: RequestId,
        page: u32,
        scale: f32,
        rect: vellora_ipc::TileRect,
        slot: SlotId,
    ) -> Response {
        let invalid = |reason: &dyn std::fmt::Display| {
            error_response(Some(req_id), ErrorKind::InvalidRequest, reason)
        };
        let Some(document) = &self.document else {
            return invalid(&"no document is open");
        };
        if page >= document.page_count {
            return invalid(&format_args!(
                "page {page} is out of range: the document has {} pages",
                document.page_count
            ));
        }
        let slot_bytes = u64::from(self.region.geometry().slot_bytes());
        let row = u64::from(rect.width) * 4;
        if row * u64::from(rect.height) > slot_bytes {
            return invalid(&"the tile does not fit in a slot");
        }

        let request = TileRequest {
            page: page as usize,
            scale,
            rect: TileRect {
                x: rect.x,
                y: rect.y,
                width: rect.width,
                height: rect.height,
            },
        };
        // The tile fits the slot (checked above), and a slot fits the address space.
        let (Ok(stride), Ok(needed)) = (
            usize::try_from(row),
            usize::try_from(row * u64::from(rect.height)),
        ) else {
            return invalid(&"the tile does not fit in a slot");
        };
        let rendered = self.region.write_slot(slot.0, |bytes| {
            let tile = bytes.get_mut(..needed).ok_or(SlotFailure::TooSmall)?;
            document
                .handle
                .render_tile(request, tile, stride)
                .map_err(SlotFailure::Render)
        });
        match rendered {
            Ok(()) => Response::TileReady { req_id, slot },
            Err(SlotFailure::Shm(error)) => invalid(&error),
            Err(SlotFailure::TooSmall) => invalid(&"the tile does not fit in a slot"),
            Err(SlotFailure::Render(render::Error::InvalidRequest(reason))) => invalid(&reason),
            Err(SlotFailure::Render(error)) => {
                tracing::warn!(page, %error, "tile could not be rendered");
                error_response(Some(req_id), ErrorKind::RenderFailed, &error)
            }
        }
    }
}

/// Whether the loop goes on after a request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Flow {
    Continue,
    Stop,
}

/// What can go wrong between picking a slot and having pixels in it.
enum SlotFailure {
    Shm(vellora_shm::Error),
    /// The slot is shorter than the tile (the geometry check should make this unreachable).
    TooSmall,
    Render(render::Error),
}

impl From<vellora_shm::Error> for SlotFailure {
    fn from(error: vellora_shm::Error) -> Self {
        Self::Shm(error)
    }
}

/// An `Error` response, with the message cut to the protocol's limit on a character boundary.
fn error_response(
    req_id: Option<RequestId>,
    kind: ErrorKind,
    message: &dyn std::fmt::Display,
) -> Response {
    let mut message = message.to_string();
    if message.len() > MAX_ERROR_MESSAGE_BYTES {
        let mut end = MAX_ERROR_MESSAGE_BYTES;
        while !message.is_char_boundary(end) {
            end -= 1;
        }
        message.truncate(end);
    }
    Response::Error {
        req_id,
        kind,
        message,
    }
}

/// Writes one response. `Ok(false)` means the UI has closed its end of the pipe, which is how a
/// normal shutdown can look from here; other write failures are errors.
fn send<W: Write>(output: &mut W, response: &Response) -> Result<bool, vellora_ipc::Error> {
    match write_frame(output, response) {
        Ok(()) => Ok(true),
        Err(vellora_ipc::Error::Io(error)) if error.kind() == io::ErrorKind::BrokenPipe => {
            Ok(false)
        }
        Err(error) => Err(error),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn long_messages_are_cut_on_a_character_boundary() {
        // 'é' is two bytes, so a cut at an odd offset would split it.
        let long = "é".repeat(MAX_ERROR_MESSAGE_BYTES);
        let Response::Error { message, .. } = error_response(None, ErrorKind::Internal, &long)
        else {
            panic!("not an error");
        };
        assert!(message.len() <= MAX_ERROR_MESSAGE_BYTES);
        assert!(message.chars().all(|c| c == 'é'));
        // And what is produced must pass the protocol's own validation.
        let mut sink = Vec::new();
        write_frame(
            &mut sink,
            &Response::Error {
                req_id: None,
                kind: ErrorKind::Internal,
                message,
            },
        )
        .unwrap();
    }

    #[test]
    fn a_closed_pipe_is_a_normal_end_not_an_error() {
        struct Gone;
        impl Write for Gone {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
        }
        let hello = Response::Hello {
            protocol_version: PROTOCOL_VERSION,
        };
        assert!(!send(&mut Gone, &hello).unwrap());
        let mut full = Vec::new();
        assert!(send(&mut full, &hello).unwrap());
    }
}
