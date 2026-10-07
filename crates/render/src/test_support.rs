//! Shared by the unit tests: finding the fetched PDFium, a generated fixture PDF and a lock that
//! serialises tests (PDFium is one instance per process).

use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::{LIBRARY_ENV, Pdfium};

/// Where `cargo xtask pdfium fetch` puts the library for this machine, or `$VELLORA_PDFIUM_LIB`.
pub(crate) fn library_path() -> PathBuf {
    if let Some(path) = env::var_os(LIBRARY_ENV) {
        return PathBuf::from(path);
    }
    let (platform, library) = if cfg!(windows) {
        ("win-x64", "bin/pdfium.dll")
    } else if cfg!(target_os = "macos") {
        let platform = if cfg!(target_arch = "aarch64") {
            "mac-arm64"
        } else {
            "mac-x64"
        };
        (platform, "lib/libpdfium.dylib")
    } else {
        ("linux-x64", "lib/libpdfium.so")
    };
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../third_party/pdfium")
        .join(platform)
        .join(library)
}

/// Tests that touch PDFium (or its process-wide claim) hold this for their whole duration.
pub(crate) fn serial() -> MutexGuard<'static, ()> {
    static SERIAL: Mutex<()> = Mutex::new(());
    SERIAL.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Runs `f` with a freshly loaded PDFium. **Fails** (does not skip) if the library was not
/// fetched: a green run must mean PDFium really was exercised.
pub(crate) fn with_pdfium<R>(f: impl FnOnce(&Pdfium) -> R) -> R {
    let _serial = serial();
    let path = library_path();
    assert!(
        path.is_file(),
        "PDFium not found at {}: run `cargo xtask pdfium fetch` (or set {LIBRARY_ENV})",
        path.display()
    );
    let pdfium = Pdfium::load(&path).unwrap();
    f(&pdfium)
}

/// Page sizes of [`sample_pdf`], in points.
pub(crate) const SAMPLE_PAGE_SIZES: [(f32, f32); 3] =
    [(200.0, 100.0), (300.0, 400.0), (612.0, 792.0)];

/// A valid three-page PDF. Every page draws one black rectangle, (20,20)-(180,80) in page space,
/// on a white page, which is symmetric top to bottom on the first (200 x 100) page.
pub(crate) fn sample_pdf() -> Vec<u8> {
    let content = "0 0 0 rg 20 20 160 60 re f";
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>".to_owned(),
    ];
    for (w, h) in SAMPLE_PAGE_SIZES {
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Contents 6 0 R >>"
        ));
    }
    objects.push(format!(
        "<< /Length {} >>\nstream\n{content}\nendstream",
        content.len()
    ));

    let mut pdf = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        write!(pdf, "{} 0 obj\n{object}\nendobj\n", index + 1).unwrap();
    }
    let xref = pdf.len();
    write!(pdf, "xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).unwrap();
    for offset in offsets {
        // Each entry is exactly 20 bytes: 10 digits, space, 5 digits, space, `n`, space, LF.
        writeln!(pdf, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
        objects.len() + 1
    )
    .unwrap();
    pdf.into_bytes()
}
