//! # vellora-inspect — document inspection and security analysis
//!
//! **Responsibility:** read-only analysis of a parsed document:
//!
//! - document summary: version, page count, object count, revisions (with byte ranges),
//!   encryption, the information dictionary, page sizes, tagging and linearization
//! - feature detection: JavaScript, embedded files, launch / URI / submit
//!   actions, AcroForm, XFA, optional content, signatures, with the URIs and the names of
//!   the embedded files
//! - later: object browser data, size attribution, fonts and images
//!
//! Every report type is serialisable, so the CLI (`--json`) and the UI share it.
//!
//! **Boundaries:** depends only on `vellora-cos`. It never modifies documents,
//! never renders, and never executes anything it finds.
//!
//! **Status:** [`Summary`] and the feature scan are done (M0 task 23). Entry points:
//! [`inspect`] for bytes, [`summarize`] for an [`ObjectStore`] that is already open.

mod scan;
mod summary;

use vellora_cos::{Error, Limits, ObjectStore};

pub use scan::MAX_PROBLEMS;
pub use summary::{
    DocumentInfo, EncryptionSummary, Features, MAX_LIST_ITEMS, MAX_PAGE_SIZES, MAX_REVISION_RANGES,
    MAX_TEXT_CHARS, PageSizeCount, Permissions, RevisionRange, StringList, Summary,
};

/// How to read the document.
#[derive(Debug, Clone, Default)]
#[non_exhaustive]
pub struct Options {
    /// Resource limits for everything read from the file.
    pub limits: Limits,
    /// The password to try. Without one the empty user password is used. See [`inspect`] for
    /// how the text becomes bytes.
    pub password: Option<String>,
}

/// Opens `data` and reports on it.
///
/// A password is tried as UTF-8 (what revisions 5 and 6 expect) and, if the text fits in
/// Latin-1, as Latin-1 bytes (what revisions 2 to 4 expect). No Unicode normalisation is applied.
///
/// A document that is encrypted and not unlocked still gives a report ([`Summary::locked`]),
/// with the encryption but without pages and features.
///
/// # Errors
/// What [`ObjectStore::open`] reports when the file cannot be read or its encryption is not
/// supported, and `IncorrectPassword` if [`Options::password`] is given and wrong.
pub fn inspect(data: &[u8], options: &Options) -> Result<Summary, Error> {
    let store = ObjectStore::open(data, options.limits.clone())?;
    if let Some(password) = &options.password {
        unlock(&store, password)?;
    }
    Ok(summarize(&store))
}

/// Tries the password in each encoding it has; the error is the first attempt's.
fn unlock(store: &ObjectStore<'_>, password: &str) -> Result<(), Error> {
    let utf8 = password.as_bytes().to_vec();
    let latin1: Option<Vec<u8>> = password
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect();
    let mut first_error = None;
    for candidate in std::iter::once(utf8.clone()).chain(latin1.filter(|bytes| *bytes != utf8)) {
        match store.authenticate(&candidate) {
            Ok(_) => return Ok(()),
            Err(error) => {
                first_error.get_or_insert(error);
            }
        }
    }
    Err(first_error.unwrap_or(Error::Encryption {
        kind: vellora_cos::EncryptionError::IncorrectPassword,
    }))
}

/// Reports on an open document. Reading may repair the file's cross-reference, so the report
/// is made after [`ObjectStore::settle`] and does not depend on what was read before.
///
/// Nothing in the report is an error: whatever could not be read is listed in
/// [`Summary::problems`].
#[must_use]
pub fn summarize(store: &ObjectStore<'_>) -> Summary {
    let mut problems = Vec::new();
    if let Err(error) = store.settle() {
        problems.push(format!("cross-reference: {error}"));
    }
    let locked = store.is_locked();
    let mut extras = (None, None, false, None, None);
    let (pages, features, scan_problems) = if locked {
        (None, None, Vec::new())
    } else {
        let mut scanner = scan::Scanner::new(store);
        scanner.run();
        extras = (
            Some(scanner.tagged),
            Some(scanner.page_sizes()),
            scanner.sizes_truncated,
            Some(scanner.uris.clone()),
            Some(scanner.embedded_files.clone()),
        );
        (
            Some(scanner.pages),
            Some(scanner.features),
            scanner.problems,
        )
    };
    problems.extend(scan_problems);
    problems.truncate(MAX_PROBLEMS);

    let (tagged, page_sizes, page_sizes_truncated, uris, embedded_files) = extras;

    // After the scan: reading objects can find damage and repair it.
    let repair_reasons: Vec<String> = store.repaired().iter().map(ToString::to_string).collect();
    Summary {
        file_size: store.data().len() as u64,
        version: store
            .header_version()
            .map(|(major, minor)| format!("{major}.{minor}")),
        objects: store.object_count() as u64,
        revisions: store.revision_count() as u64,
        repaired: !repair_reasons.is_empty(),
        repair_reasons,
        encryption: store
            .encryption_info()
            .map(|info| EncryptionSummary::new(&info, store.password_role())),
        locked,
        pages,
        features,
        problems,
        info: if locked { None } else { document_info(store) },
        tagged,
        linearized: is_linearized(store.data()),
        page_sizes,
        page_sizes_truncated,
        revision_ranges: store
            .revision_ranges()
            .into_iter()
            .take(MAX_REVISION_RANGES)
            .enumerate()
            .map(|(index, range)| RevisionRange::new(index as u64 + 1, range.start, range.end))
            .collect(),
        uris,
        embedded_files,
    }
}

/// The entries of the `/Info` dictionary that are strings.
fn document_info(store: &ObjectStore<'_>) -> Option<DocumentInfo> {
    let info = store.info().ok()??;
    let dict = info.as_dict()?;
    let text = |key: &[u8]| -> Option<String> {
        let value = store.deref(dict.get(key)?).ok()?;
        match &value.kind {
            vellora_cos::ObjectKind::String(bytes) => Some(vellora_cos::textstring::truncated(
                vellora_cos::textstring::decode_text_string(bytes),
                MAX_TEXT_CHARS,
            )),
            _ => None,
        }
    };
    Some(DocumentInfo {
        title: text(b"Title"),
        author: text(b"Author"),
        subject: text(b"Subject"),
        keywords: text(b"Keywords"),
        creator: text(b"Creator"),
        producer: text(b"Producer"),
        creation_date: text(b"CreationDate"),
        modification_date: text(b"ModDate"),
    })
}

/// Whether the first object of the file is a linearization dictionary (§F.2): within the first
/// kilobyte after the header there is an `obj` whose dictionary has a `/Linearized` key before the
/// object ends. Only the dictionary is looked for; nothing else about linearization is checked.
fn is_linearized(data: &[u8]) -> bool {
    const WINDOW: usize = 1024;
    let head = &data[..data.len().min(WINDOW)];
    let Some(obj) = head.windows(3).position(|w| w == b"obj") else {
        return false;
    };
    let after = &head[obj + 3..];
    let end = after
        .windows(6)
        .position(|w| w == b"endobj")
        .unwrap_or(after.len());
    after[..end].windows(11).any(|w| w == b"/Linearized")
}
