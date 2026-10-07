//! The PDF writers: the **only** code in Vellora that produces PDF bytes (CLAUDE.md invariant 1).
//!
//! - [`write_object`] serialises one object canonically (deterministic output that parses back to
//!   the same object).
//! - [`incremental_update`] appends a revision to an open file (ISO 32000-2 §7.5.6). The result
//!   is only the **appended bytes**: the file on disk is the original bytes followed by them, and
//!   the original bytes are never touched (invariant 4).
//! - [`write_full`] writes the objects reachable from the trailer as a new file, renumbered and
//!   without earlier revisions.
//!
//! # Choices
//!
//! - **Identity.** An incremental update keeps every object number and generation. A full rewrite
//!   renumbers densely (1..n in discovery order, generation 0) and drops what nothing refers to.
//! - **Repaired files.** A store whose cross-reference was rebuilt by the recovery scan has no
//!   revision chain to extend; [`incremental_update`] refuses it with
//!   [`WriteError::NeedsFullRewrite`] and the caller uses [`write_full`] (see the data model: a
//!   *repaired* document is always saved in full).
//! - **File identifier.** `/ID[0]` is never changed by an update (it is the identity of the
//!   document, and for encrypted files part of the key). `/ID[1]` becomes the first 16 bytes of a
//!   SHA-256 over the old `/ID[1]` and the new bytes, so output is deterministic. A file with no
//!   `/ID` gets one, except an encrypted file, whose key already depends on the absent value.
//! - **Encryption.** An incremental update encrypts the new strings and streams with the file's
//!   existing key and copies `/Encrypt` into the new trailer; it needs an unlocked store. A full
//!   rewrite writes **unencrypted by default** ([`EncryptionPolicy::Remove`]), because the output
//!   is then valid with no password and nothing can be left half-encrypted;
//!   [`EncryptionPolicy::Keep`] re-encrypts everything under the same key and the same
//!   `/Encrypt` dictionary and `/ID[0]`. Changing passwords or algorithms is not offered: it
//!   needs a source of randomness and a UI decision (see `docs/milestones/M0.md`, task 12).
//! - **No randomness and no clock.** Output depends only on the input and the request.

mod full;
mod incremental;
mod serialize;
mod xref;

use sha2::{Digest as _, Sha256};

use crate::error::{Result, WriteError};
use crate::object::{Object, ObjectKind};
use crate::store::ObjectStore;

pub use full::{EncryptionPolicy, FullOptions, write_full};
pub use incremental::{Changes, NewObject, incremental_update};
pub use serialize::write_object;

/// The first 16 bytes of the SHA-256 of `parts`, used for file identifiers.
fn digest16(parts: &[&[u8]]) -> Vec<u8> {
    let mut hash = Sha256::new();
    for part in parts {
        hash.update(part);
    }
    hash.finalize().get(..16).unwrap_or_default().to_vec()
}

/// The two strings of the trailer's `/ID`, if it has a usable one.
fn trailer_ids(store: &ObjectStore<'_>) -> Result<Option<(Vec<u8>, Vec<u8>)>> {
    let Some(value) = store.trailer_value(b"ID") else {
        return Ok(None);
    };
    let value = store.deref(&value)?;
    let ObjectKind::Array(items) = &value.kind else {
        return Ok(None);
    };
    let string = |item: Option<&Object<'_>>| match item.map(|o| &o.kind) {
        Some(ObjectKind::String(s)) => Some(s.to_vec()),
        _ => None,
    };
    Ok(match (string(items.first()), string(items.get(1))) {
        (Some(first), Some(second)) => Some((first, second)),
        (Some(first), None) => Some((first.clone(), first)),
        _ => None,
    })
}

/// `[<a> <b>]` for the trailer.
fn id_array(first: &[u8], second: &[u8]) -> Object<'static> {
    let string = |bytes: &[u8]| Object::new(ObjectKind::String(bytes.to_vec().into()));
    Object::new(ObjectKind::Array(vec![string(first), string(second)]))
}

fn reference(num: u32) -> Object<'static> {
    Object::new(ObjectKind::Ref(crate::ObjRef::new(num, 0)))
}

fn integer(n: u64) -> Object<'static> {
    Object::new(ObjectKind::Integer(i64::try_from(n).unwrap_or(i64::MAX)))
}

/// The object number must be usable in a cross-reference.
fn check_number(number: u32) -> Result<()> {
    if number == 0 || number == u32::MAX {
        return Err(WriteError::InvalidObjectNumber(number).into());
    }
    Ok(())
}
