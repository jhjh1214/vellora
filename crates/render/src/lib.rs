//! # vellora-render — safe PDFium wrapper (read-only)
//!
//! **Responsibility:** the only place Vellora talks to PDFium:
//!
//! - load a document revision from bytes we provide (`FPDF_FILEACCESS`)
//! - rasterise tiles into caller-provided buffers (shared memory)
//! - report page sizes and, later, glyph geometry, annotation and form rendering
//!
//! **Boundaries:**
//!
//! - **Read-only.** Never call PDFium save or edit APIs (`FPDF_SaveAsCopy`,
//!   `FPDFPage_GenerateContent`, …). `vellora-cos` is the only writer
//!   (ADR-0002). The binding table in `ffi.rs` declares no such function.
//! - PDFium is not thread-safe. One PDFium instance per process; all calls go
//!   through a single thread owned by this crate.
//! - Linked only into `vellora-engine`, never into the UI or the CLI process.
//! - `unsafe` is confined to the FFI module, and every block has a `// SAFETY:`
//!   comment.
//!
//! **Loading:** PDFium is a shared library resolved at run time (ADR-0014): the
//! engine ships it next to its executable, and `cargo xtask pdfium fetch` puts the
//! pinned build under `third_party/pdfium/<platform>/` for development. A missing
//! library is a typed error, never a link failure of the workspace.
//!
//! **Rendering:** [`Renderer`] owns the PDFium thread. Documents are opened from shared bytes
//! ([`Renderer::open`]) and rendered tile by tile into caller buffers
//! ([`DocHandle::render_tile`]); requests from any number of threads are serialised on it.
//!
//! **Status:** bindings and loader (M0 task 14), renderer thread and tile API (task 15).

// The FFI module is the one place `unsafe` is allowed (see the crate lints).
#[allow(unsafe_code)]
mod ffi;

use std::env;
use std::ffi::{OsString, c_ulong};
use std::path::{Path, PathBuf};

mod renderer;

pub use renderer::{
    DocHandle, MAX_SCALE, MAX_TILE_PIXELS, MAX_TILE_SIDE, Renderer, TileRect, TileRequest,
};

/// Environment variable that overrides where [`Pdfium::locate`] looks for the library.
pub const LIBRARY_ENV: &str = "VELLORA_PDFIUM_LIB";

/// The platform's file name for the PDFium shared library.
#[must_use]
pub const fn library_file_name() -> &'static str {
    if cfg!(windows) {
        "pdfium.dll"
    } else if cfg!(target_os = "macos") {
        "libpdfium.dylib"
    } else {
        "libpdfium.so"
    }
}

/// The search behind [`Pdfium::locate`], with its two inputs made explicit.
fn locate_in(override_path: Option<OsString>, exe_dir: Option<&Path>) -> Result<PathBuf, Error> {
    let file = library_file_name();
    // An explicit override is never second-guessed: if it is wrong, say so instead of quietly
    // loading a different library.
    let candidate = match override_path {
        Some(path) => Some(PathBuf::from(path)),
        None => exe_dir.map(|dir| dir.join(file)),
    };
    candidate
        .filter(|path| path.is_file())
        .ok_or(Error::NotFound { file })
}

/// Why a PDFium call failed, from `FPDF_GetLastError`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[non_exhaustive]
pub enum LastError {
    /// `FPDF_ERR_UNKNOWN`.
    #[error("unknown error")]
    Unknown,
    /// `FPDF_ERR_FILE`: file not found or could not be opened.
    #[error("file could not be read")]
    File,
    /// `FPDF_ERR_FORMAT`: not a PDF, or too damaged for PDFium to open.
    #[error("not a PDF or corrupted")]
    Format,
    /// `FPDF_ERR_PASSWORD`: a password is required or was wrong.
    #[error("password required or incorrect")]
    Password,
    /// `FPDF_ERR_SECURITY`: unsupported security scheme.
    #[error("unsupported security scheme")]
    Security,
    /// `FPDF_ERR_PAGE`: page not found or content error.
    #[error("page not found or content error")]
    Page,
    /// A code this wrapper does not know (or `FPDF_ERR_SUCCESS`, which PDFium may leave
    /// behind when the failure did not set one).
    #[error("PDFium error code {0}")]
    Other(u32),
}

impl LastError {
    // `c_ulong` is 32 bits on Windows and 64 bits elsewhere, so the conversion is only a no-op
    // on some targets.
    #[allow(clippy::useless_conversion)]
    pub(crate) fn from_code(code: c_ulong) -> Self {
        match code {
            1 => Self::Unknown,
            2 => Self::File,
            3 => Self::Format,
            4 => Self::Password,
            5 => Self::Security,
            6 => Self::Page,
            other => Self::Other(u32::try_from(other).unwrap_or(u32::MAX)),
        }
    }
}

/// Errors from loading or calling PDFium.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The shared library could not be opened.
    #[error("cannot load the PDFium library {}: {source}", path.display())]
    Load {
        /// The path that was tried.
        path: PathBuf,
        /// The loader's message.
        source: libloading::Error,
    },
    /// The library lacks a function we need (a wrong or truncated build).
    #[error("the PDFium library does not export {name}: {source}")]
    Symbol {
        /// The missing function.
        name: &'static str,
        /// The loader's message.
        source: libloading::Error,
    },
    /// No library file was found by [`Pdfium::locate`].
    #[error("no PDFium library found: set {LIBRARY_ENV} or put {file} next to the executable")]
    NotFound {
        /// The file name that was looked for.
        file: &'static str,
    },
    /// PDFium is already loaded in this process; it keeps global state, so there is one instance.
    #[error("PDFium is already loaded in this process")]
    AlreadyLoaded,
    /// The document is larger than PDFium's custom file access can describe (the length is a C
    /// `unsigned long`, 32 bits on Windows).
    #[error("document of {len} bytes exceeds the {max} bytes PDFium's file access can address")]
    DocumentTooLarge {
        /// Size of the document.
        len: u64,
        /// The largest size PDFium can take on this platform.
        max: u64,
    },
    /// PDFium refused to open the document.
    #[error("PDFium could not open the document: {0}")]
    Open(LastError),
    /// The page index is not in the document.
    #[error("page {page} is out of range: the document has {count} pages")]
    PageOutOfRange {
        /// The requested page (zero-based).
        page: usize,
        /// Pages in the document.
        count: usize,
    },
    /// PDFium could not load or measure a page.
    #[error("PDFium could not load page {page}: {source}")]
    Page {
        /// The page (zero-based).
        page: usize,
        /// PDFium's reason.
        source: LastError,
    },
    /// PDFium could not wrap the pixel buffer in a bitmap.
    #[error("PDFium could not create a bitmap for the tile")]
    Bitmap,
    /// A render request is malformed (scale, tile size, buffer or stride).
    #[error("invalid render request: {0}")]
    InvalidRequest(&'static str),
    /// The renderer thread has shut down (or never started), so the request was not served.
    #[error("the renderer has shut down")]
    Closed,
    /// The renderer thread panicked while serving this request. The request failed; the thread
    /// keeps serving others.
    #[error("the renderer panicked while serving the request")]
    Panicked,
    /// The renderer thread could not be started.
    #[error("cannot start the renderer thread: {0}")]
    Spawn(std::io::Error),
}

/// A loaded and initialised PDFium library.
///
/// There is at most one per process, and it must stay on the thread that created it (it is
/// neither `Send` nor `Sync`). Dropping it destroys the library.
pub struct Pdfium {
    api: ffi::Api,
}

impl Pdfium {
    /// Loads the shared library at `path` and initialises PDFium.
    ///
    /// # Errors
    ///
    /// [`Error::AlreadyLoaded`] if another instance is alive, [`Error::Load`] if the file cannot
    /// be opened, [`Error::Symbol`] if it lacks an entry point we need.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, Error> {
        ffi::Api::load(path.as_ref()).map(|api| Self { api })
    }

    /// Finds the library: `$VELLORA_PDFIUM_LIB` if set, else [`library_file_name`] in the
    /// directory of the running executable (where the engine ships it).
    ///
    /// # Errors
    ///
    /// [`Error::NotFound`] if neither exists.
    pub fn locate() -> Result<PathBuf, Error> {
        let exe_dir = env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        locate_in(env::var_os(LIBRARY_ENV), exe_dir.as_deref())
    }

    /// Opens `pdf` through PDFium's custom file access and returns its page count.
    ///
    /// The bytes are only read while this call runs. This is the narrowest end-to-end use of
    /// the bindings; the renderer proper arrives with task 15.
    ///
    /// # Errors
    ///
    /// [`Error::Open`] if PDFium cannot open the document, [`Error::DocumentTooLarge`] above the
    /// platform's file access limit.
    pub fn page_count(&self, pdf: &[u8]) -> Result<usize, Error> {
        self.api.page_count(pdf)
    }
}

#[cfg(test)]
mod renderer_tests;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
