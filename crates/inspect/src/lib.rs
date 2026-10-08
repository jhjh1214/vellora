//! # vellora-inspect — document inspection and security analysis
//!
//! **Responsibility:** read-only analysis of a parsed document:
//!
//! - document summary: version, page count, object count, revisions, encryption
//! - feature detection: JavaScript, embedded files, launch / URI / submit
//!   actions, AcroForm, XFA, optional content, signatures
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
pub use summary::{EncryptionSummary, Features, Permissions, Summary};

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
    let (pages, features, scan_problems) = if locked {
        (None, None, Vec::new())
    } else {
        let mut scanner = scan::Scanner::new(store);
        scanner.run();
        (
            Some(scanner.pages),
            Some(scanner.features),
            scanner.problems,
        )
    };
    problems.extend(scan_problems);
    problems.truncate(MAX_PROBLEMS);

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
    }
}
