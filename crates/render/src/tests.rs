use std::fs;

use super::*;
use crate::test_support::{library_path, sample_pdf, serial, with_pdfium};

#[test]
fn reports_the_page_count_of_a_fixture() {
    let pdf = sample_pdf();
    with_pdfium(|pdfium| assert_eq!(pdfium.page_count(&pdf).unwrap(), 3));
}

#[test]
fn bytes_that_are_not_a_pdf_are_a_typed_open_error() {
    with_pdfium(|pdfium| {
        let err = pdfium.page_count(b"this is not a PDF").unwrap_err();
        assert!(matches!(err, Error::Open(LastError::Format)), "{err:?}");
        // The instance stays usable after a failed open.
        assert_eq!(pdfium.page_count(&sample_pdf()).unwrap(), 3);
    });
}

#[test]
fn truncated_fixture_is_an_error_or_repaired_but_never_a_crash() {
    let pdf = sample_pdf();
    with_pdfium(|pdfium| {
        for cut in [0, 1, 8, pdf.len() / 2, pdf.len() - 1] {
            // PDFium may repair a truncated file (the trailer is missing), so only the absence of a
            // crash and a sane page count are asserted.
            if let Ok(pages) = pdfium.page_count(&pdf[..cut]) {
                assert!(pages <= 3, "cut at {cut}: {pages} pages");
            }
        }
    });
}

#[test]
fn only_one_instance_exists_at_a_time() {
    with_pdfium(|_first| {
        let second = Pdfium::load(library_path());
        assert!(matches!(second, Err(Error::AlreadyLoaded)));
    });
    // Dropping the first instance releases the claim and destroys the library.
    with_pdfium(|pdfium| assert_eq!(pdfium.page_count(&sample_pdf()).unwrap(), 3));
}

#[test]
fn a_missing_library_is_a_typed_error_and_does_not_hold_the_claim() {
    let guard = serial();
    let err = Pdfium::load("definitely-not-a-library.so").err().unwrap();
    assert!(matches!(err, Error::Load { .. }), "{err:?}");
    assert!(
        err.to_string().contains("definitely-not-a-library"),
        "{err}"
    );
    drop(guard);
    with_pdfium(|pdfium| assert_eq!(pdfium.page_count(&sample_pdf()).unwrap(), 3));
}

#[test]
fn a_library_without_pdfium_symbols_is_a_typed_error() {
    // Any always-present system library: it loads, but exports no PDFium function.
    let other = if cfg!(windows) {
        "kernel32.dll"
    } else if cfg!(target_os = "macos") {
        "/usr/lib/libSystem.B.dylib"
    } else {
        "libc.so.6"
    };
    let guard = serial();
    let err = Pdfium::load(other).err().unwrap();
    assert!(
        matches!(err, Error::Symbol { name, .. } if name == "FPDF_InitLibrary"),
        "{err:?}"
    );
    // The failed load released the claim.
    drop(guard);
    with_pdfium(|pdfium| assert_eq!(pdfium.page_count(&sample_pdf()).unwrap(), 3));
}

#[test]
fn locate_prefers_the_override_and_never_falls_back_from_it() {
    let dir = std::env::temp_dir().join(format!("vellora-render-locate-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    let in_exe_dir = dir.join(library_file_name());
    let elsewhere = dir.join("custom-name");
    fs::write(&in_exe_dir, b"x").unwrap();
    fs::write(&elsewhere, b"x").unwrap();

    // Next to the executable.
    assert_eq!(locate_in(None, Some(&dir)).unwrap(), in_exe_dir);
    // The override wins.
    let found = locate_in(Some(elsewhere.clone().into_os_string()), Some(&dir)).unwrap();
    assert_eq!(found, elsewhere);
    // A wrong override is an error even though the executable directory has a library.
    let bad = dir.join("missing");
    let err = locate_in(Some(bad.into_os_string()), Some(&dir)).unwrap_err();
    assert!(matches!(err, Error::NotFound { .. }), "{err:?}");
    // Nothing anywhere.
    fs::remove_file(&in_exe_dir).unwrap();
    assert!(matches!(
        locate_in(None, Some(&dir)),
        Err(Error::NotFound { .. })
    ));
    assert!(matches!(locate_in(None, None), Err(Error::NotFound { .. })));
}

#[test]
fn last_error_codes_map_to_variants() {
    let cases = [
        (1, LastError::Unknown),
        (2, LastError::File),
        (3, LastError::Format),
        (4, LastError::Password),
        (5, LastError::Security),
        (6, LastError::Page),
        (0, LastError::Other(0)),
        (99, LastError::Other(99)),
    ];
    for (code, expected) in cases {
        assert_eq!(LastError::from_code(code), expected);
    }
}
