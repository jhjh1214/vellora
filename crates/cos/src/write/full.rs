//! The full writer: a new file holding what is reachable from the trailer.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use crate::error::{EncryptionError, Result, WriteError};
use crate::limits::LimitKind;
use crate::object::{Dict, ObjRef, Object, ObjectKind};
use crate::recovery::RepairReason;
use crate::store::ObjectStore;
use crate::xref::XrefEntry;

use super::serialize::{Serializer, effective_entries};
use super::xref::{flate_compress, write_table, xref_stream};
use super::{check_number, digest16, id_array, integer, reference, trailer_ids};

/// Objects per object stream. Readers parse a whole stream to reach one member, so the size
/// trades file size against lookup cost; 100 is what common producers use.
const OBJECTS_PER_STREAM: usize = 100;

/// What a full rewrite does with the document's encryption.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub enum EncryptionPolicy {
    /// Write the output unencrypted (the default). The file opens without a password.
    #[default]
    Remove,
    /// Encrypt the output again with the document's own key: same `/Encrypt` dictionary
    /// (so the same passwords and permissions), same `/ID[0]`. The store must be unlocked, and
    /// the document must be encrypted.
    Keep,
}

/// Options for [`write_full`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
#[non_exhaustive]
pub struct FullOptions {
    /// Write a cross-reference stream instead of a classic table (PDF 1.5).
    pub xref_stream: bool,
    /// Pack objects that are not streams into object streams (PDF 1.5). This needs a
    /// cross-reference stream, so it implies [`xref_stream`](Self::xref_stream).
    pub object_streams: bool,
    /// What to do with encryption.
    pub encryption: EncryptionPolicy,
}

/// Collects the references inside `object` (and a stream's dictionary) in document order.
/// Iterative: it does not depend on how deep the object nests.
fn collect_refs(object: &Object<'_>, found: &mut VecDeque<ObjRef>) {
    let mut stack = vec![object];
    while let Some(current) = stack.pop() {
        match &current.kind {
            ObjectKind::Ref(reference) => found.push_back(*reference),
            ObjectKind::Array(items) => stack.extend(items.iter().rev()),
            ObjectKind::Dict(dict) => {
                stack.extend(effective_entries(dict).into_iter().rev().map(|e| &e.value));
            }
            ObjectKind::Stream(stream) => {
                stack.extend(
                    effective_entries(&stream.dict)
                        .into_iter()
                        .rev()
                        .map(|e| &e.value),
                );
            }
            _ => {}
        }
    }
}

/// Removes `/Crypt` from the stream's filter chain (and its entry in `/DecodeParms`). The data
/// is already decrypted, and the output has no `/Encrypt`.
fn strip_crypt_filter(dict: &mut Dict<'static>) {
    let is_crypt =
        |o: &Object<'_>| matches!(&o.kind, ObjectKind::Name(n) if n.as_ref() == b"Crypt");
    let Some(filter) = dict.get(b"Filter").cloned() else {
        return;
    };
    match filter.kind {
        ObjectKind::Name(_) if is_crypt(&filter) => {
            dict.remove(b"Filter");
            dict.remove(b"DecodeParms");
        }
        ObjectKind::Array(mut filters) if filters.iter().any(is_crypt) => {
            let mut parms = match dict.get(b"DecodeParms").map(|p| p.kind.clone()) {
                Some(ObjectKind::Array(parms)) => Some(parms),
                _ => None,
            };
            let mut i = 0;
            while i < filters.len() {
                if filters.get(i).is_some_and(is_crypt) {
                    filters.remove(i);
                    if let Some(parms) = parms.as_mut()
                        && i < parms.len()
                    {
                        parms.remove(i);
                    }
                } else {
                    i += 1;
                }
            }
            if filters.is_empty() {
                dict.remove(b"Filter");
                dict.remove(b"DecodeParms");
            } else {
                dict.set(b"Filter", Object::new(ObjectKind::Array(filters)));
                if let Some(parms) = parms {
                    dict.set(b"DecodeParms", Object::new(ObjectKind::Array(parms)));
                }
            }
        }
        _ => {}
    }
}

/// The objects reachable from the root, numbered.
struct Reachable {
    /// Old reference and object, in the order of the new numbers (index + 1).
    objects: Vec<(ObjRef, Arc<Object<'static>>)>,
    /// Old object number to new object number.
    numbers: HashMap<u32, u32>,
}

/// Walks from `roots` over references, breadth first, and numbers what it finds. A reference to
/// a missing object is not numbered, so it is written as `null`.
fn discover(store: &ObjectStore<'_>, roots: VecDeque<ObjRef>) -> Result<Reachable> {
    let limits = store.limits();
    let mut queue = roots;
    let mut seen: HashSet<u32> = HashSet::new();
    let mut found = Reachable {
        objects: Vec::new(),
        numbers: HashMap::new(),
    };
    while let Some(reference) = queue.pop_front() {
        if !seen.insert(reference.num) {
            continue;
        }
        let object = store.resolve(reference)?;
        if matches!(object.kind, ObjectKind::Null) {
            continue;
        }
        let next = u64::try_from(found.objects.len()).unwrap_or(u64::MAX) + 1;
        limits.check(LimitKind::XrefEntries, next, None)?;
        // Within the limit checked above, so it fits.
        found
            .numbers
            .insert(reference.num, u32::try_from(next).unwrap_or(u32::MAX));
        collect_refs(&object, &mut queue);
        found.objects.push((reference, object));
    }
    Ok(found)
}

/// Object numbers beyond the document's own objects, and which objects go into object streams.
struct Plan {
    encrypt: Option<u32>,
    /// Indices into [`Reachable::objects`] of the objects that are packed.
    packed: Vec<usize>,
    first_container: u32,
    xref: Option<u32>,
    /// The trailer's `/Size`.
    size: u32,
}

impl Plan {
    fn new(reachable: &Reachable, options: FullOptions) -> Result<Self> {
        let mut next = u32::try_from(reachable.objects.len()).unwrap_or(u32::MAX) + 1;
        let encrypt = (options.encryption == EncryptionPolicy::Keep).then(|| {
            next += 1;
            next - 1
        });
        let packed: Vec<usize> = if options.object_streams {
            reachable
                .objects
                .iter()
                .enumerate()
                .filter(|(_, (_, o))| !matches!(o.kind, ObjectKind::Stream(_)))
                .map(|(i, _)| i)
                .collect()
        } else {
            Vec::new()
        };
        let first_container = next;
        let containers = packed.len().div_ceil(OBJECTS_PER_STREAM);
        next = next.saturating_add(u32::try_from(containers).unwrap_or(u32::MAX));
        let xref = (options.xref_stream || options.object_streams).then(|| {
            next += 1;
            next - 1
        });
        check_number(next)?;
        Ok(Self {
            encrypt,
            packed,
            first_container,
            xref,
            size: next,
        })
    }

    /// The object stream and index of the packed object `index`.
    fn member(&self, index: usize) -> Option<(u32, u32)> {
        let at = self.packed.binary_search(&index).ok()?;
        Some((
            self.first_container + u32::try_from(at / OBJECTS_PER_STREAM).ok()?,
            u32::try_from(at % OBJECTS_PER_STREAM).ok()?,
        ))
    }
}

/// The bytes written so far and the cross-reference entries they define.
struct Output {
    bytes: Vec<u8>,
    entries: Vec<(u32, XrefEntry)>,
}

impl Output {
    fn in_use(&mut self, number: u32) {
        self.entries.push((
            number,
            XrefEntry::InUse {
                offset: self.bytes.len() as u64,
                generation: 0,
            },
        ));
    }
}

/// Writes the document as a new file: the objects reachable from the trailer's `/Root` and
/// `/Info`, renumbered 1..n in discovery order (generation 0), with no earlier revisions.
/// Unreachable objects, free entries and old revisions are gone; **signatures do not survive**
/// (the signed byte range no longer exists).
///
/// - Stream data is copied as stored, still under its filters, so untouched content is not
///   re-encoded. Decryption happens first (see [`EncryptionPolicy`]); a `/Crypt` filter is
///   removed from the chain of a stream written unencrypted.
/// - A reference to a missing object is written as `null` (§7.3.10).
/// - A reference that points to another reference is flattened: the target's content is written
///   under the referring object's number.
/// - A direct `/Info` is dropped (it must be indirect, §14.3.3).
/// - The new `/ID` is the first 16 bytes of a SHA-256 over the output so far, in both slots
///   (with [`EncryptionPolicy::Keep`]: the original `/ID[0]` and that hash).
/// - Output is deterministic.
///
/// # Errors
/// - [`WriteError::NoRoot`] if the document has no `/Root` object,
/// - [`WriteError::NotEncrypted`] for [`EncryptionPolicy::Keep`] on an unencrypted document,
/// - [`EncryptionError::PasswordRequired`] for a locked store,
/// - [`Error::LimitExceeded`](crate::Error::LimitExceeded) if the document has more objects than
///   [`Limits::max_xref_entries`](crate::Limits::max_xref_entries),
/// - and whatever the store reports for an object it cannot read: the write stops rather than
///   silently dropping content.
pub fn write_full(store: &ObjectStore<'_>, options: &FullOptions) -> Result<Vec<u8>> {
    if store.is_locked() {
        return Err(EncryptionError::PasswordRequired.into());
    }
    let keep = options.encryption == EncryptionPolicy::Keep;
    let decryptor = store.decryptor();
    if keep && decryptor.is_none() {
        return Err(WriteError::NotEncrypted.into());
    }
    // Reading the graph can rebuild a damaged cross-reference, and the rebuilt trailer can name
    // another `/Root` or `/Info`: start over once with those (it cannot be rebuilt twice).
    let mut settled = None;
    for _ in 0..2 {
        let (root, info) = trailer_refs(store)?;
        let reachable = discover(
            store,
            VecDeque::from_iter(std::iter::once(root).chain(info)),
        )?;
        let unchanged = trailer_refs(store)? == (root, info);
        settled = Some((root, info, reachable));
        if unchanged {
            break;
        }
    }
    let Some((root, info, reachable)) = settled else {
        return Err(WriteError::NoRoot.into());
    };
    // The repair may only happen while `discover` reads (the store rebuilds lazily), so this has
    // to come after it. Objects inside an unreadable object stream are `null` by now; writing
    // the file would drop them without a trace.
    if let Some(stream) = store
        .repaired()
        .into_iter()
        .find_map(|reason| match reason {
            RepairReason::ObjectStreamNotExpanded { stream } => Some(stream),
            _ => None,
        })
    {
        return Err(WriteError::ObjectStreamLost { stream }.into());
    }
    let Some(&root_number) = reachable.numbers.get(&root.num) else {
        return Err(WriteError::NoRoot.into());
    };
    let info_number = info.and_then(|info| reachable.numbers.get(&info.num).copied());
    let plan = Plan::new(&reachable, *options)?;
    store
        .limits()
        .check(LimitKind::XrefEntries, u64::from(plan.size), None)?;

    let serializers = Serializers {
        remapped: Serializer::new(store.limits()).with_remap(&reachable.numbers),
        stored: Serializer::new(store.limits())
            .with_remap(&reachable.numbers)
            .with_crypt(decryptor.as_deref().filter(|_| keep)),
    };
    let (major, minor) = store.write_base().version;
    let (major, minor) = if plan.xref.is_some() {
        (major, minor).max((1, 5))
    } else {
        (major, minor)
    };
    let mut out = Output {
        bytes: format!("%PDF-{major}.{minor}\n").into_bytes(),
        entries: vec![(
            0,
            XrefEntry::Free {
                next_free: 0,
                generation: u16::MAX,
            },
        )],
    };
    out.bytes.extend_from_slice(b"%\xE2\xE3\xCF\xD3\n");

    write_objects(store, &reachable, &plan, &serializers, &mut out)?;
    write_containers(&reachable, &plan, &serializers, &mut out)?;
    if let Some(number) = plan.encrypt {
        write_encryption_dictionary(store, number, &mut out)?;
    }

    let mut trailer = Dict::default();
    trailer.set(b"Size", integer(u64::from(plan.size)));
    trailer.set(b"Root", reference(root_number));
    if let Some(info) = info_number {
        trailer.set(b"Info", reference(info));
    }
    if let Some(number) = plan.encrypt {
        trailer.set(b"Encrypt", reference(number));
    }
    let fresh = digest16(&[&out.bytes]);
    if keep {
        // /ID[0] belongs to the key, so it stays; a document without /ID keeps having none.
        if let Some((first, _)) = trailer_ids(store)? {
            trailer.set(b"ID", id_array(&first, &fresh));
        }
    } else {
        trailer.set(b"ID", id_array(&fresh, &fresh));
    }
    write_xref(store, &plan, &trailer, &mut out)?;
    Ok(out.bytes)
}

/// The trailer's `/Root` and `/Info` references.
fn trailer_refs(store: &ObjectStore<'_>) -> Result<(ObjRef, Option<ObjRef>)> {
    let Some(root) = store.root_ref() else {
        return Err(WriteError::NoRoot.into());
    };
    let info = match store.trailer_value(b"Info").map(|o| o.kind) {
        Some(ObjectKind::Ref(info)) => Some(info),
        _ => None,
    };
    Ok((root, info))
}

/// The two serializers a rewrite uses.
struct Serializers<'c> {
    /// Renumbers references; writes strings in the clear (object stream members).
    remapped: Serializer<'c>,
    /// Like `remapped`, and encrypts when the document stays encrypted (top-level objects).
    stored: Serializer<'c>,
}

/// Writes every object that is not packed into an object stream.
fn write_objects(
    store: &ObjectStore<'_>,
    reachable: &Reachable,
    plan: &Plan,
    serializers: &Serializers<'_>,
    out: &mut Output,
) -> Result<()> {
    let keep = plan.encrypt.is_some();
    for (index, (original, object)) in reachable.objects.iter().enumerate() {
        let number = u32::try_from(index).unwrap_or(u32::MAX) + 1;
        if let Some((stream, position)) = plan.member(index) {
            out.entries.push((
                number,
                XrefEntry::Compressed {
                    stream,
                    index: position,
                },
            ));
            continue;
        }
        let id = ObjRef::new(number, 0);
        out.in_use(number);
        if let ObjectKind::Stream(stream) = &object.kind {
            let mut dict = stream.dict.clone();
            if !keep {
                strip_crypt_filter(&mut dict);
            }
            // Decrypted but still encoded, so untouched content stays as it was.
            let data = store
                .stream_decrypted(*original)?
                .ok_or(WriteError::StreamWithoutData)?;
            serializers
                .stored
                .stream(&mut out.bytes, id, &dict, &data)?;
        } else {
            serializers.stored.indirect(&mut out.bytes, id, object)?;
        }
    }
    Ok(())
}

/// Writes the object streams (§7.5.7) that hold the packed objects.
fn write_containers(
    reachable: &Reachable,
    plan: &Plan,
    serializers: &Serializers<'_>,
    out: &mut Output,
) -> Result<()> {
    for (chunk_index, chunk) in plan.packed.chunks(OBJECTS_PER_STREAM).enumerate() {
        let container = plan.first_container + u32::try_from(chunk_index).unwrap_or(0);
        let mut header = Vec::new();
        let mut body = Vec::new();
        for &object_index in chunk {
            let Some((_, object)) = reachable.objects.get(object_index) else {
                continue;
            };
            let number = u32::try_from(object_index).unwrap_or(u32::MAX) + 1;
            header.extend_from_slice(format!("{number} {} ", body.len()).as_bytes());
            serializers
                .remapped
                .value(&mut body, object, ObjRef::new(number, 0), 0, false)?;
            body.push(b'\n');
        }
        header.push(b'\n');
        let mut dict = Dict::default();
        dict.set(b"Type", name("ObjStm"));
        dict.set(b"N", integer(chunk.len() as u64));
        dict.set(b"First", integer(header.len() as u64));
        dict.set(b"Filter", name("FlateDecode"));
        header.extend_from_slice(&body);
        out.in_use(container);
        // Encrypted as a whole when the document stays encrypted; the members never are.
        serializers.stored.stream(
            &mut out.bytes,
            ObjRef::new(container, 0),
            &dict,
            &flate_compress(&header),
        )?;
    }
    Ok(())
}

fn name(value: &str) -> Object<'static> {
    Object::new(ObjectKind::Name(value.as_bytes().to_vec().into()))
}

/// The `/Encrypt` dictionary is copied as it is and never encrypted: its strings are the key
/// material.
fn write_encryption_dictionary(
    store: &ObjectStore<'_>,
    number: u32,
    out: &mut Output,
) -> Result<()> {
    let dictionary = store.encrypt()?.ok_or(WriteError::NotEncrypted)?;
    out.in_use(number);
    let no_references = HashMap::new();
    Serializer::new(store.limits())
        .with_remap(&no_references)
        .indirect(&mut out.bytes, ObjRef::new(number, 0), &dictionary)
}

/// The cross-reference section, `startxref` and `%%EOF`.
fn write_xref(
    store: &ObjectStore<'_>,
    plan: &Plan,
    trailer: &Dict<'_>,
    out: &mut Output,
) -> Result<()> {
    let section = out.bytes.len() as u64;
    let serializer = Serializer::new(store.limits());
    if let Some(number) = plan.xref {
        out.in_use(number);
        out.entries.sort_by_key(|&(number, _)| number);
        let (dict, bytes) = xref_stream(&out.entries, trailer);
        serializer.stream(&mut out.bytes, ObjRef::new(number, 0), &dict, &bytes)?;
    } else {
        out.entries.sort_by_key(|&(number, _)| number);
        write_table(&mut out.bytes, &out.entries, trailer, &serializer)?;
    }
    out.bytes
        .extend_from_slice(format!("startxref\n{section}\n%%EOF\n").as_bytes());
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::limits::Limits;
    use crate::parser::Parser;

    fn dict(source: &str) -> Dict<'static> {
        let limits = Limits::default();
        let object = Parser::new(source.as_bytes(), &limits)
            .parse_object()
            .expect("test dictionary parses");
        object.as_dict().expect("a dictionary").clone().into_owned()
    }

    fn written(dict: &Dict<'static>) -> String {
        let mut out = Vec::new();
        crate::write::write_object(
            &mut out,
            &Object::new(ObjectKind::Dict(dict.clone())),
            &Limits::default(),
        )
        .expect("writes");
        String::from_utf8(out).expect("ASCII")
    }

    #[test]
    fn a_lone_crypt_filter_is_removed_with_its_parameters() {
        let mut d = dict("<< /Filter /Crypt /DecodeParms << /Name /Identity >> /Length 3 >>");
        strip_crypt_filter(&mut d);
        assert_eq!(written(&d), "<< /Length 3 >>");
    }

    #[test]
    fn crypt_is_removed_from_a_chain_with_its_parameters_entry() {
        let mut d =
            dict("<< /Filter [/Crypt /FlateDecode] /DecodeParms [<< /Name /Identity >> null] >>");
        strip_crypt_filter(&mut d);
        assert_eq!(
            written(&d),
            "<< /Filter [/FlateDecode] /DecodeParms [null] >>"
        );
    }

    #[test]
    fn a_chain_of_only_crypt_disappears_and_other_filters_are_left_alone() {
        let mut d = dict("<< /Filter [/Crypt] >>");
        strip_crypt_filter(&mut d);
        assert_eq!(written(&d), "<< >>");
        let mut d = dict("<< /Filter /FlateDecode /DecodeParms << /Predictor 12 >> >>");
        let before = written(&d);
        strip_crypt_filter(&mut d);
        assert_eq!(written(&d), before);
    }

    #[test]
    fn references_are_collected_in_document_order_and_a_repeated_key_counts_once() {
        let limits = Limits::default();
        let object = Parser::new(b"<< /A [1 0 R 2 0 R] /B 3 0 R /A [9 0 R] >>", &limits)
            .parse_object()
            .expect("parses");
        let mut found = VecDeque::new();
        collect_refs(&object, &mut found);
        let numbers: Vec<u32> = found.iter().map(|r| r.num).collect();
        assert_eq!(numbers, [3, 9]);
    }
}
