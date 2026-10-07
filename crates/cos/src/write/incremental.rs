//! The incremental writer (ISO 32000-2 §7.5.6).

use std::collections::{BTreeMap, BTreeSet};

use crate::error::{EncryptionError, Result, WriteError};
use crate::limits::LimitKind;
use crate::object::{Dict, ObjRef, Object};
use crate::store::ObjectStore;
use crate::xref::{SectionKind, XrefEntry};

use super::serialize::Serializer;
use super::xref::{write_table, xref_stream};
use super::{check_number, digest16, id_array, integer, reference, trailer_ids};

/// The body of an object to write.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum NewObject {
    /// Any object except a stream. Strings are given in the clear; with encryption they are
    /// encrypted on the way out.
    Value(Object<'static>),
    /// A stream: its dictionary and its data **as it is to be stored**, already run through the
    /// filters the dictionary's `/Filter` names (none if it has none). `/Length` is set by the
    /// writer; the data is encrypted on the way out when the document is encrypted.
    Stream {
        /// The stream dictionary.
        dict: Dict<'static>,
        /// The encoded data.
        data: Vec<u8>,
    },
}

/// What to change in a document: objects to add or replace, objects to free, and the `/Info`
/// reference. Build it, then pass it to [`incremental_update`].
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Changes {
    objects: BTreeMap<u32, NewObject>,
    freed: BTreeSet<u32>,
    info: Option<ObjRef>,
    next_new: Option<u32>,
}

impl Changes {
    /// No changes.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Writes `object` as object `number`, replacing the existing object of that number or
    /// adding it. The generation number is the one the cross-reference gives the number (0 for a
    /// number the file has never used).
    pub fn set_object(&mut self, number: u32, object: NewObject) {
        self.freed.remove(&number);
        self.objects.insert(number, object);
    }

    /// Adds `object` under a new number, the lowest above everything `store` knows and above
    /// numbers handed out by earlier calls, and returns its reference.
    pub fn add_object(&mut self, store: &ObjectStore<'_>, object: NewObject) -> ObjRef {
        let number = self
            .next_new
            .unwrap_or_else(|| store.write_base().size)
            .max(1);
        self.next_new = Some(number.saturating_add(1));
        self.set_object(number, object);
        ObjRef::new(number, 0)
    }

    /// Frees object `number`: later readers see it as deleted. Its generation number goes up by
    /// one, as the next use of the number needs (§7.5.4). Numbers that are already free or that
    /// the file does not have are ignored.
    pub fn free(&mut self, number: u32) {
        self.objects.remove(&number);
        self.freed.insert(number);
    }

    /// Makes `info` the trailer's `/Info` (the document information dictionary).
    pub fn set_info(&mut self, info: ObjRef) {
        self.info = Some(info);
    }
}

/// The generation to write an object under: the one the cross-reference holds for the number.
fn generation_of(entry: Option<XrefEntry>) -> u16 {
    match entry {
        Some(XrefEntry::InUse { generation, .. } | XrefEntry::Free { generation, .. }) => {
            generation
        }
        _ => 0,
    }
}

/// Free entries for the freed objects, chained through object 0 in number order. Empty when
/// nothing is freed.
fn free_entries(store: &ObjectStore<'_>, changes: &Changes) -> Vec<(u32, XrefEntry)> {
    let freed: Vec<(u32, u16)> = changes
        .freed
        .iter()
        .filter_map(|&number| match store.entry_of(number) {
            Some(XrefEntry::InUse { generation, .. }) => {
                Some((number, generation.saturating_add(1)))
            }
            Some(XrefEntry::Compressed { .. }) => Some((number, 1)),
            _ => None,
        })
        .collect();
    let Some(&(first, _)) = freed.first() else {
        return Vec::new();
    };
    let mut entries = vec![(
        0,
        XrefEntry::Free {
            next_free: u64::from(first),
            generation: u16::MAX,
        },
    )];
    for (i, &(number, generation)) in freed.iter().enumerate() {
        let next_free = freed.get(i + 1).map_or(0, |&(next, _)| u64::from(next));
        entries.push((
            number,
            XrefEntry::Free {
                next_free,
                generation,
            },
        ));
    }
    entries
}

/// The new trailer: the old one's `/Root`, `/Info` and `/Encrypt`, a new `/Size`, `/Prev` and an
/// `/ID` whose second string follows `appended` (see the [module documentation](super)).
fn build_trailer(
    store: &ObjectStore<'_>,
    changes: &Changes,
    size: u32,
    previous: u64,
    appended: &[u8],
) -> Result<Dict<'static>> {
    let mut trailer = Dict::default();
    trailer.set(b"Size", integer(u64::from(size)));
    if let Some(root) = store.trailer_value(b"Root") {
        trailer.set(b"Root", root);
    }
    if let Some(info) = changes.info {
        trailer.set(b"Info", reference(info.num));
    } else if let Some(info) = store.trailer_value(b"Info") {
        trailer.set(b"Info", info);
    }
    if let Some(encrypt) = store.trailer_value(b"Encrypt") {
        trailer.set(b"Encrypt", encrypt);
    }
    match trailer_ids(store)? {
        Some((first, second)) => {
            trailer.set(b"ID", id_array(&first, &digest16(&[&second, appended])));
        }
        // An encrypted file's key depends on its (absent) /ID[0]: do not invent one.
        None if store.decryptor().is_none() => {
            let id = digest16(&[appended]);
            trailer.set(b"ID", id_array(&id, &id));
        }
        None => {}
    }
    trailer.set(b"Prev", integer(previous));
    Ok(trailer)
}

/// Builds the bytes to append to the file `store` was opened on so that it contains `changes`.
///
/// **The result is only the appended section.** The new file is the original bytes followed by
/// it; nothing in the original is changed. The section is the new and replaced objects, a
/// cross-reference section in the same style as the file's newest one (table, or stream), and a
/// trailer carrying `/Size`, `/Root`, `/Info`, `/Encrypt`, `/ID` and `/Prev`, then `startxref`.
/// A file with a hybrid-reference trailer gets a plain table section (readers look it up first).
///
/// Freed objects are chained as the free list through object 0; free entries of earlier sections
/// that are not freed again are no longer chained, which no reader depends on.
///
/// With an encrypted document the new strings and streams are encrypted with the existing key
/// (see the [module documentation](super)); the store must be unlocked.
///
/// # Errors
/// - [`WriteError::NeedsFullRewrite`] if the cross-reference of the file had to be rebuilt,
/// - [`WriteError::NoRoot`] for a document without `/Root`,
/// - [`WriteError::InvalidObjectNumber`] for object 0 or a number the cross-reference cannot
///   hold, [`WriteError::EncryptionDictionary`] for a change to the `/Encrypt` object,
///   [`WriteError::StreamWithoutData`] for a stream given as a plain value,
///   [`WriteError::OffsetTooLarge`] when a classic table cannot hold an offset,
/// - [`EncryptionError::PasswordRequired`] for a locked store,
/// - [`Error::LimitExceeded`](crate::Error::LimitExceeded) when an object nests too deeply or
///   the document would have more objects than allowed,
/// - and the store's errors when it must read the trailer's `/ID`.
pub fn incremental_update(store: &ObjectStore<'_>, changes: &Changes) -> Result<Vec<u8>> {
    if store.is_locked() {
        return Err(EncryptionError::PasswordRequired.into());
    }
    let layout = store.write_base();
    let Some(style @ (SectionKind::Table | SectionKind::Stream)) = layout.kind else {
        return Err(WriteError::NeedsFullRewrite.into());
    };
    if store.trailer_value(b"Root").is_none() {
        return Err(WriteError::NoRoot.into());
    }
    // The new section sits on top of entries this does not read: they must be right.
    if !store.offsets_are_valid() {
        return Err(WriteError::NeedsFullRewrite.into());
    }
    let protected = store.encrypt_object_number();
    for &number in changes.objects.keys().chain(&changes.freed) {
        check_number(number)?;
        if protected == Some(number) {
            return Err(WriteError::EncryptionDictionary(number).into());
        }
    }

    let data = store.data();
    let limits = store.limits();
    let decryptor = store.decryptor();
    let serializer = Serializer::new(limits).with_crypt(decryptor.as_deref());
    let origin = layout.base;

    let mut out: Vec<u8> = Vec::new();
    // `%%EOF` may be the last thing in the file; the next object must start on a line of its own.
    if !matches!(data.last(), Some(b'\n' | b'\r')) {
        out.push(b'\n');
    }
    let relative = |out_len: usize| (data.len() + out_len - origin) as u64;

    let mut entries: Vec<(u32, XrefEntry)> = Vec::new();
    for (&number, object) in &changes.objects {
        let generation = generation_of(store.entry_of(number));
        let id = ObjRef::new(number, generation);
        let offset = relative(out.len());
        match object {
            NewObject::Value(value) => serializer.indirect(&mut out, id, value)?,
            NewObject::Stream { dict, data } => serializer.stream(&mut out, id, dict, data)?,
        }
        entries.push((number, XrefEntry::InUse { offset, generation }));
    }

    entries.extend(free_entries(store, changes));

    let highest = entries.iter().map(|&(n, _)| n).max().unwrap_or(0);
    let mut size = layout.size.max(highest.saturating_add(1));
    let stream_number = (style == SectionKind::Stream).then_some(size);
    if stream_number.is_some() {
        size = size.saturating_add(1);
    }
    limits.check(LimitKind::XrefEntries, u64::from(size), None)?;

    let previous = (layout.section_offset - origin) as u64;
    let trailer = build_trailer(store, changes, size, previous, &out)?;

    entries.sort_by_key(|&(number, _)| number);
    let section = relative(out.len());
    match stream_number {
        None => write_table(&mut out, &entries, &trailer, &Serializer::new(limits))?,
        Some(number) => {
            entries.push((
                number,
                XrefEntry::InUse {
                    offset: section,
                    generation: 0,
                },
            ));
            let (dict, bytes) = xref_stream(&entries, &trailer);
            Serializer::new(limits).stream(&mut out, ObjRef::new(number, 0), &dict, &bytes)?;
        }
    }
    out.extend_from_slice(format!("startxref\n{section}\n%%EOF\n").as_bytes());
    Ok(out)
}
