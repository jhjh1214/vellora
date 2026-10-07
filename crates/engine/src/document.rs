//! Opening a document: the same bytes go to `cos` and to PDFium, and the two are compared.
//!
//! PDFium renders; `cos` is the parser Vellora trusts to read and (later) write the file
//! (ADR-0002). If they disagree about the file, the document is flagged `repaired` so the UI can
//! say so, instead of silently showing what only one of them understood.

use std::fs::File;
use std::sync::Arc;

use vellora_cos::{ObjectStore, RepairReason};
use vellora_ipc::{MAX_PAGE_SIZES_PER_MESSAGE, PageSize};
use vellora_render::{DocHandle, Renderer};
use vellora_shm::MappedFile;

/// Size used for a page PDFium could not measure, so that one bad page does not fail the whole
/// document. US Letter in points.
const FALLBACK_PAGE_SIZE: PageSize = PageSize {
    width: 612.0,
    height: 792.0,
};

/// The mapped file as the byte buffer PDFium keeps for as long as the document is open.
struct SharedBytes(Arc<MappedFile>);

impl AsRef<[u8]> for SharedBytes {
    fn as_ref(&self) -> &[u8] {
        self.0.as_slice()
    }
}

/// Why a document could not be opened.
#[derive(Debug, thiserror::Error)]
pub(crate) enum OpenError {
    /// The file could not be mapped (too large, unreadable).
    #[error("cannot read the document: {0}")]
    Map(#[from] vellora_shm::Error),
    /// PDFium refused it.
    #[error("{0}")]
    Render(#[from] vellora_render::Error),
    /// More pages than the protocol can number.
    #[error("the document has {0} pages, more than the protocol supports")]
    TooManyPages(usize),
}

/// An open document and what the UI is told about it.
pub(crate) struct Document {
    pub(crate) handle: DocHandle,
    pub(crate) page_count: u32,
    /// The first pages' sizes (at most [`MAX_PAGE_SIZES_PER_MESSAGE`]).
    pub(crate) page_sizes: Vec<PageSize>,
    pub(crate) repaired: bool,
}

impl Document {
    /// Maps `file`, opens it in PDFium, checks it against `cos` and measures the first pages.
    pub(crate) fn open(
        renderer: &Renderer,
        file: &File,
        max_document_bytes: u64,
    ) -> Result<Self, OpenError> {
        let mapped = Arc::new(MappedFile::new(file, max_document_bytes)?);
        let handle = renderer.open(SharedBytes(Arc::clone(&mapped)))?;
        let pdfium_pages = handle.page_count();
        let page_count =
            u32::try_from(pdfium_pages).map_err(|_| OpenError::TooManyPages(pdfium_pages))?;

        let check = cross_check(mapped.as_slice(), pdfium_pages as u64);
        let mut repaired = check.repaired;

        let first_chunk = pdfium_pages.min(MAX_PAGE_SIZES_PER_MESSAGE);
        let mut page_sizes = Vec::with_capacity(first_chunk);
        for page in 0..first_chunk {
            match handle.page_size(page) {
                Ok((width, height)) if sane(width) && sane(height) => {
                    page_sizes.push(PageSize { width, height });
                }
                other => {
                    tracing::debug!(page, ?other, "page could not be measured, using Letter");
                    page_sizes.push(FALLBACK_PAGE_SIZE);
                    repaired = true;
                }
            }
        }

        tracing::info!(page_count, repaired, "document opened");
        Ok(Self {
            handle,
            page_count,
            page_sizes,
            repaired,
        })
    }
}

fn sane(points: f32) -> bool {
    points.is_finite() && points > 0.0
}

/// What `cos` made of a file that PDFium reports `pdfium_pages` pages for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CrossCheck {
    /// The two disagree, or `cos` had to repair the file to read it.
    pub(crate) repaired: bool,
    /// Why, for the debug log. These can quote the document, so they are never logged above
    /// debug level.
    pub(crate) notes: Vec<String>,
}

/// Reads `bytes` with `cos` and compares it with PDFium's page count.
///
/// The page walk stops one page past `pdfium_pages`: a disagreement is already certain by then,
/// and a hostile page tree cannot make the check run longer than the document it describes.
///
/// PDFium is not asked for page references (the narrow binding exposes none), so only the page
/// count is compared.
pub(crate) fn cross_check(bytes: &[u8], pdfium_pages: u64) -> CrossCheck {
    let mut check = CrossCheck::default();
    let mut flag = |note: String| {
        check.repaired = true;
        check.notes.push(note);
    };

    let store = match ObjectStore::open(bytes, vellora_cos::Limits::default()) {
        Ok(store) => store,
        Err(error) => {
            flag(format!("cos cannot read the file: {error}"));
            return check;
        }
    };
    // Rebuild a damaged cross-reference now, so that the page walk below does not depend on
    // which object happens to be read first (M0 task 13 findings).
    if let Err(error) = store.settle() {
        flag(format!("cos cannot settle the cross-reference: {error}"));
        return check;
    }
    if store.is_locked() {
        flag("cos cannot unlock the document with the empty password".to_owned());
        return check;
    }

    let mut walked = 0_u64;
    for page in store.pages() {
        match page {
            Ok(_) => walked += 1,
            Err(error) => flag(format!("cos cannot read a page: {error}")),
        }
        if walked > pdfium_pages {
            break;
        }
    }
    if walked != pdfium_pages {
        flag(format!(
            "cos finds {walked} pages where PDFium finds {pdfium_pages}"
        ));
    }
    for reason in store.repaired() {
        flag(describe(&reason));
    }
    check
}

fn describe(reason: &RepairReason) -> String {
    format!("cos repaired the file: {reason:?}")
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    /// A valid PDF of `pages` blank pages with a classic xref table.
    fn pdf(pages: usize) -> Vec<u8> {
        let kids = (0..pages)
            .map(|i| format!("{} 0 R", 3 + i))
            .collect::<Vec<_>>()
            .join(" ");
        let mut objects = vec![
            "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
            format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
        ];
        objects.extend(
            (0..pages)
                .map(|_| "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] >>".to_owned()),
        );
        let mut out = String::from("%PDF-1.4\n");
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(out.len());
            writeln!(out, "{} 0 obj\n{object}\nendobj", index + 1).unwrap();
        }
        let xref = out.len();
        writeln!(out, "xref\n0 {}\n0000000000 65535 f ", objects.len() + 1).unwrap();
        for offset in offsets {
            writeln!(out, "{offset:010} 00000 n ").unwrap();
        }
        write!(
            out,
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .unwrap();
        out.into_bytes()
    }

    #[test]
    fn a_clean_file_that_both_parsers_agree_on_is_not_repaired() {
        for pages in [0, 1, 7] {
            let check = cross_check(&pdf(pages), pages as u64);
            assert_eq!(check, CrossCheck::default(), "{pages} pages");
        }
    }

    #[test]
    fn a_page_count_mismatch_is_flagged_in_both_directions() {
        let file = pdf(3);
        for pdfium in [2, 4, 0] {
            let check = cross_check(&file, pdfium);
            assert!(check.repaired, "PDFium says {pdfium}");
            assert!(
                check.notes.iter().any(|note| note.contains("PDFium finds")),
                "{:?}",
                check.notes
            );
        }
    }

    #[test]
    fn the_page_walk_stops_one_page_past_what_pdfium_found() {
        // 5000 pages against PDFium's 1: the check must report the mismatch without walking all
        // of them. It cannot say how many were walked, so look at the note's count instead.
        let check = cross_check(&pdf(5000), 1);
        assert!(check.repaired);
        assert!(
            check.notes.iter().any(|n| n.contains("finds 2 pages")),
            "{:?}",
            check.notes
        );
    }

    #[test]
    fn a_damaged_cross_reference_that_cos_rebuilds_is_flagged() {
        let mut file = pdf(2);
        let at = file
            .windows(9)
            .rposition(|w| w == b"startxref")
            .expect("startxref");
        file[at..at + 9].copy_from_slice(b"startxraf");
        let check = cross_check(&file, 2);
        assert!(check.repaired);
        assert!(
            check.notes.iter().any(|note| note.contains("repaired")),
            "{:?}",
            check.notes
        );
    }

    #[test]
    fn a_file_cos_cannot_read_is_flagged_not_fatal() {
        for garbage in [&b""[..], b"not a pdf at all", &[0xFF; 64]] {
            let check = cross_check(garbage, 1);
            assert!(check.repaired, "{garbage:?}");
            assert!(check.notes.len() == 1, "{:?}", check.notes);
        }
    }
}
