//! The report types. Their serialised form is the `--json` schema documented in
//! `docs/user/cli.md`: renaming a field or changing its meaning is a breaking change.

use serde::Serialize;
use vellora_cos::{CryptMethod, EncryptionInfo, PasswordRole};

/// What `vellora inspect` knows about a document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct Summary {
    /// Size of the file in bytes.
    pub file_size: u64,
    /// The version in the `%PDF-M.m` header, e.g. `"1.7"`; `None` if the header has none.
    pub version: Option<String>,
    /// Objects the cross-reference lists as in use (compressed ones included). Not checked
    /// against the file.
    pub objects: u64,
    /// Revisions in the file's own incremental history (1 for a file that was never updated or
    /// whose cross-reference had to be rebuilt).
    pub revisions: u64,
    /// Whether the file had to be repaired to be read; see `repair_reasons`.
    pub repaired: bool,
    /// Why the file counts as repaired, one line each. Empty when `repaired` is false.
    pub repair_reasons: Vec<String>,
    /// The encryption, `None` for an unencrypted document.
    pub encryption: Option<EncryptionSummary>,
    /// The document is encrypted and no password was accepted: nothing but the encryption could
    /// be read, so `pages` and `features` are `None`.
    pub locked: bool,
    /// Pages found by walking the page tree; `None` for a locked document.
    pub pages: Option<u64>,
    /// What the document contains that matters for security or compatibility; `None` for a
    /// locked document.
    pub features: Option<Features>,
    /// Things that went wrong while reading the document, so some numbers may be too low. At
    /// most [`MAX_PROBLEMS`](crate::MAX_PROBLEMS) messages.
    pub problems: Vec<String>,
}

/// The Standard Security Handler settings of an encrypted document.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[non_exhaustive]
pub struct EncryptionSummary {
    /// The security handler (`/Filter`). Only `"Standard"` is supported.
    pub handler: &'static str,
    /// The algorithm version (`/V`).
    pub version: u32,
    /// The handler revision (`/R`), 2 to 6.
    pub revision: u32,
    /// Length of the file encryption key in bits.
    pub key_bits: u32,
    /// The raw permission mask (`/P`) as a signed 32-bit integer.
    pub permissions: i32,
    /// `permissions` decoded (Table 22 of ISO 32000-1, Table 24 of ISO 32000-2).
    pub allowed: Permissions,
    /// How streams are encrypted (`/StmF`): `identity`, `rc4`, `aes-128` or `aes-256`.
    pub stream_method: &'static str,
    /// How strings are encrypted (`/StrF`), as for `stream_method`.
    pub string_method: &'static str,
    /// Whether the document metadata stream is encrypted.
    pub encrypt_metadata: bool,
    /// Which password unlocked the document: `"user"` (also for the empty password) or `"owner"`;
    /// `None` when it is locked.
    pub unlocked_with: Option<&'static str>,
}

/// The user-access permissions of an encrypted document. They bind a conforming reader that
/// opened it with the user password; they are not a security boundary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // one flag per permission bit is the point
pub struct Permissions {
    /// Print (bit 3).
    pub print: bool,
    /// Modify the contents (bit 4).
    pub modify: bool,
    /// Copy or extract text and graphics (bit 5).
    pub copy: bool,
    /// Add or modify annotations and fill in forms (bit 6).
    pub annotate: bool,
    /// Fill in existing form fields (bit 9).
    pub fill_forms: bool,
    /// Extract text for accessibility (bit 10).
    pub accessibility: bool,
    /// Insert, rotate or delete pages and build bookmarks (bit 11).
    pub assemble: bool,
    /// Print at full quality (bit 12).
    pub print_high_quality: bool,
}

impl Permissions {
    /// Decodes the `/P` mask.
    #[must_use]
    pub fn from_mask(mask: i32) -> Self {
        let bit = |n: u32| mask & (1 << (n - 1)) != 0;
        Self {
            print: bit(3),
            modify: bit(4),
            copy: bit(5),
            annotate: bit(6),
            fill_forms: bit(9),
            accessibility: bit(10),
            assemble: bit(11),
            print_high_quality: bit(12),
        }
    }
}

impl EncryptionSummary {
    pub(crate) fn new(info: &EncryptionInfo, role: Option<PasswordRole>) -> Self {
        Self {
            handler: "Standard",
            version: info.version,
            revision: info.revision,
            key_bits: info.key_bits,
            permissions: info.permissions,
            allowed: Permissions::from_mask(info.permissions),
            stream_method: method_name(info.stream_method),
            string_method: method_name(info.string_method),
            encrypt_metadata: info.encrypt_metadata,
            unlocked_with: role.map(|role| match role {
                PasswordRole::User => "user",
                PasswordRole::Owner => "owner",
            }),
        }
    }
}

fn method_name(method: CryptMethod) -> &'static str {
    match method {
        CryptMethod::Identity => "identity",
        CryptMethod::Rc4 => "rc4",
        CryptMethod::AesV2 => "aes-128",
        CryptMethod::AesV3 => "aes-256",
        _ => "unknown",
    }
}

/// What the document contains. Each flag says the feature is present somewhere the scan looked;
/// see [`complete`](Self::complete) for when a `false` cannot be trusted.
///
/// Nothing is executed, opened or followed: the flags come from reading dictionaries only.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
#[non_exhaustive]
#[allow(clippy::struct_excessive_bools)] // one flag per feature is the point
pub struct Features {
    /// The scan read everything it set out to. `false` means it hit its work limit or could not
    /// read some objects (see `Summary::problems`), so a `false` flag below may be a miss.
    pub complete: bool,
    /// The catalog's `/OpenAction` is (or chains to) a JavaScript action: it runs on open.
    pub open_action_javascript: bool,
    /// A JavaScript action in the catalog, a page, an annotation, a form field or a bookmark.
    pub javascript_actions: bool,
    /// The catalog has a `/Names` `/JavaScript` name tree (document-level scripts).
    pub names_javascript: bool,
    /// An additional-actions dictionary (`/AA`) on the catalog, a page, an annotation or a form
    /// field: events such as open, close, print and focus.
    pub additional_actions: bool,
    /// A `Launch` action (starts another program or opens a file).
    pub launch_actions: bool,
    /// A `URI` action (opens a link).
    pub uri_actions: bool,
    /// A `SubmitForm` action (sends form data to a URL).
    pub submit_form_actions: bool,
    /// A `GoToR` action (opens another PDF file).
    pub goto_remote_actions: bool,
    /// Embedded files: a `/Names` `/EmbeddedFiles` tree or a file attachment annotation.
    pub embedded_files: bool,
    /// An interactive form (`/AcroForm`).
    pub acroform: bool,
    /// XFA form data (`/AcroForm` `/XFA`).
    pub xfa: bool,
    /// Optional content (layers): the catalog has `/OCProperties`.
    pub optional_content: bool,
    /// A signature field (`/FT /Sig`) in the form or on a page.
    pub signature_fields: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn permission_bits_follow_the_spec_numbering() {
        // Bit 3 is the value 4, bit 12 is 2048; the reserved high bits are set by writers.
        let p = Permissions::from_mask(-3904); // 0xFFFFF0C0: nothing allowed
        assert!(!p.print && !p.modify && !p.copy && !p.annotate);
        assert!(!p.fill_forms && !p.accessibility && !p.assemble && !p.print_high_quality);

        let p = Permissions::from_mask(4 | 2048);
        assert!(p.print && p.print_high_quality);
        assert!(!p.modify && !p.copy && !p.annotate && !p.fill_forms);

        let all = Permissions::from_mask(-1);
        assert!(all.print && all.modify && all.copy && all.annotate);
        assert!(all.fill_forms && all.accessibility && all.assemble && all.print_high_quality);
    }
}
