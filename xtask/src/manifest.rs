//! Corpus manifest (`tests/corpus/manifest.toml`): parsing and validation.

use std::collections::HashSet;

use serde::Deserialize;

use crate::Result;

/// The parsed manifest: a list of `[[doc]]` tables.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Manifest {
    pub(crate) doc: Vec<Doc>,
}

/// One corpus document.
#[derive(Debug, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Doc {
    /// Unique; becomes the file name `<id>.pdf`, so it is restricted to a portable charset.
    pub(crate) id: String,
    pub(crate) url: String,
    /// Lower-case hex SHA-256 of the file's bytes.
    pub(crate) sha256: String,
    pub(crate) license: String,
    pub(crate) categories: Vec<String>,
    pub(crate) notes: String,
    /// The password that opens an encrypted document (the user password, as the user would type
    /// it), for the corpus gate in `crates/engine/tests/corpus.rs`. Absent for documents that
    /// open without one.
    #[serde(default)]
    pub(crate) password: Option<String>,
}

impl Manifest {
    /// Parses and validates manifest text.
    pub(crate) fn parse(text: &str) -> Result<Self> {
        let manifest: Self = toml::from_str(text)?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<()> {
        let mut seen = HashSet::new();
        for doc in &self.doc {
            doc.validate()?;
            // Windows and macOS file systems are case-insensitive by default.
            if !seen.insert(doc.id.to_ascii_lowercase()) {
                return Err(format!("duplicate id (case-insensitive): {}", doc.id).into());
            }
        }
        Ok(())
    }
}

impl Doc {
    fn validate(&self) -> Result<()> {
        let id = &self.id;
        let id_ok = !id.is_empty()
            && id.len() <= 100
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
            && !id.starts_with('.');
        if !id_ok {
            return Err(
                format!("invalid id {id:?} (use [A-Za-z0-9._-], max 100, no leading dot)").into(),
            );
        }
        if !self.url.starts_with("https://") {
            return Err(format!("{id}: url must be https://").into());
        }
        let hash_ok = self.sha256.len() == 64
            && self
                .sha256
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !hash_ok {
            return Err(format!("{id}: sha256 must be 64 lower-case hex digits").into());
        }
        if self.license.trim().is_empty() {
            return Err(format!("{id}: license is required").into());
        }
        if self.notes.trim().is_empty() {
            return Err(format!("{id}: notes are required (record provenance)").into());
        }
        if self.categories.is_empty() || self.categories.iter().any(|c| c.trim().is_empty()) {
            return Err(format!("{id}: at least one non-empty category is required").into());
        }
        // What the protocol accepts as a password (`vellora_ipc::MAX_PASSWORD_BYTES`, no NUL).
        if let Some(password) = &self.password
            && (password.len() > 256 || password.contains('\0'))
        {
            return Err(format!("{id}: password is longer than 256 bytes or contains NUL").into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const HASH: &str = "0000000000000000000000000000000000000000000000000000000000000000";

    fn entry(id: &str, extra: &str) -> String {
        format!(
            "[[doc]]\nid = \"{id}\"\nurl = \"https://example.org/a.pdf\"\nsha256 = \"{HASH}\"\n\
             license = \"MIT\"\ncategories = [\"x\"]\nnotes = \"n\"\n{extra}\n"
        )
    }

    #[test]
    fn parses_valid_manifest() {
        let m = Manifest::parse(&format!("{}{}", entry("a", ""), entry("b", ""))).unwrap();
        assert_eq!(m.doc.len(), 2);
        assert_eq!(m.doc[1].id, "b");
    }

    #[test]
    fn rejects_duplicate_ids_ignoring_case() {
        let err =
            Manifest::parse(&format!("{}{}", entry("Foo", ""), entry("foo", ""))).unwrap_err();
        assert!(err.to_string().contains("duplicate"), "{err}");
    }

    #[test]
    fn rejects_path_like_ids() {
        for id in ["../x", "a/b", "a b", "", ".hidden"] {
            assert!(Manifest::parse(&entry(id, "")).is_err(), "{id:?} accepted");
        }
    }

    #[test]
    fn rejects_bad_fields() {
        let bad_hash = entry("a", "").replace(HASH, "ABC");
        assert!(Manifest::parse(&bad_hash).is_err());
        let http = entry("a", "").replace("https://", "http://");
        assert!(Manifest::parse(&http).is_err());
        let no_license = entry("a", "").replace("license = \"MIT\"", "license = \" \"");
        assert!(Manifest::parse(&no_license).is_err());
        let no_cats = entry("a", "").replace("[\"x\"]", "[]");
        assert!(Manifest::parse(&no_cats).is_err());
        assert!(Manifest::parse(&entry("a", "surprise = 1")).is_err());
        assert!(Manifest::parse(&entry("a", "password = \"a\\u0000b\"")).is_err());
        let long = format!("password = \"{}\"", "p".repeat(257));
        assert!(Manifest::parse(&entry("a", &long)).is_err());
    }

    #[test]
    fn a_password_is_optional_and_may_be_any_text() {
        let m = Manifest::parse(&format!(
            "{}{}",
            entry("a", ""),
            entry("b", "password = \"h\\u00F4tel \\u2168\"")
        ))
        .unwrap();
        assert_eq!(m.doc[0].password, None);
        assert_eq!(m.doc[1].password.as_deref(), Some("hôtel Ⅸ"));
    }

    /// Guards the committed manifest against drift from the M0 task 1 requirements.
    #[test]
    fn committed_manifest_is_valid_and_meets_corpus_v0_targets() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus/manifest.toml");
        let m = Manifest::parse(&std::fs::read_to_string(path).unwrap()).unwrap();
        assert!(
            m.doc.len() >= 200,
            "corpus v0 should hold ~200 documents, has {}",
            m.doc.len()
        );
        let malformed = m
            .doc
            .iter()
            .filter(|d| d.categories.iter().any(|c| c == "malformed"))
            .count();
        assert!(
            malformed >= 30,
            "need >= 30 malformed files, have {malformed}"
        );
    }

    /// `ci-manifest.toml` is what CI fetches for the invariant harness: a small subset whose
    /// entries must be verbatim copies of the main manifest's, so the two cannot drift apart.
    #[test]
    fn ci_manifest_is_a_small_verbatim_subset_of_the_main_manifest() {
        let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../tests/corpus");
        let main =
            Manifest::parse(&std::fs::read_to_string(dir.join("manifest.toml")).unwrap()).unwrap();
        let ci = Manifest::parse(&std::fs::read_to_string(dir.join("ci-manifest.toml")).unwrap())
            .unwrap();
        assert!(
            (1..=20).contains(&ci.doc.len()),
            "the CI subset holds 1..=20 documents, has {}",
            ci.doc.len()
        );
        for doc in &ci.doc {
            let original = main.doc.iter().find(|d| d.id == doc.id);
            assert_eq!(original, Some(doc), "{} differs from manifest.toml", doc.id);
        }
    }
}
