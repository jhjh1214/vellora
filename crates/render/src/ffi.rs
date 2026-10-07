//! The FFI boundary: hand-written declarations for the narrow slice of PDFium's C API that
//! Vellora uses, and the loader that resolves them from the shared library at run time.
//!
//! This is the only module in the crate where `unsafe` is allowed. Every block has a
//! `// SAFETY:` comment.
//!
//! The declarations mirror `fpdfview.h` of the pinned release (`third_party/pdfium.lock`). Only
//! read-side functions are declared: no save, edit or page-generation entry point exists here
//! (ADR-0002). `FPDF_CALLCONV` is empty in the public headers, so everything is `extern "C"`.

// Until task 15 lands, only the unit tests call most of the table; they check each entry against
// the real library.
#![allow(
    dead_code,
    reason = "bindings are used by the renderer from task 15 on"
)]

use std::ffi::{c_int, c_ulong, c_void};
use std::marker::PhantomData;
use std::mem::ManuallyDrop;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};

use libloading::Library;

use crate::{Error, LastError};

/// `FPDF_DOCUMENT` points at one of these (opaque to us).
#[repr(C)]
pub(crate) struct DocumentT {
    _private: [u8; 0],
}

/// `FPDF_PAGE` points at one of these (opaque to us).
#[repr(C)]
pub(crate) struct PageT {
    _private: [u8; 0],
}

/// `FPDF_BITMAP` points at one of these (opaque to us).
#[repr(C)]
pub(crate) struct BitmapT {
    _private: [u8; 0],
}

pub(crate) type Document = *mut DocumentT;
pub(crate) type Page = *mut PageT;
pub(crate) type Bitmap = *mut BitmapT;

/// `FPDF_GetBlock` callback: copy `size` bytes at `position` into `buf`; non-zero on success.
pub(crate) type GetBlock = unsafe extern "C" fn(*mut c_void, c_ulong, *mut u8, c_ulong) -> c_int;

/// `FPDF_FILEACCESS`.
///
/// `m_FileLen` is a C `unsigned long`: 32 bits on Windows, so PDFium's custom file access cannot
/// describe a file of 4 GiB or more there.
#[repr(C)]
pub(crate) struct FileAccess {
    pub(crate) len: c_ulong,
    pub(crate) get_block: Option<GetBlock>,
    pub(crate) param: *mut c_void,
}

/// `FS_MATRIX`: maps page space to device space, `[a b 0; c d 0; e f 1]`.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct Matrix {
    pub(crate) a: f32,
    pub(crate) b: f32,
    pub(crate) c: f32,
    pub(crate) d: f32,
    pub(crate) e: f32,
    pub(crate) f: f32,
}

/// `FS_RECTF` in device space (y grows downwards), so `top <= bottom` for a clip rectangle.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RectF {
    pub(crate) left: f32,
    pub(crate) top: f32,
    pub(crate) right: f32,
    pub(crate) bottom: f32,
}

/// `FS_SIZEF`.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct SizeF {
    pub(crate) width: f32,
    pub(crate) height: f32,
}

/// `FPDFBitmap_BGRx`: 4 bytes per pixel, blue, green, red, unused.
pub(crate) const BITMAP_BGRX: c_int = 3;

/// `FPDF_ANNOT`: also render annotations that need no user interaction.
pub(crate) const RENDER_ANNOT: c_int = 0x01;

/// Whether a `Pdfium` exists in this process. PDFium keeps global state, and
/// `FPDF_InitLibrary` / `FPDF_DestroyLibrary` must pair up exactly once per process.
static LOADED: AtomicBool = AtomicBool::new(false);

/// Holds the process-wide claim on PDFium; releasing it makes a new `load` possible.
struct Claim;

impl Claim {
    fn take() -> Result<Self, Error> {
        LOADED
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map(|_| Self)
            .map_err(|_| Error::AlreadyLoaded)
    }
}

impl Drop for Claim {
    fn drop(&mut self) {
        LOADED.store(false, Ordering::Release);
    }
}

/// The resolved PDFium entry points plus the library that keeps them valid.
///
/// Not `Send` or `Sync`: PDFium is not thread-safe, and all calls must come from one thread
/// (task 15 gives that thread to a dedicated owner).
pub(crate) struct Api {
    pub(crate) init_library: unsafe extern "C" fn(),
    pub(crate) destroy_library: unsafe extern "C" fn(),
    pub(crate) get_last_error: unsafe extern "C" fn() -> c_ulong,
    pub(crate) load_custom_document:
        unsafe extern "C" fn(*mut FileAccess, *const std::ffi::c_char) -> Document,
    pub(crate) close_document: unsafe extern "C" fn(Document),
    pub(crate) get_page_count: unsafe extern "C" fn(Document) -> c_int,
    pub(crate) load_page: unsafe extern "C" fn(Document, c_int) -> Page,
    pub(crate) close_page: unsafe extern "C" fn(Page),
    pub(crate) get_page_size_by_index_f: unsafe extern "C" fn(Document, c_int, *mut SizeF) -> c_int,
    pub(crate) bitmap_create_ex:
        unsafe extern "C" fn(c_int, c_int, c_int, *mut c_void, c_int) -> Bitmap,
    pub(crate) bitmap_fill_rect:
        unsafe extern "C" fn(Bitmap, c_int, c_int, c_int, c_int, u32) -> c_int,
    pub(crate) bitmap_destroy: unsafe extern "C" fn(Bitmap),
    pub(crate) render_page_bitmap_with_matrix:
        unsafe extern "C" fn(Bitmap, Page, *const Matrix, *const RectF, c_int),
    // Never unloaded: a large C++ library can leave thread-local destructors or atexit hooks
    // behind, and running them after `dlclose` unmapped their code crashes the process. PDFium
    // is destroyed in `Drop`; its code simply stays mapped until the process exits, so a later
    // `load` reuses the same mapping.
    _library: ManuallyDrop<Library>,
    _claim: Claim,
    _not_send: PhantomData<*const ()>,
}

/// Looks up one exported function.
///
/// # Safety
///
/// `T` must be the exact type of the C function called `name`.
unsafe fn symbol<T: Copy>(library: &Library, name: &'static str) -> Result<T, Error> {
    // SAFETY: the caller guarantees that `T` matches the symbol; the copy is only used while
    // `library` is alive (it is stored next to the pointers in `Api`).
    unsafe {
        library
            .get::<T>(name.as_bytes())
            .map(|symbol| *symbol)
            .map_err(|source| Error::Symbol {
                name: name.trim_end_matches('\0'),
                source,
            })
    }
}

impl Api {
    /// Loads the shared library at `path`, resolves every entry point and initialises PDFium.
    pub(crate) fn load(path: &Path) -> Result<Self, Error> {
        let claim = Claim::take()?;
        // SAFETY: loading a library runs its initialisers. The caller names the PDFium build
        // that ships with Vellora (pinned and checksum-verified by `cargo xtask pdfium fetch`).
        let library = unsafe { Library::new(path) }.map_err(|source| Error::Load {
            path: path.to_owned(),
            source,
        })?;

        // SAFETY: for every `symbol` call below, the type is transcribed from the declaration of
        // the function with that name in `fpdfview.h` (see the module docs). If one fails, nothing
        // was initialised, and dropping `library` and `claim` undoes the load.
        let api = unsafe {
            Self {
                init_library: symbol(&library, "FPDF_InitLibrary\0")?,
                destroy_library: symbol(&library, "FPDF_DestroyLibrary\0")?,
                get_last_error: symbol(&library, "FPDF_GetLastError\0")?,
                load_custom_document: symbol(&library, "FPDF_LoadCustomDocument\0")?,
                close_document: symbol(&library, "FPDF_CloseDocument\0")?,
                get_page_count: symbol(&library, "FPDF_GetPageCount\0")?,
                load_page: symbol(&library, "FPDF_LoadPage\0")?,
                close_page: symbol(&library, "FPDF_ClosePage\0")?,
                get_page_size_by_index_f: symbol(&library, "FPDF_GetPageSizeByIndexF\0")?,
                bitmap_create_ex: symbol(&library, "FPDFBitmap_CreateEx\0")?,
                bitmap_fill_rect: symbol(&library, "FPDFBitmap_FillRect\0")?,
                bitmap_destroy: symbol(&library, "FPDFBitmap_Destroy\0")?,
                render_page_bitmap_with_matrix: symbol(
                    &library,
                    "FPDF_RenderPageBitmapWithMatrix\0",
                )?,
                _library: ManuallyDrop::new(library),
                _claim: claim,
                _not_send: PhantomData,
            }
        };
        // SAFETY: first and only initialisation in this process (guarded by `LOADED`).
        unsafe { (api.init_library)() };
        Ok(api)
    }

    /// The error code of the last failed PDFium call, as `FPDF_GetLastError` reports it.
    pub(crate) fn last_error(&self) -> LastError {
        // SAFETY: no arguments; only meaningful right after a call that documents it.
        LastError::from_code(unsafe { (self.get_last_error)() })
    }

    /// Opens `pdf` through `FPDF_FILEACCESS` and returns its page count.
    pub(crate) fn page_count(&self, pdf: &[u8]) -> Result<usize, Error> {
        let len = c_ulong::try_from(pdf.len()).map_err(|_| Error::DocumentTooLarge {
            len: pdf.len() as u64,
            max: u64::from(c_ulong::MAX),
        })?;
        let source: &[u8] = pdf;
        let mut access = FileAccess {
            len,
            get_block: Some(read_block),
            param: ptr::from_ref(&source).cast_mut().cast(),
        };
        // SAFETY: `access` and `source` stay alive until the document is closed below; the
        // callback only reads through `param`, and PDFium copies the `FPDF_FILEACCESS` struct.
        // A null password means none.
        let document = unsafe { (self.load_custom_document)(&raw mut access, ptr::null()) };
        if document.is_null() {
            return Err(Error::Open(self.last_error()));
        }
        // SAFETY: `document` is a live handle returned just above.
        let count = unsafe { (self.get_page_count)(document) };
        // SAFETY: `document` is closed exactly once and not used afterwards.
        unsafe { (self.close_document)(document) };
        usize::try_from(count).map_err(|_| Error::Open(self.last_error()))
    }
}

impl Drop for Api {
    fn drop(&mut self) {
        // SAFETY: `init_library` ran in `load` and this is the only destroy; the library (the code
        // being called) is never unloaded.
        unsafe { (self.destroy_library)() };
    }
}

/// `FPDF_FILEACCESS::m_GetBlock` over an in-memory document. `param` is a `*const &[u8]`.
///
/// Never unwinds into C: a panic would be undefined behaviour, so it is caught and reported
/// as a read error.
unsafe extern "C" fn read_block(
    param: *mut c_void,
    position: c_ulong,
    buf: *mut u8,
    size: c_ulong,
) -> c_int {
    let copied = catch_unwind(AssertUnwindSafe(|| {
        if param.is_null() || buf.is_null() {
            return false;
        }
        // SAFETY: `param` is the `&&[u8]` that `Api::page_count` put in `FileAccess::param`,
        // alive for as long as the document is open.
        let data: &[u8] = unsafe { *param.cast::<&[u8]>() };
        let (Ok(start), Ok(len)) = (usize::try_from(position), usize::try_from(size)) else {
            return false;
        };
        let Some(chunk) = start.checked_add(len).and_then(|end| data.get(start..end)) else {
            return false;
        };
        // SAFETY: PDFium passes a writable buffer of `size` bytes that does not overlap the
        // document bytes (`chunk.len() == len == size`).
        unsafe { ptr::copy_nonoverlapping(chunk.as_ptr(), buf, len) };
        true
    }));
    c_int::from(copied.unwrap_or(false))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{SAMPLE_PAGE_SIZES, sample_pdf, with_pdfium};

    const SENTINEL: u8 = 0xAB;
    const BLACK: [u8; 3] = [0, 0, 0];
    const WHITE: [u8; 3] = [255, 255, 255];

    const IDENTITY: Matrix = Matrix {
        a: 1.0,
        b: 0.0,
        c: 0.0,
        d: 1.0,
        e: 0.0,
        f: 0.0,
    };

    fn whole(width: i32, height: i32) -> RectF {
        RectF {
            left: 0.0,
            top: 0.0,
            right: f32::from(i16::try_from(width).unwrap()),
            bottom: f32::from(i16::try_from(height).unwrap()),
        }
    }

    /// Renders `page` into a `width` x `height` buffer whose rows are `stride` bytes apart,
    /// after filling the pixels with white. Bytes between the end of a row's pixels and the next
    /// row hold `SENTINEL` and must stay untouched.
    fn render(
        api: &Api,
        page: Page,
        (width, height, stride): (i32, i32, i32),
        matrix: Matrix,
        clip: RectF,
    ) -> Vec<u8> {
        let mut buffer = vec![SENTINEL; usize::try_from(stride * height).unwrap()];
        // SAFETY: `buffer` holds `stride * height` bytes, outlives the bitmap (destroyed below)
        // and is not touched while PDFium writes to it.
        let bitmap = unsafe {
            (api.bitmap_create_ex)(
                width,
                height,
                BITMAP_BGRX,
                buffer.as_mut_ptr().cast(),
                stride,
            )
        };
        assert!(!bitmap.is_null());
        // SAFETY: `bitmap` is live; the rectangle is the whole bitmap.
        let filled = unsafe { (api.bitmap_fill_rect)(bitmap, 0, 0, width, height, 0xFFFF_FFFF) };
        assert_eq!(filled, 1);
        // SAFETY: `bitmap` and `page` are live; the matrix is invertible and both pointers
        // reference locals that outlive the call.
        unsafe {
            (api.render_page_bitmap_with_matrix)(
                bitmap,
                page,
                &raw const matrix,
                &raw const clip,
                0,
            );
        }
        // SAFETY: destroyed once; with an external buffer PDFium does not free `buffer`.
        unsafe { (api.bitmap_destroy)(bitmap) };
        buffer
    }

    fn pixel(buffer: &[u8], stride: i32, x: i32, y: i32) -> [u8; 3] {
        let at = usize::try_from(y * stride + x * 4).unwrap();
        [buffer[at], buffer[at + 1], buffer[at + 2]]
    }

    #[test]
    fn sizes_and_rendering_through_the_raw_bindings() {
        let pdf = sample_pdf();
        with_pdfium(|pdfium| {
            let api = &pdfium.api;
            let source: &[u8] = &pdf;
            let mut access = FileAccess {
                len: c_ulong::try_from(pdf.len()).unwrap(),
                get_block: Some(read_block),
                param: ptr::from_ref(&source).cast_mut().cast(),
            };
            // SAFETY: `access` and `source` outlive the document (closed at the end of this
            // closure); null password.
            let document = unsafe { (api.load_custom_document)(&raw mut access, ptr::null()) };
            assert!(!document.is_null());
            // SAFETY: `document` is live.
            assert_eq!(unsafe { (api.get_page_count)(document) }, 3);

            // Page sizes by index, including an index that does not exist.
            for (index, (width, height)) in SAMPLE_PAGE_SIZES.into_iter().enumerate() {
                let mut size = SizeF::default();
                let index = i32::try_from(index).unwrap();
                // SAFETY: `document` is live and `size` is a valid out parameter.
                let ok = unsafe { (api.get_page_size_by_index_f)(document, index, &raw mut size) };
                assert_eq!(ok, 1, "page {index}");
                assert_eq!(size, SizeF { width, height }, "page {index}");
            }
            let mut size = SizeF::default();
            // SAFETY: as above; the index is out of range on purpose.
            let ok = unsafe { (api.get_page_size_by_index_f)(document, 3, &raw mut size) };
            assert_eq!(ok, 0);
            // SAFETY: as above; out of range on purpose, PDFium returns null.
            assert!(unsafe { (api.load_page)(document, 3) }.is_null());

            // SAFETY: `document` is live and index 0 exists.
            let page = unsafe { (api.load_page)(document, 0) };
            assert!(!page.is_null());

            // One pixel per point, rows padded by 16 bytes: the padding must stay untouched.
            let stride = 200 * 4 + 16;
            let buffer = render(api, page, (200, 100, stride), IDENTITY, whole(200, 100));
            assert_eq!(
                pixel(&buffer, stride, 100, 50),
                BLACK,
                "inside the rectangle"
            );
            assert_eq!(pixel(&buffer, stride, 10, 10), WHITE, "page background");
            assert_eq!(pixel(&buffer, stride, 190, 90), WHITE, "page background");
            for row in 0..100 {
                let pad = usize::try_from(row * stride + 200 * 4).unwrap();
                assert!(
                    buffer[pad..pad + 16].iter().all(|&b| b == SENTINEL),
                    "row {row} padding was written"
                );
            }

            // Scale 2 through the matrix, into a bitmap twice the size.
            let double = Matrix {
                a: 2.0,
                d: 2.0,
                ..IDENTITY
            };
            let buffer = render(api, page, (400, 200, 1600), double, whole(400, 200));
            assert_eq!(pixel(&buffer, 1600, 200, 100), BLACK);
            assert_eq!(
                pixel(&buffer, 1600, 20, 20),
                WHITE,
                "rectangle is 40..360 x 40..160"
            );

            // A clip rectangle limits what is drawn: the left half stays white.
            let right_half = RectF {
                left: 100.0,
                ..whole(200, 100)
            };
            let buffer = render(api, page, (200, 100, 800), IDENTITY, right_half);
            assert_eq!(pixel(&buffer, 800, 50, 50), WHITE, "clipped away");
            assert_eq!(pixel(&buffer, 800, 150, 50), BLACK, "inside the clip");

            // A translation moves the page inside the bitmap (device y grows downwards).
            let shifted = Matrix {
                e: 20.0,
                f: 10.0,
                ..IDENTITY
            };
            let buffer = render(api, page, (220, 110, 880), shifted, whole(220, 110));
            assert_eq!(
                pixel(&buffer, 880, 120, 60),
                BLACK,
                "rectangle moved by (20, 10)"
            );
            assert_eq!(
                pixel(&buffer, 880, 5, 5),
                WHITE,
                "uncovered corner keeps the fill"
            );

            // SAFETY: both handles are live and closed exactly once, page before document.
            unsafe {
                (api.close_page)(page);
                (api.close_document)(document);
            }
        });
    }

    #[test]
    fn the_read_callback_rejects_ranges_outside_the_document() {
        let data: &[u8] = b"0123456789";
        let param = ptr::from_ref(&data).cast_mut().cast::<c_void>();
        let mut out = [0u8; 4];
        let read = |position: c_ulong, size: c_ulong, out: &mut [u8; 4]| {
            // SAFETY: `param` points at `data`; `out` is a writable buffer of 4 bytes and `size`
            // never exceeds 4.
            unsafe { read_block(param, position, out.as_mut_ptr(), size) }
        };
        assert_eq!(read(2, 4, &mut out), 1);
        assert_eq!(&out, b"2345");
        assert_eq!(read(6, 4, &mut out), 1, "ends exactly at the end");
        assert_eq!(&out, b"6789");
        assert_eq!(read(7, 4, &mut out), 0, "one past the end");
        assert_eq!(
            read(c_ulong::MAX, 4, &mut out),
            0,
            "position + size overflows"
        );
        assert_eq!(read(20, 4, &mut out), 0, "start beyond the end");
        // SAFETY: a null buffer is rejected before it is touched.
        assert_eq!(unsafe { read_block(param, 0, ptr::null_mut(), 4) }, 0);
        // SAFETY: a null param is rejected before it is dereferenced.
        let read_null_param = unsafe { read_block(ptr::null_mut(), 0, out.as_mut_ptr(), 4) };
        assert_eq!(read_null_param, 0);
    }
}
