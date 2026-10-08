//! The request loop: one conversation with the UI over a pair of byte streams.
//!
//! The engine speaks first (`Hello`), reads the UI's `Hello`, and then serves requests until the
//! UI says `Close` or goes away. Two threads share the conversation:
//!
//! - the **reader** (the calling thread) reads and validates requests, answers `Open` and every
//!   refusal itself, and puts `RenderTile` requests on the priority [`Queue`]; `Cancel` removes
//!   queued work or marks the running tile so that its result is dropped;
//! - the **worker** takes tiles in priority order, renders them one at a time into their shared
//!   memory slots and sends `TileReady`.
//!
//! A [`Watchdog`] thread times the tile being rendered against the [`Deadlines`]. Responses can
//! therefore arrive in a different order than their requests; each carries its `req_id`.

use std::fs::File;
use std::io::{self, Read, Write};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, Mutex};
use std::thread;

use vellora_ipc::{
    ErrorKind, MAX_ERROR_MESSAGE_BYTES, PROTOCOL_VERSION, Password, Priority, Request, RequestId,
    Response, SlotId, check_version, read_frame, write_frame,
};
use vellora_render::{self as render, Renderer, TileRect, TileRequest};
use vellora_shm::{HandleToken, TileRegion};

use crate::document::Document;
use crate::scheduler::{Deadlines, Queue, Watchdog};

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

/// What the hard-deadline hook is told: the request whose tile has been rendering too long.
type HardDeadlineHook = dyn Fn(RequestId) + Send + Sync;

/// One engine process's state: the renderer, the shared tile region and at most one document.
pub struct Engine {
    renderer: Renderer,
    region: TileRegion,
    file: File,
    file_token: HandleToken,
    max_document_bytes: u64,
    document: Option<Arc<Document>>,
    deadlines: Deadlines,
    on_hard_deadline: Box<HardDeadlineHook>,
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
            deadlines: Deadlines::default(),
            on_hard_deadline: Box::new(|req_id| {
                // The library only reports; the executable's `main` replaces this hook with the
                // process abort (a library must not end its host process by default).
                tracing::error!(
                    req_id = req_id.0,
                    "a tile exceeded its hard deadline and nothing is attached to stop it"
                );
            }),
        }
    }

    /// Replaces the per-tile soft and hard deadlines.
    pub fn set_deadlines(&mut self, deadlines: Deadlines) {
        self.deadlines = deadlines;
    }

    /// Replaces what happens when a tile passes its hard deadline. The hook runs once per tile,
    /// on the watchdog thread, while the stuck render is still going on.
    pub fn set_hard_deadline_hook(&mut self, hook: impl Fn(RequestId) + Send + Sync + 'static) {
        self.on_hard_deadline = Box::new(hook);
    }

    /// Runs the conversation: handshake, then requests until `Close` or end of input.
    ///
    /// On the way out the queue is dropped and the tile being rendered (if any) is finished
    /// before this returns; its answer is still sent if the pipe is open.
    ///
    /// # Errors
    ///
    /// [`ServeError`] if the handshake fails or the stream breaks. Problems with a single request
    /// are not errors here: they are answered with [`Response::Error`] and the loop goes on.
    pub fn serve<R: Read, W: Write + Send>(
        &mut self,
        mut input: R,
        output: W,
    ) -> Result<Exit, ServeError> {
        let output = Mutex::new(output);
        let hello = Response::Hello {
            protocol_version: PROTOCOL_VERSION,
        };
        if !send_shared(&output, &hello)? {
            return Ok(Exit::PeerGone);
        }

        match read_frame::<_, Request>(&mut input)? {
            None => return Ok(Exit::PeerGone),
            Some(Request::Hello { protocol_version }) => {
                if let Err(error) = check_version(protocol_version) {
                    let _ = send_shared(
                        &output,
                        &error_response(None, ErrorKind::VersionMismatch, &error),
                    );
                    return Err(error.into());
                }
            }
            Some(_) => {
                let _ = send_shared(
                    &output,
                    &error_response(
                        None,
                        ErrorKind::InvalidRequest,
                        &"the first message must be Hello",
                    ),
                );
                return Err(ServeError::NoHello);
            }
        }

        let Self {
            renderer,
            region,
            file,
            file_token,
            max_document_bytes,
            document,
            deadlines,
            on_hard_deadline,
        } = self;
        let slot_bytes = u64::from(region.geometry().slot_bytes());
        let queue = Queue::new();
        let watchdog = Watchdog::new();
        let mut host = Host {
            renderer,
            file,
            file_token: *file_token,
            max_document_bytes: *max_document_bytes,
            slot_bytes,
            document,
            queue: &queue,
        };

        thread::scope(|scope| {
            scope.spawn(|| worker(&queue, &watchdog, region, &output));
            scope.spawn(|| watchdog.run(*deadlines, &**on_hard_deadline));
            let result = host.read_loop(&mut input, &output);
            // Wakes the worker and the watchdog; the scope then waits for both to finish.
            queue.close();
            watchdog.close();
            result
        })
    }
}

/// The reader's side of a session: everything it needs to answer or queue a request.
struct Host<'a> {
    renderer: &'a Renderer,
    file: &'a File,
    file_token: HandleToken,
    max_document_bytes: u64,
    slot_bytes: u64,
    document: &'a mut Option<Arc<Document>>,
    queue: &'a Queue<TileJob>,
}

/// A validated tile request waiting for the worker.
struct TileJob {
    document: Arc<Document>,
    page: u32,
    scale: f32,
    rect: vellora_ipc::TileRect,
    slot: SlotId,
}

impl Host<'_> {
    fn read_loop<R: Read, W: Write>(
        &mut self,
        input: &mut R,
        output: &Mutex<W>,
    ) -> Result<Exit, ServeError> {
        loop {
            let request = match read_frame::<_, Request>(input) {
                Ok(Some(request)) => request,
                Ok(None) => return Ok(Exit::PeerGone),
                // The frame was read whole and is bad: say so, and carry on with the next one.
                Err(
                    error @ (vellora_ipc::Error::Invalid(_)
                    | vellora_ipc::Error::Decode(_)
                    | vellora_ipc::Error::TrailingBytes { .. }),
                ) => {
                    if !send_shared(
                        output,
                        &error_response(None, ErrorKind::InvalidRequest, &error),
                    )? {
                        return Ok(Exit::PeerGone);
                    }
                    continue;
                }
                // The stream is broken or out of step: nothing after this point can be trusted.
                Err(error) => {
                    let _ = send_shared(
                        output,
                        &error_response(None, ErrorKind::InvalidRequest, &error),
                    );
                    return Err(error.into());
                }
            };

            let (reply, flow) = self.handle_guarded(&request);
            if let Some(reply) = reply
                && !send_shared(output, &reply)?
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
            Request::Open {
                handle_token,
                ref password,
            } => (
                Some(self.open(handle_token, password.as_ref())),
                Flow::Continue,
            ),
            Request::RenderTile {
                req_id,
                page,
                scale,
                rect,
                slot,
                priority,
            } => (
                self.enqueue(req_id, page, scale, rect, slot, priority),
                Flow::Continue,
            ),
            // No answer either way: what was queued is dropped, what is rendering is not sent.
            Request::Cancel { req_id } => {
                let found = self.queue.cancel(req_id);
                tracing::debug!(req_id = req_id.0, ?found, "cancel");
                (None, Flow::Continue)
            }
            Request::Close => (None, Flow::Stop),
        }
    }

    fn open(&mut self, handle_token: u64, password: Option<&Password>) -> Response {
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
        match Document::open(self.renderer, self.file, self.max_document_bytes, password) {
            Ok(document) => {
                let response = Response::Opened {
                    page_count: document.page_count,
                    page_sizes: document.page_sizes.clone(),
                    repairs: document.repairs.clone(),
                };
                *self.document = Some(Arc::new(document));
                response
            }
            // The file stays available, so the UI may try again: with a password, or after a fix.
            Err(error) => error_response(None, error.kind(), &error),
        }
    }

    /// Checks a tile request against the document and the slot size and queues it. `Some` is the
    /// refusal to send; a queued tile is answered later by the worker.
    fn enqueue(
        &self,
        req_id: RequestId,
        page: u32,
        scale: f32,
        rect: vellora_ipc::TileRect,
        slot: SlotId,
        priority: Priority,
    ) -> Option<Response> {
        let invalid = |reason: &dyn std::fmt::Display| {
            Some(error_response(
                Some(req_id),
                ErrorKind::InvalidRequest,
                reason,
            ))
        };
        let Some(document) = self.document.as_ref() else {
            return invalid(&"no document is open");
        };
        if page >= document.page_count {
            return invalid(&format_args!(
                "page {page} is out of range: the document has {} pages",
                document.page_count
            ));
        }
        let row = u64::from(rect.width) * 4;
        if row * u64::from(rect.height) > self.slot_bytes {
            return invalid(&"the tile does not fit in a slot");
        }
        self.queue.push(
            priority,
            req_id,
            TileJob {
                document: Arc::clone(document),
                page,
                scale,
                rect,
                slot,
            },
        );
        None
    }
}

/// The render worker: takes tiles in priority order until the queue is closed.
fn worker<W: Write>(
    queue: &Queue<TileJob>,
    watchdog: &Watchdog,
    region: &mut TileRegion,
    output: &Mutex<W>,
) {
    while let Some((req_id, job)) = queue.next() {
        watchdog.begin(req_id);
        let response = catch_unwind(AssertUnwindSafe(|| render_tile(region, req_id, &job)))
            .unwrap_or_else(|_| {
                tracing::error!("a tile render panicked");
                error_response(Some(req_id), ErrorKind::Internal, &"internal error")
            });
        watchdog.end();
        // A request cancelled while it rendered gets no answer, not even an error.
        if !queue.finish() {
            continue;
        }
        match send_shared(output, &response) {
            Ok(true) => {}
            // The UI is gone; the reader sees the closed pipe and ends the session.
            Ok(false) => queue.close(),
            Err(error) => {
                tracing::error!(%error, "a response could not be sent");
                queue.close();
            }
        }
    }
}

fn render_tile(region: &mut TileRegion, req_id: RequestId, job: &TileJob) -> Response {
    let invalid = |reason: &dyn std::fmt::Display| {
        error_response(Some(req_id), ErrorKind::InvalidRequest, reason)
    };
    let rect = job.rect;
    let request = TileRequest {
        page: job.page as usize,
        scale: job.scale,
        rect: TileRect {
            x: rect.x,
            y: rect.y,
            width: rect.width,
            height: rect.height,
        },
    };
    // The tile fits the slot (checked when it was queued), and a slot fits the address space.
    let row = u64::from(rect.width) * 4;
    let (Ok(stride), Ok(needed)) = (
        usize::try_from(row),
        usize::try_from(row * u64::from(rect.height)),
    ) else {
        return invalid(&"the tile does not fit in a slot");
    };
    let rendered = region.write_slot(job.slot.0, |bytes| {
        let tile = bytes.get_mut(..needed).ok_or(SlotFailure::TooSmall)?;
        job.document
            .handle
            .render_tile(request, tile, stride)
            .map_err(SlotFailure::Render)
    });
    match rendered {
        Ok(()) => Response::TileReady {
            req_id,
            slot: job.slot,
        },
        Err(SlotFailure::Shm(error)) => invalid(&error),
        Err(SlotFailure::TooSmall) => invalid(&"the tile does not fit in a slot"),
        Err(SlotFailure::Render(render::Error::InvalidRequest(reason))) => invalid(&reason),
        Err(SlotFailure::Render(error)) => {
            tracing::warn!(page = job.page, %error, "tile could not be rendered");
            error_response(Some(req_id), ErrorKind::RenderFailed, &error)
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

/// [`send`] on the output the reader and the worker share, so that frames never interleave.
fn send_shared<W: Write>(
    output: &Mutex<W>,
    response: &Response,
) -> Result<bool, vellora_ipc::Error> {
    let mut output = output
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    send(&mut *output, response)
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
