//! The renderer: one thread that owns PDFium, and the handles that talk to it.
//!
//! PDFium is not thread-safe and [`Pdfium`] is not `Send`, so it is created on the renderer
//! thread and never leaves it. Everything else sends it commands over a channel and blocks for the
//! answer; many threads may do so at once and are served one at a time, in arrival order.
//!
//! Tiles are rendered into a buffer owned by the renderer thread and copied into the caller's
//! buffer afterwards. That costs one memcpy of the tile, and it keeps this module free of
//! `unsafe`: the caller's `&mut [u8]` never crosses a thread boundary.

use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};
use std::thread::{self, JoinHandle};

use crate::ffi::{self, DocumentBytes, Matrix, OpenDocument};
use crate::{Error, Pdfium};

/// Longest side of a tile in pixels.
pub const MAX_TILE_SIDE: u32 = 4096;
/// Most pixels in one tile (the renderer allocates four bytes for each).
pub const MAX_TILE_PIXELS: u32 = 1 << 24;
/// Largest scale (device pixels per point).
pub const MAX_SCALE: f32 = 64.0;
/// Largest tile origin. Below 2^24, so the translation is exact in an `f32`.
const MAX_ORIGIN: u32 = 1 << 24;

/// A tile's position and size in device pixels, with the page already scaled and its top-left
/// corner at `(0, 0)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileRect {
    /// Left edge.
    pub x: u32,
    /// Top edge (device y grows downwards).
    pub y: u32,
    /// Width in pixels, `1..=`[`MAX_TILE_SIDE`].
    pub width: u32,
    /// Height in pixels, `1..=`[`MAX_TILE_SIDE`].
    pub height: u32,
}

/// What to render: one rectangle of one page at one scale.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TileRequest {
    /// Page index, zero-based.
    pub page: usize,
    /// Device pixels per point: finite, `> 0` and at most [`MAX_SCALE`]. 1.0 is 72 dpi.
    pub scale: f32,
    /// The part of the scaled page to render. It may extend past the page, where the tile is
    /// plain white.
    pub rect: TileRect,
}

impl TileRequest {
    /// Checks the request and the caller's buffer, which must hold `rect.height` rows of
    /// `stride` bytes (the last row only needs `rect.width * 4`).
    fn validate(&self, buffer_len: usize, stride: usize) -> Result<(), Error> {
        let TileRect {
            x,
            y,
            width,
            height,
        } = self.rect;
        if !(self.scale.is_finite() && self.scale > 0.0 && self.scale <= MAX_SCALE) {
            return Err(Error::InvalidRequest("scale must be in (0, 64]"));
        }
        if width == 0 || height == 0 || width > MAX_TILE_SIDE || height > MAX_TILE_SIDE {
            return Err(Error::InvalidRequest("tile sides must be 1..=4096 pixels"));
        }
        if u64::from(width) * u64::from(height) > u64::from(MAX_TILE_PIXELS) {
            return Err(Error::InvalidRequest("tile has too many pixels"));
        }
        if x > MAX_ORIGIN || y > MAX_ORIGIN {
            return Err(Error::InvalidRequest(
                "tile origin is too far from the page",
            ));
        }
        // Both fit in `usize`: the sides are capped above.
        let row = width as usize * 4;
        let rows = height as usize;
        if stride < row {
            return Err(Error::InvalidRequest(
                "stride is shorter than a row of pixels",
            ));
        }
        let needed = (rows - 1)
            .checked_mul(stride)
            .and_then(|n| n.checked_add(row))
            .ok_or(Error::InvalidRequest("buffer size overflows"))?;
        if buffer_len < needed {
            return Err(Error::InvalidRequest("buffer is too small for the tile"));
        }
        Ok(())
    }

    /// Page space to tile space: PDFium's own display transform (1 pt = 1 px, y flipped) is
    /// applied first, so this is a scale about the page's top-left corner and a shift.
    // The origin is checked to be at most 2^24, which an `f32` holds exactly.
    #[allow(clippy::cast_precision_loss)]
    fn matrix(&self) -> Matrix {
        Matrix {
            a: self.scale,
            b: 0.0,
            c: 0.0,
            d: self.scale,
            e: -(self.rect.x as f32),
            f: -(self.rect.y as f32),
        }
    }
}

enum Command {
    Open {
        bytes: DocumentBytes,
        reply: Sender<Result<(u64, usize), Error>>,
    },
    PageSize {
        doc: u64,
        page: usize,
        reply: Sender<Result<(f32, f32), Error>>,
    },
    Render {
        doc: u64,
        request: TileRequest,
        reply: Sender<Result<Vec<u8>, Error>>,
    },
    Close {
        doc: u64,
    },
    Shutdown,
}

/// Owns the PDFium thread. Share it by reference between threads.
///
/// Dropping it stops the thread and closes every document still open. Handles that outlive it
/// answer [`Error::Closed`].
pub struct Renderer {
    commands: Sender<Command>,
    thread: Option<JoinHandle<()>>,
}

impl Renderer {
    /// Starts the renderer thread, which loads the PDFium library at `library`
    /// (see [`Pdfium::locate`]) and initialises it.
    ///
    /// There is one PDFium per process, so a second `Renderer` (or a [`Pdfium`]) is
    /// [`Error::AlreadyLoaded`] until the first is dropped.
    ///
    /// # Errors
    ///
    /// Whatever [`Pdfium::load`] returns, or [`Error::Spawn`].
    pub fn start(library: impl AsRef<Path>) -> Result<Self, Error> {
        let library = library.as_ref().to_owned();
        let (commands, inbox) = mpsc::channel();
        let (ready, started) = mpsc::sync_channel(1);
        let thread = thread::Builder::new()
            .name("vellora-pdfium".into())
            .spawn(move || match Pdfium::load(library) {
                Ok(pdfium) => {
                    // The caller is waiting in `start`; if it is gone, so is the reason to run.
                    if ready.send(Ok(())).is_ok() {
                        serve(&pdfium, &inbox);
                    }
                }
                Err(error) => {
                    let _ = ready.send(Err(error));
                }
            })
            .map_err(Error::Spawn)?;
        let renderer = Self {
            commands,
            thread: Some(thread),
        };
        // On failure `renderer` is dropped here, which joins the thread that has already ended.
        started.recv().map_err(|_| Error::Closed)??;
        Ok(renderer)
    }

    /// Opens a document revision. `bytes` is kept alive until the returned handle is dropped,
    /// because PDFium reads it lazily. It must always yield the same
    /// slice; a memory-mapped file works as well as a `Vec`.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] if PDFium cannot open the document (including a wrong or missing
    /// password: encrypted documents are not supported here yet), [`Error::DocumentTooLarge`],
    /// [`Error::Closed`].
    pub fn open(
        &self,
        bytes: impl AsRef<[u8]> + Send + Sync + 'static,
    ) -> Result<DocHandle, Error> {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Open {
                bytes: Arc::new(bytes),
                reply,
            })
            .map_err(|_| Error::Closed)?;
        let (id, page_count) = answer.recv().map_err(|_| Error::Closed)??;
        Ok(DocHandle {
            id,
            page_count,
            commands: self.commands.clone(),
        })
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        // If the thread is already gone the send fails, and so there is nothing to wait for.
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

/// An open document. Cheap to share by reference between threads; closes the document when
/// dropped.
pub struct DocHandle {
    id: u64,
    page_count: usize,
    commands: Sender<Command>,
}

impl DocHandle {
    /// Number of pages, as PDFium counts them.
    #[must_use]
    pub fn page_count(&self) -> usize {
        self.page_count
    }

    /// Size of page `page` in points, with its rotation applied.
    ///
    /// # Errors
    ///
    /// [`Error::PageOutOfRange`], [`Error::Page`] if PDFium cannot read the page,
    /// [`Error::Closed`].
    pub fn page_size(&self, page: usize) -> Result<(f32, f32), Error> {
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::PageSize {
                doc: self.id,
                page,
                reply,
            })
            .map_err(|_| Error::Closed)?;
        answer.recv().map_err(|_| Error::Closed)?
    }

    /// Renders the tile into `buffer`: 4 bytes per pixel (blue, green, red, unused), rows `stride`
    /// bytes apart, top row first, over an opaque white page. Bytes between the end of a row's
    /// pixels and the next row are not touched. Blocks until the renderer thread has done it.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidRequest`] for a bad scale, tile or buffer (nothing is rendered),
    /// [`Error::PageOutOfRange`], [`Error::Page`], [`Error::Bitmap`], [`Error::Panicked`],
    /// [`Error::Closed`]. The buffer is only written on success.
    pub fn render_tile(
        &self,
        request: TileRequest,
        buffer: &mut [u8],
        stride: usize,
    ) -> Result<(), Error> {
        request.validate(buffer.len(), stride)?;
        let (reply, answer) = mpsc::channel();
        self.commands
            .send(Command::Render {
                doc: self.id,
                request,
                reply,
            })
            .map_err(|_| Error::Closed)?;
        let pixels = answer.recv().map_err(|_| Error::Closed)??;

        // `validate` guarantees `stride >= row` and that the buffer holds every row.
        let row = request.rect.width as usize * 4;
        for (source, target) in pixels.chunks_exact(row).zip(buffer.chunks_mut(stride)) {
            target[..row].copy_from_slice(source);
        }
        Ok(())
    }
}

impl Drop for DocHandle {
    fn drop(&mut self) {
        // The renderer may already be gone, in which case the document is closed already.
        let _ = self.commands.send(Command::Close { doc: self.id });
    }
}

/// Runs a request, turning a panic into an error so that one bad request cannot take the thread
/// down (a panic must never unwind into PDFium's frames either).
fn guarded<T>(work: impl FnOnce() -> Result<T, Error>) -> Result<T, Error> {
    catch_unwind(AssertUnwindSafe(work)).unwrap_or(Err(Error::Panicked))
}

/// The renderer thread's loop.
fn serve(pdfium: &Pdfium, inbox: &Receiver<Command>) {
    let api: &ffi::Api = &pdfium.api;
    let mut documents: HashMap<u64, OpenDocument> = HashMap::new();
    let mut next_id = 0_u64;

    // Replies are best effort: a caller that gave up has dropped its end.
    while let Ok(command) = inbox.recv() {
        match command {
            Command::Open { bytes, reply } => {
                let result = guarded(|| {
                    let document = api.open(bytes)?;
                    let page_count = document.page_count();
                    next_id += 1;
                    documents.insert(next_id, document);
                    Ok((next_id, page_count))
                });
                let _ = reply.send(result);
            }
            Command::PageSize { doc, page, reply } => {
                let result = guarded(|| api.page_size(document(&documents, doc)?, page));
                let _ = reply.send(result);
            }
            Command::Render {
                doc,
                request,
                reply,
            } => {
                let result = guarded(|| {
                    let TileRect { width, height, .. } = request.rect;
                    let mut pixels = vec![0_u8; width as usize * height as usize * 4];
                    api.render(
                        document(&documents, doc)?,
                        request.page,
                        request.matrix(),
                        (width, height),
                        &mut pixels,
                    )?;
                    Ok(pixels)
                });
                let _ = reply.send(result);
            }
            Command::Close { doc } => {
                if let Some(document) = documents.remove(&doc) {
                    api.close(document);
                }
            }
            Command::Shutdown => break,
        }
    }

    // Documents must be closed before the library is destroyed (when `pdfium` is dropped).
    for (_, document) in documents.drain() {
        api.close(document);
    }
}

/// A document the renderer has open. A missing one means the handle was used after its document
/// was closed, which the handle's own `Drop` makes impossible; report it as a closed renderer.
fn document(documents: &HashMap<u64, OpenDocument>, id: u64) -> Result<&OpenDocument, Error> {
    documents.get(&id).ok_or(Error::Closed)
}
