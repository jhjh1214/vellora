//! Opening a document: the same bytes go to `cos` and to PDFium, and the two are compared.
//!
//! PDFium renders; `cos` is the parser Vellora trusts to read and (later) write the file
//! (ADR-0002). If they disagree about the file, the document carries the reasons (`repairs`) so the UI
//! can say so, instead of silently showing what only one of them understood.

use std::borrow::Cow;
use std::fs::File;
use std::sync::Arc;

use vellora_cos::ObjectStore;
use vellora_ipc::{
    ErrorKind, MAX_PAGE_SIZES_PER_MESSAGE, MAX_REPAIR_MESSAGE_BYTES, MAX_REPAIRS, PageSize,
    Password, Repair,
};
use vellora_render::{DocHandle, LastError, Renderer};
use vellora_shm::MappedFile;
use zeroize::{Zeroize, Zeroizing};

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
    /// The document is encrypted and no password was given.
    #[error("password required")]
    PasswordRequired,
    /// The password does not open the document.
    #[error("incorrect password")]
    WrongPassword,
    /// PDFium refused it.
    #[error("{0}")]
    Render(#[from] vellora_render::Error),
    /// More pages than the protocol can number.
    #[error("the document has {0} pages, more than the protocol supports")]
    TooManyPages(usize),
}

impl OpenError {
    /// The class of error the UI is told.
    pub(crate) fn kind(&self) -> ErrorKind {
        match self {
            Self::PasswordRequired => ErrorKind::PasswordRequired,
            Self::WrongPassword => ErrorKind::WrongPassword,
            Self::Map(_) | Self::Render(_) | Self::TooManyPages(_) => ErrorKind::OpenFailed,
        }
    }
}

/// An open document and what the UI is told about it.
pub(crate) struct Document {
    pub(crate) handle: DocHandle,
    pub(crate) page_count: u32,
    /// The first pages' sizes (at most [`MAX_PAGE_SIZES_PER_MESSAGE`]).
    pub(crate) page_sizes: Vec<PageSize>,
    /// Why the document counts as repaired; empty when it does not.
    pub(crate) repairs: Vec<Repair>,
    /// The file's bytes, which the navigation thread reads with `cos` while PDFium renders.
    pub(crate) mapped: Arc<MappedFile>,
    /// The forms of the password the user typed, until the session hands them to the navigation
    /// thread (`std::mem::take`); empty when none was given. Wiped when dropped.
    pub(crate) unlock: Vec<Zeroizing<Vec<u8>>>,
}

impl Document {
    /// Maps `file`, opens it in PDFium (with `password` if there is one), checks it against `cos`
    /// and measures the first pages. The password itself is not kept; its forms stay in
    /// [`Self::unlock`] for the navigation thread to take.
    pub(crate) fn open(
        renderer: &Renderer,
        file: &File,
        max_document_bytes: u64,
        password: Option<&Password>,
    ) -> Result<Self, OpenError> {
        let mapped = Arc::new(MappedFile::new(file, max_document_bytes)?);
        let candidates = password.map(candidates);
        let handle = open_in_pdfium(renderer, &mapped, candidates.as_deref())?;
        let pdfium_pages = handle.page_count();
        let page_count =
            u32::try_from(pdfium_pages).map_err(|_| OpenError::TooManyPages(pdfium_pages))?;

        let mut check = CrossCheck::default();
        compare(
            mapped.as_slice(),
            pdfium_pages as u64,
            candidates.as_deref(),
            &mut check,
        );

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
                    check.flag(
                        "page-unmeasurable",
                        &format!(
                            "page {} could not be measured; Letter size is shown",
                            page + 1
                        ),
                    );
                }
            }
        }

        let repairs = check.finish().repairs;
        tracing::info!(
            page_count,
            repaired = !repairs.is_empty(),
            "document opened"
        );
        Ok(Self {
            handle,
            page_count,
            page_sizes,
            repairs,
            mapped,
            unlock: candidates.unwrap_or_default(),
        })
    }
}

/// The byte strings a typed password stands for, most likely first. Which one a file wants is not
/// known before trying:
///
/// 1. the text as typed, in UTF-8 (revisions 5 and 6 read the password as UTF-8);
/// 2. its `SASLprep` form (RFC 4013), when that differs: revision 6 requires the password to be
///    normalised that way (ISO 32000-2 §7.6.4.3.3), and neither `cos` nor PDFium does it;
/// 3. its Latin-1 form, when every character fits: revisions 2 to 4 read the password as
///    `PDFDocEncoding` bytes, which for the letters people type (U+00A1..U+00FF) are the Latin-1
///    bytes.
///
/// Copies made by the normaliser itself are not wiped; the forms returned here are.
fn candidates(password: &Password) -> Vec<Zeroizing<Vec<u8>>> {
    let text = password.expose();
    let mut forms = vec![Zeroizing::new(text.as_bytes().to_vec())];
    if !text.is_ascii() {
        // `Borrowed`: normalisation left the text alone, so it is already in `forms`. An error
        // (a prohibited character, an unassigned code point) leaves only the other forms.
        if let Ok(Cow::Owned(mut prepared)) = stringprep::saslprep(text) {
            if prepared != text {
                forms.push(Zeroizing::new(prepared.as_bytes().to_vec()));
            }
            prepared.zeroize();
        }
        if let Some(latin1) = text
            .chars()
            .map(|c| u8::try_from(u32::from(c)).ok())
            .collect::<Option<Vec<u8>>>()
        {
            forms.push(Zeroizing::new(latin1));
        }
    }
    forms
}

/// Opens the document in PDFium: without a password, or with each form of the one given until a
/// form opens it. A refusal for the password becomes the typed answer for the UI.
fn open_in_pdfium(
    renderer: &Renderer,
    mapped: &Arc<MappedFile>,
    candidates: Option<&[Zeroizing<Vec<u8>>]>,
) -> Result<DocHandle, OpenError> {
    let refused = |error: &vellora_render::Error| {
        matches!(error, vellora_render::Error::Open(LastError::Password))
    };
    let Some(candidates) = candidates else {
        tracing::debug!("opening without a password");
        return renderer
            .open(SharedBytes(Arc::clone(mapped)))
            .map_err(|error| {
                if refused(&error) {
                    OpenError::PasswordRequired
                } else {
                    error.into()
                }
            });
    };
    // Only counts are logged: a password, or anything derived from one, never is.
    tracing::debug!(forms = candidates.len(), "opening with a password");
    for candidate in candidates {
        match renderer.open_with_password(SharedBytes(Arc::clone(mapped)), candidate) {
            Ok(handle) => return Ok(handle),
            Err(error) if refused(&error) => tracing::debug!("a form of the password was refused"),
            Err(error) => return Err(error.into()),
        }
    }
    Err(OpenError::WrongPassword)
}

fn sane(points: f32) -> bool {
    points.is_finite() && points > 0.0
}

/// What `cos` made of a file that PDFium reports `pdfium_pages` pages for.
#[derive(Debug, Default, PartialEq, Eq)]
pub(crate) struct CrossCheck {
    /// Why the document counts as repaired; empty when it does not. `cos` had to repair the file
    /// to read it, or the two parsers disagree. Holds at most [`MAX_REPAIRS`] entries.
    pub(crate) repairs: Vec<Repair>,
    /// Findings past the cap, which [`Self::finish`] reports as one last entry.
    omitted: usize,
}

impl CrossCheck {
    /// Records one finding. The text is `cos`'s and the engine's own wording, which names object
    /// numbers and counts but never quotes the document.
    fn flag(&mut self, code: &str, message: &str) {
        if self.repairs.len() + 1 < MAX_REPAIRS {
            self.repairs.push(Repair {
                code: code.to_owned(),
                message: truncated(message, MAX_REPAIR_MESSAGE_BYTES),
            });
        } else {
            self.omitted += 1;
        }
    }

    /// Closes the list: findings past the cap become one entry saying how many there were.
    fn finish(mut self) -> Self {
        if self.omitted > 0 {
            let message = format!("{} more repairs are not listed", self.omitted);
            self.repairs.push(Repair {
                code: "more-repairs".to_owned(),
                message,
            });
            self.omitted = 0;
        }
        self
    }
}

/// `text` cut to at most `max` bytes at a character boundary.
fn truncated(text: &str, max: usize) -> String {
    let mut end = text.len().min(max);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_owned()
}

/// Reads `bytes` with `cos` and compares it with PDFium's page count, adding what differs to
/// `check`.
///
/// `candidates` are the forms of the password the user gave, if any; they unlock a document that
/// needs one.
///
/// The page walk stops one page past `pdfium_pages`: a disagreement is already certain by then,
/// and a hostile page tree cannot make the check run longer than the document it describes.
///
/// PDFium is not asked for page references (the narrow binding exposes none), so only the page
/// count is compared.
fn compare(
    bytes: &[u8],
    pdfium_pages: u64,
    candidates: Option<&[Zeroizing<Vec<u8>>]>,
    check: &mut CrossCheck,
) {
    let store = match ObjectStore::open(bytes, vellora_cos::Limits::default()) {
        Ok(store) => store,
        Err(error) => {
            check.flag(
                "cos-unreadable",
                &format!("cos cannot read the file: {error}"),
            );
            return;
        }
    };
    // Rebuild a damaged cross-reference now, so that the page walk below does not depend on
    // which object happens to be read first (M0 task 13 findings).
    if let Err(error) = store.settle() {
        check.flag(
            "cos-unsettled",
            &format!("cos cannot settle the cross-reference: {error}"),
        );
        return;
    }
    if store.is_locked() {
        let unlocked = candidates
            .unwrap_or_default()
            .iter()
            .any(|candidate| store.authenticate(candidate).is_ok());
        if !unlocked {
            check.flag(
                "cos-locked",
                if candidates.is_some() {
                    "cos cannot unlock the document with that password"
                } else {
                    "cos cannot unlock the document with the empty password"
                },
            );
            return;
        }
    }

    let mut walked = 0_u64;
    for page in store.pages() {
        match page {
            Ok(_) => walked += 1,
            Err(error) => check.flag(
                "cos-page-unreadable",
                &format!("cos cannot read a page: {error}"),
            ),
        }
        if walked > pdfium_pages {
            break;
        }
    }
    if walked != pdfium_pages {
        check.flag(
            "page-count-mismatch",
            &format!("cos finds {walked} pages where PDFium finds {pdfium_pages}"),
        );
    }
    for reason in store.repaired() {
        check.flag(reason.code(), &reason.to_string());
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use super::*;

    fn cross_check(bytes: &[u8], pdfium_pages: u64) -> CrossCheck {
        let mut check = CrossCheck::default();
        compare(bytes, pdfium_pages, None, &mut check);
        check.finish()
    }

    fn cross_check_with(bytes: &[u8], pdfium_pages: u64, password: &str) -> CrossCheck {
        let mut check = CrossCheck::default();
        let forms = candidates(&Password::new(password));
        compare(bytes, pdfium_pages, Some(&forms), &mut check);
        check.finish()
    }

    /// One page, user password `user-pw`, owner password `owner-pw` (see the cos fixtures).
    const PROTECTED: &[u8] =
        include_bytes!("../../cos/tests/fixtures/encryption/r3-rc4-128-user-password.pdf");

    fn forms(password: &str) -> Vec<Vec<u8>> {
        candidates(&Password::new(password))
            .iter()
            .map(|form| form.to_vec())
            .collect()
    }

    #[test]
    fn a_password_is_tried_as_typed_then_normalised_then_as_latin1() {
        assert_eq!(forms("abc"), [b"abc".to_vec()]);
        assert_eq!(forms(""), [Vec::<u8>::new()]);
        assert_eq!(
            forms("hôtel"),
            ["hôtel".as_bytes().to_vec(), b"h\xF4tel".to_vec()]
        );
        // SASLprep drops the soft hyphen (U+00AD) and folds U+00AA to "a": pdf.js' `saslprep-r6.pdf`.
        assert_eq!(
            forms("S\u{AA}SL\u{AD}prep"),
            [
                "S\u{AA}SL\u{AD}prep".as_bytes().to_vec(),
                b"SaSLprep".to_vec(),
                b"S\xAASL\xADprep".to_vec(),
            ]
        );
        // Prohibited (a control character): no normalised form, and the others stay.
        assert_eq!(forms("a\u{7}é").len(), 2);
        // U+00FF is the last Latin-1 letter, U+0100 the first that is not.
        assert_eq!(forms("\u{FF}").len(), 2);
        assert_eq!(forms("\u{100}"), ["\u{100}".as_bytes().to_vec()]);
        assert_eq!(forms("日本語").len(), 1);
        assert_eq!(forms("a😀").len(), 1);
    }

    #[test]
    fn cos_is_unlocked_with_the_password_and_then_has_nothing_to_report() {
        for password in ["user-pw", "owner-pw"] {
            let check = cross_check_with(PROTECTED, 1, password);
            assert_eq!(check, CrossCheck::default(), "{password}");
        }
    }

    #[test]
    fn cos_that_cannot_unlock_the_document_says_which_password_it_had() {
        let none = cross_check(PROTECTED, 1);
        assert_eq!(codes(&none), ["cos-locked"]);
        assert!(none.repairs[0].message.contains("empty password"));
        let wrong = cross_check_with(PROTECTED, 1, "wrong");
        assert_eq!(codes(&wrong), ["cos-locked"]);
        assert!(wrong.repairs[0].message.contains("that password"));
        assert!(!wrong.repairs[0].message.contains("wrong"));
    }

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

    fn codes(check: &CrossCheck) -> Vec<&str> {
        check.repairs.iter().map(|r| r.code.as_str()).collect()
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
            assert_eq!(
                codes(&check),
                ["page-count-mismatch"],
                "PDFium says {pdfium}"
            );
            assert!(
                check.repairs[0].message.contains("PDFium finds"),
                "{:?}",
                check.repairs
            );
        }
    }

    #[test]
    fn the_page_walk_stops_one_page_past_what_pdfium_found() {
        // 5000 pages against PDFium's 1: the check must report the mismatch without walking all
        // of them. It cannot say how many were walked, so look at the message's count instead.
        let check = cross_check(&pdf(5000), 1);
        assert_eq!(codes(&check), ["page-count-mismatch"]);
        assert!(
            check.repairs[0].message.contains("finds 2 pages"),
            "{:?}",
            check.repairs
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
        assert_ne!(check.repairs, Vec::<Repair>::new());
        assert!(
            check
                .repairs
                .iter()
                .all(|r| r.code != "page-count-mismatch"),
            "{:?}",
            check.repairs
        );
        // The reason is cos's own wording and code, not a Debug dump.
        assert!(
            check
                .repairs
                .iter()
                .all(|r| !r.message.contains("RepairReason")),
            "{:?}",
            check.repairs
        );
    }

    #[test]
    fn a_file_cos_cannot_read_is_flagged_not_fatal() {
        for garbage in [&b""[..], b"not a pdf at all", &[0xFF; 64]] {
            let check = cross_check(garbage, 1);
            assert_eq!(check.repairs.len(), 1, "{garbage:?}: {:?}", check.repairs);
            assert!(check.repairs[0].code.starts_with("cos-"), "{garbage:?}");
        }
    }

    #[test]
    fn a_flood_of_findings_is_cut_to_the_cap_with_a_count() {
        let mut check = CrossCheck::default();
        for i in 0..100 {
            check.flag("object-recovered", &format!("object {i} needed a repair"));
        }
        let check = check.finish();
        assert_eq!(check.repairs.len(), MAX_REPAIRS);
        let last = check.repairs.last().expect("a last entry");
        assert_eq!(last.code, "more-repairs");
        assert_eq!(
            last.message,
            format!("{} more repairs are not listed", 100 - (MAX_REPAIRS - 1))
        );
        // Exactly at the cap needs no summary entry.
        let mut check = CrossCheck::default();
        for _ in 0..MAX_REPAIRS - 1 {
            check.flag("x", "y");
        }
        assert_eq!(check.finish().repairs.len(), MAX_REPAIRS - 1);
    }

    #[test]
    fn long_text_is_cut_on_a_character_boundary() {
        let long = "é".repeat(MAX_REPAIR_MESSAGE_BYTES);
        let mut check = CrossCheck::default();
        check.flag("x", &long);
        let message = &check.finish().repairs[0].message;
        assert!(message.len() <= MAX_REPAIR_MESSAGE_BYTES);
        assert!(message.chars().all(|c| c == 'é'));
    }
}
