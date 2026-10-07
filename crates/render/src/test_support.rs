//! Shared by the unit tests: finding the fetched PDFium, a generated fixture PDF and a lock that
//! serialises tests (PDFium is one instance per process).

use std::env;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, PoisonError};

use crate::{LIBRARY_ENV, Pdfium, Renderer};

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

/// Like [`with_pdfium`], with a [`Renderer`] (which owns the process's one PDFium while it lives).
pub(crate) fn with_renderer<R>(f: impl FnOnce(&Renderer) -> R) -> R {
    let _serial = serial();
    let path = library_path();
    assert!(
        path.is_file(),
        "PDFium not found at {}: run `cargo xtask pdfium fetch` (or set {LIBRARY_ENV})",
        path.display()
    );
    let renderer = Renderer::start(&path).unwrap();
    f(&renderer)
}

/// Page sizes of [`sample_pdf`], in points.
pub(crate) const SAMPLE_PAGE_SIZES: [(f32, f32); 3] =
    [(200.0, 100.0), (300.0, 400.0), (612.0, 792.0)];

/// One page of a generated fixture.
pub(crate) struct PageSpec {
    /// MediaBox size in points.
    pub(crate) size: (f32, f32),
    /// `/Rotate`, a multiple of 90.
    pub(crate) rotate: i32,
    /// The content stream.
    pub(crate) content: String,
}

/// A valid PDF with one content stream per page and a classic xref table.
pub(crate) fn build_pdf(pages: &[PageSpec]) -> Vec<u8> {
    let count = pages.len();
    let kids = (0..count)
        .map(|i| format!("{} 0 R", 3 + i))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {count} >>"),
    ];
    for (i, page) in pages.iter().enumerate() {
        let ((w, h), rotate) = (page.size, page.rotate);
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {w} {h}] /Rotate {rotate} /Contents {} 0 R >>",
            3 + count + i
        ));
    }
    for page in pages {
        objects.push(format!(
            "<< /Length {} >>
stream
{}
endstream",
            page.content.len(),
            page.content
        ));
    }

    let mut pdf = String::from(
        "%PDF-1.4
",
    );
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(pdf.len());
        write!(
            pdf,
            "{} 0 obj
{object}
endobj
",
            index + 1
        )
        .unwrap();
    }
    let xref = pdf.len();
    write!(
        pdf,
        "xref
0 {}
0000000000 65535 f 
",
        objects.len() + 1
    )
    .unwrap();
    for offset in offsets {
        // Each entry is exactly 20 bytes: 10 digits, space, 5 digits, space, `n`, space, LF.
        writeln!(pdf, "{offset:010} 00000 n ").unwrap();
    }
    write!(
        pdf,
        "trailer
<< /Size {} /Root 1 0 R >>
startxref
{xref}
%%EOF
",
        objects.len() + 1
    )
    .unwrap();
    pdf.into_bytes()
}

/// A valid three-page PDF. Every page draws one black rectangle, (20,20)-(180,80) in page space,
/// on a white page, which is symmetric top to bottom on the first (200 x 100) page.
pub(crate) fn sample_pdf() -> Vec<u8> {
    let pages: Vec<PageSpec> = SAMPLE_PAGE_SIZES
        .into_iter()
        .map(|size| PageSpec {
            size,
            rotate: 0,
            content: "0 0 0 rg 20 20 160 60 re f".to_owned(),
        })
        .collect();
    build_pdf(&pages)
}

/// Three pages for the golden images: curves (anti-aliasing), a rotated page, and a clip. No
/// text, so the result does not depend on the fonts of the machine.
pub(crate) fn golden_pdf() -> Vec<u8> {
    // A circle of radius 50 around (110, 70) from four Bezier arcs (kappa = 0.5523).
    let circle = "0.2 0.4 0.9 rg 160 70 m 160 97.6 137.6 120 110 120 c          82.4 120 60 97.6 60 70 c 60 42.4 82.4 20 110 20 c 137.6 20 160 42.4 160 70 c f";
    let pages = [
        PageSpec {
            size: (220.0, 140.0),
            rotate: 0,
            content: format!(
                "0.85 0.85 0.85 rg 10 10 60 120 re f {circle}                  0.9 0.1 0.1 RG 3 w 5 5 m 215 135 l S"
            ),
        },
        // 160 x 200 points, shown turned by 90 degrees (so 200 x 160).
        PageSpec {
            size: (160.0, 200.0),
            rotate: 90,
            content: "1 0.6 0 rg 20 20 m 140 40 l 80 180 l f                       0 0 0 RG 2 w [6 3] 0 d 10 10 140 180 re S"
                .to_owned(),
        },
        PageSpec {
            size: (180.0, 120.0),
            rotate: 0,
            // Stripes of rising grey that overshoot a rectangular clip, so the clip cuts them.
            content: format!(
                "q 30 20 120 80 re W n {} Q 0 0 1 RG 1 w 0.5 0.5 179 119 re S",
                (0_i16..12)
                    .map(|i| format!("{0:.2} g {1} 0 13 120 re f", f32::from(i) / 12.0, 10 + i * 13))
                    .collect::<Vec<_>>()
                    .join(" ")
            ),
        },
    ];
    build_pdf(&pages)
}
