//! The lazy, revision-aware object store.
//!
//! [`ObjectStore::open`] reads only the cross-reference data. An object is parsed the first time
//! it is asked for and then kept in a bounded LRU cache; objects inside object streams are parsed
//! from a decoded stream that is cached the same way. Nothing here walks the document, so opening
//! a file touches no objects at all (the page tree is walked lazily, see [`crate::pages`]).
//!
//! # Behaviour on bad input
//!
//! - **Missing objects are null** (ISO 32000-2 Â§7.3.10): a number with no entry, a free entry,
//!   or an object stream that does not hold the number resolves to `null`, not to an error.
//!   The generation number of a reference is not compared with the cross-reference entry's:
//!   producers disagree about it, and the recovery scan ignores it too.
//! - **Offsets are validated** with [`object_header_at`] before they are trusted. The first time
//!   one is wrong the whole cross-reference is rebuilt by scanning ([`recovery::rebuild`]) with
//!   [`RepairReason::ObjectOffsetInvalid`]; that happens at most once per store, after which a
//!   bad offset just makes that object null. A rebuilt table has no revision history.
//! - **Reference chains are bounded** by [`Limits::max_reference_depth`], which also stops
//!   cycles (`5 0 obj 5 0 R`).
//! - **Everything the parser repairs** (a wrong `/Length`, a missing `endobj`) is recorded in
//!   [`ObjectStore::repaired`].
//!
//! Objects are handed out as `Arc<Object<'static>>`: they own their bytes, so they outlive cache
//! eviction. Spans in a returned object are positions in the file, except for an object that
//! came from an object stream, where they are positions in that stream's decoded data.
//! Stream *data* is not read here; [`ObjectStore::stream_raw`] gives the raw bytes.
//!
//! The store is `Sync`; its caches sit behind one mutex that is never held across a call that
//! could take it again.

use std::collections::{BTreeMap, HashMap};
use std::mem::size_of;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError};

use crate::error::{Error, Result, SyntaxKind};
use crate::limits::{DecodeBudget, LimitKind, Limits};
use crate::object::{Dict, DictEntry, ObjRef, Object, ObjectKind, Stream};
use crate::objstm::ObjectStream;
use crate::parser::Parser;
use crate::recovery::{RepairReason, object_header_at, rebuild};
use crate::xref::{Xref, XrefEntry};

/// Repair reasons kept; a hostile file cannot make the list grow without bound.
const MAX_REPAIR_REASONS: usize = 1024;

/// A least-recently-used cache keyed by object number, bounded by an estimated byte weight.
struct Lru<V> {
    entries: HashMap<u32, LruEntry<V>>,
    /// Recency: tick to key, oldest first.
    order: BTreeMap<u64, u32>,
    tick: u64,
    bytes: usize,
    capacity: usize,
}

struct LruEntry<V> {
    value: V,
    tick: u64,
    weight: usize,
}

impl<V: Clone> Lru<V> {
    fn new(capacity: u64) -> Self {
        Self {
            entries: HashMap::new(),
            order: BTreeMap::new(),
            tick: 0,
            bytes: 0,
            capacity: usize::try_from(capacity).unwrap_or(usize::MAX),
        }
    }

    fn get(&mut self, key: u32) -> Option<V> {
        let entry = self.entries.get_mut(&key)?;
        self.order.remove(&entry.tick);
        self.tick += 1;
        entry.tick = self.tick;
        self.order.insert(self.tick, key);
        Some(entry.value.clone())
    }

    /// Inserts a value; one heavier than the whole cache is not kept.
    fn insert(&mut self, key: u32, value: V, weight: usize) {
        self.remove(key);
        if weight > self.capacity {
            return;
        }
        while self.bytes + weight > self.capacity {
            let Some((_, oldest)) = self.order.pop_first() else {
                break;
            };
            if let Some(evicted) = self.entries.remove(&oldest) {
                self.bytes -= evicted.weight;
            }
        }
        self.tick += 1;
        self.order.insert(self.tick, key);
        self.bytes += weight;
        self.entries.insert(
            key,
            LruEntry {
                value,
                tick: self.tick,
                weight,
            },
        );
    }

    fn remove(&mut self, key: u32) {
        if let Some(old) = self.entries.remove(&key) {
            self.order.remove(&old.tick);
            self.bytes -= old.weight;
        }
    }

    fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.bytes = 0;
    }
}

/// Estimated memory held by a parsed object. Recursion is bounded by the parser's nesting limit.
fn weight(object: &Object<'_>) -> usize {
    size_of::<Object<'static>>()
        + match &object.kind {
            ObjectKind::String(bytes) | ObjectKind::Name(bytes) => bytes.len(),
            ObjectKind::Array(items) => items.iter().map(weight).sum(),
            ObjectKind::Dict(dict) => dict_weight(dict),
            ObjectKind::Stream(stream) => dict_weight(&stream.dict),
            _ => 0,
        }
}

fn dict_weight(dict: &Dict<'_>) -> usize {
    dict.entries
        .iter()
        .map(|e| size_of::<DictEntry<'static>>() + e.key.len() + weight(&e.value))
        .sum()
}

/// What the free functions below need from the store.
#[derive(Clone, Copy)]
struct Env<'e, 'a> {
    data: &'a [u8],
    limits: &'e Limits,
}

fn malformed_object_stream() -> Error {
    Error::Syntax {
        kind: SyntaxKind::MalformedObjectStream,
        offset: 0,
    }
}

fn null() -> Object<'static> {
    Object {
        kind: ObjectKind::Null,
        span: 0..0,
    }
}

/// The mutable part of the store.
struct State<'a> {
    xref: Xref<'a>,
    /// Effective entry per object number ([`Xref::merged`]), built on first use because looking
    /// up in the revision lists is linear.
    index: Option<HashMap<u32, XrefEntry>>,
    budget: DecodeBudget,
    objects: Lru<Arc<Object<'static>>>,
    streams: Lru<Arc<ObjectStream>>,
    repaired: Vec<RepairReason>,
    rebuild_attempted: bool,
    /// Objects fetched from the file or an object stream so far (cache misses).
    loaded: u64,
}

impl<'a> State<'a> {
    fn note(&mut self, reason: RepairReason) {
        if self.repaired.len() < MAX_REPAIR_REASONS && !self.repaired.contains(&reason) {
            self.repaired.push(reason);
        }
    }

    fn ensure_index(&mut self) {
        if self.index.is_none() {
            self.index = Some(self.xref.merged());
        }
    }

    fn entry(&mut self, number: u32) -> Option<XrefEntry> {
        self.ensure_index();
        self.index.as_ref()?.get(&number).copied()
    }

    /// The object `number`, from the cache or the file.
    fn load(&mut self, env: Env<'_, 'a>, number: u32) -> Result<Arc<Object<'static>>> {
        if let Some(cached) = self.objects.get(number) {
            return Ok(cached);
        }
        self.loaded += 1;
        let object = Arc::new(self.load_uncached(env, number)?);
        let size = weight(&object);
        self.objects.insert(number, Arc::clone(&object), size);
        Ok(object)
    }

    fn load_uncached(&mut self, env: Env<'_, 'a>, number: u32) -> Result<Object<'static>> {
        match self.entry(number) {
            None | Some(XrefEntry::Free { .. }) => Ok(null()),
            Some(XrefEntry::InUse { offset, .. }) => self.load_in_use(env, number, offset),
            Some(XrefEntry::Compressed { stream, index }) => {
                self.load_compressed(env, number, stream, index)
            }
        }
    }

    fn load_in_use(
        &mut self,
        env: Env<'_, 'a>,
        number: u32,
        offset: u64,
    ) -> Result<Object<'static>> {
        let base = self.xref.base;
        if !object_header_at(env.data, base, offset, number) {
            self.note(RepairReason::ObjectOffsetInvalid { number });
            if self.rebuild_attempted {
                return Ok(null());
            }
            self.rebuild_xref(env)?;
            // The rebuilt table decides; it cannot rebuild again, so this ends.
            return self.load_uncached(env, number);
        }
        // `object_header_at` accepted `base + offset`, so it is a valid position.
        let position = usize::try_from(offset).unwrap_or(0).saturating_add(base);

        self.ensure_index();
        let empty = HashMap::new();
        let index = self.index.as_ref().unwrap_or(&empty);
        let resolver = |reference: ObjRef| length_of(env, base, index, reference);
        let indirect = Parser::at(env.data, position, env.limits)
            .with_length_resolver(&resolver)
            .parse_indirect_object()?;
        for &recovery in &indirect.recoveries {
            self.note(RepairReason::ObjectRecovered { number, recovery });
        }
        Ok(indirect.object.into_owned())
    }

    /// Replaces the cross-reference with one built by scanning the file, keeping the reasons
    /// collected so far. Only limit errors are reported; if the file cannot be rebuilt the old
    /// table stays and the object that triggered this is just missing.
    fn rebuild_xref(&mut self, env: Env<'_, 'a>) -> Result<()> {
        self.rebuild_attempted = true;
        match rebuild(env.data, env.limits, &mut self.budget, &self.repaired) {
            Ok(rebuilt) => {
                self.repaired.clone_from(&rebuilt.repaired);
                self.xref = rebuilt;
                self.index = None;
                self.objects.clear();
                self.streams.clear();
                Ok(())
            }
            Err(error @ Error::LimitExceeded { .. }) => Err(error),
            Err(_) => Ok(()),
        }
    }

    fn load_compressed(
        &mut self,
        env: Env<'_, 'a>,
        number: u32,
        stream: u32,
        index: u32,
    ) -> Result<Object<'static>> {
        let container = self.object_stream(env, stream)?;
        // The cross-reference gives an index, but the stream's own header is what says which
        // object is where: trust the number, and use the index only as the first guess.
        let at = usize::try_from(index)
            .ok()
            .filter(|&i| container.object_number(i) == Some(number))
            .or_else(|| container.index_of(number));
        match at {
            Some(i) => Ok(container.object(i, env.limits)?.into_owned()),
            None => Ok(null()),
        }
    }

    fn object_stream(&mut self, env: Env<'_, 'a>, number: u32) -> Result<Arc<ObjectStream>> {
        if let Some(cached) = self.streams.get(number) {
            return Ok(cached);
        }
        // An object stream must itself be an ordinary indirect object (Â§7.5.7); this also keeps
        // the recursion through `load` one level deep.
        if !matches!(self.entry(number), Some(XrefEntry::InUse { .. })) {
            return Err(malformed_object_stream());
        }
        let container = self.load(env, number)?;
        let ObjectKind::Stream(Stream { dict, data }) = &container.kind else {
            return Err(malformed_object_stream());
        };
        let raw = env.data.get(data.clone()).ok_or(Error::OutOfRange {
            start: data.start as u64,
            end: data.end as u64,
            len: env.data.len() as u64,
        })?;
        let decoded = ObjectStream::from_stream(
            dict,
            raw,
            env.limits,
            Some(&mut self.budget),
            Some(data.start as u64),
        )?;
        let size = decoded.decoded_len();
        let decoded = Arc::new(decoded);
        self.streams.insert(number, Arc::clone(&decoded), size);
        Ok(decoded)
    }
}

/// Reads an indirect `/Length` for the parser: the integer object `reference` names, if it is a
/// plain in-use object. A length inside an object stream, or a length that is itself a
/// reference, is "unresolved" and the parser finds the stream end by searching.
fn length_of(
    env: Env<'_, '_>,
    base: usize,
    index: &HashMap<u32, XrefEntry>,
    reference: ObjRef,
) -> Option<i64> {
    let Some(&XrefEntry::InUse { offset, .. }) = index.get(&reference.num) else {
        return None;
    };
    if !object_header_at(env.data, base, offset, reference.num) {
        return None;
    }
    let position = usize::try_from(offset).ok()?.checked_add(base)?;
    let indirect = Parser::at(env.data, position, env.limits)
        .parse_indirect_object()
        .ok()?;
    indirect.object.as_integer()
}

/// A PDF file's objects, parsed lazily. See the [module documentation](self).
pub struct ObjectStore<'a> {
    data: &'a [u8],
    limits: Limits,
    state: Mutex<State<'a>>,
}

impl<'a> ObjectStore<'a> {
    /// Opens `data`: reads the cross-reference chain (rebuilding it if it is damaged, see
    /// [`Xref::open`]) and nothing else.
    ///
    /// # Errors
    /// What [`Xref::open`] reports when the file cannot be read or rebuilt.
    pub fn open(data: &'a [u8], limits: Limits) -> Result<Self> {
        let mut budget = DecodeBudget::new(&limits);
        let xref = Xref::open_with_budget(data, &limits, &mut budget)?;
        let state = State {
            repaired: xref.repaired.clone(),
            xref,
            index: None,
            budget,
            objects: Lru::new(limits.max_cache_bytes),
            streams: Lru::new(limits.max_cache_bytes),
            rebuild_attempted: false,
            loaded: 0,
        };
        Ok(Self {
            data,
            limits,
            state: Mutex::new(state),
        })
    }

    fn lock(&self) -> MutexGuard<'_, State<'a>> {
        // A poisoned lock means another thread panicked; the caches stay consistent because
        // every update is a single insert or remove, so keep going.
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn env(&self) -> Env<'_, 'a> {
        Env {
            data: self.data,
            limits: &self.limits,
        }
    }

    /// The file's bytes.
    #[must_use]
    pub fn data(&self) -> &'a [u8] {
        self.data
    }

    /// The limits this store was opened with.
    #[must_use]
    pub fn limits(&self) -> &Limits {
        &self.limits
    }

    /// Why the file counts as repaired; empty if it was read as it is. Grows as objects are
    /// loaded and found damaged.
    #[must_use]
    pub fn repaired(&self) -> Vec<RepairReason> {
        self.lock().repaired.clone()
    }

    /// How many objects have been read from the file (or an object stream) so far, counting a
    /// cache miss each time. Opening a file leaves this at 0, which is how tests check that
    /// nothing is read eagerly.
    #[must_use]
    pub fn objects_loaded(&self) -> u64 {
        self.lock().loaded
    }

    /// Resolves a reference to its object, following objects that are themselves references.
    /// A missing object is `null` (Â§7.3.10).
    ///
    /// # Errors
    /// [`Error::LimitExceeded`] for a reference chain longer than
    /// [`Limits::max_reference_depth`] (including cycles), a limit hit while decoding an object
    /// stream, [`SyntaxKind::MalformedObjectStream`] if `/ObjStm` is not usable, and the parser's
    /// error for an object that cannot be parsed.
    pub fn resolve(&self, reference: ObjRef) -> Result<Arc<Object<'static>>> {
        let env = self.env();
        let mut state = self.lock();
        let mut current = reference;
        for _ in 0..=self.limits.max_reference_depth {
            let object = state.load(env, current.num)?;
            match object.kind {
                ObjectKind::Ref(next) => current = next,
                _ => return Ok(object),
            }
        }
        let max = u64::from(self.limits.max_reference_depth);
        Err(Error::LimitExceeded {
            limit: LimitKind::ReferenceDepth,
            max,
            value: max + 1,
            offset: None,
        })
    }

    /// Like [`resolve`](Self::resolve) for any object: a reference is followed, anything else is
    /// returned as an owned copy.
    ///
    /// # Errors
    /// As [`resolve`](Self::resolve).
    pub fn deref(&self, object: &Object<'_>) -> Result<Arc<Object<'static>>> {
        match object.kind {
            ObjectKind::Ref(reference) => self.resolve(reference),
            _ => Ok(Arc::new(object.clone().into_owned())),
        }
    }

    /// The raw bytes of a stream as stored in the file: still encoded and, in an encrypted
    /// document, still encrypted. `stream` must come from this store.
    ///
    /// # Errors
    /// [`Error::OutOfRange`] if the stream's range is not inside the file.
    pub fn stream_raw(&self, stream: &Stream<'_>) -> Result<&'a [u8]> {
        self.data.get(stream.data.clone()).ok_or(Error::OutOfRange {
            start: stream.data.start as u64,
            end: stream.data.end as u64,
            len: self.data.len() as u64,
        })
    }

    /// The trailer of the newest revision (for a rebuilt file, the one chosen by the scan).
    #[must_use]
    pub fn trailer(&self) -> Dict<'static> {
        self.lock()
            .xref
            .trailer()
            .map(|trailer| trailer.clone().into_owned())
            .unwrap_or_default()
    }

    /// The trailer value for `key`, from the newest revision that has one: a damaged file can
    /// lose `/Info` or `/Encrypt` from its newest trailer.
    fn trailer_value(&self, key: &[u8]) -> Option<Object<'static>> {
        self.lock()
            .xref
            .revisions
            .iter()
            .find_map(|r| r.section.trailer.get(key))
            .map(|value| value.clone().into_owned())
    }

    /// The reference in the trailer's `/Root`.
    #[must_use]
    pub fn root_ref(&self) -> Option<ObjRef> {
        match self.trailer_value(b"Root")?.kind {
            ObjectKind::Ref(reference) => Some(reference),
            _ => None,
        }
    }

    /// The trailer entry `key`, resolved; `None` if it is absent or `null`.
    fn trailer_object(&self, key: &[u8]) -> Result<Option<Arc<Object<'static>>>> {
        let Some(value) = self.trailer_value(key) else {
            return Ok(None);
        };
        let object = self.deref(&value)?;
        Ok(match object.kind {
            ObjectKind::Null => None,
            _ => Some(object),
        })
    }

    /// The document catalog (`/Root`), or `None` if there is none.
    ///
    /// # Errors
    /// As [`resolve`](Self::resolve).
    pub fn root(&self) -> Result<Option<Arc<Object<'static>>>> {
        self.trailer_object(b"Root")
    }

    /// The document information dictionary (`/Info`), or `None`.
    ///
    /// # Errors
    /// As [`resolve`](Self::resolve).
    pub fn info(&self) -> Result<Option<Arc<Object<'static>>>> {
        self.trailer_object(b"Info")
    }

    /// The encryption dictionary (`/Encrypt`, direct or indirect), or `None` for an unencrypted
    /// file. Strings and streams are **not** decrypted yet (M0 task 11).
    ///
    /// # Errors
    /// As [`resolve`](Self::resolve).
    pub fn encrypt(&self) -> Result<Option<Arc<Object<'static>>>> {
        self.trailer_object(b"Encrypt")
    }

    /// Iterates the pages in document order, reading the page tree lazily (see
    /// [`crate::pages`]).
    #[must_use]
    pub fn pages(&self) -> crate::pages::Pages<'_, 'a> {
        crate::pages::Pages::new(self)
    }
}

impl std::fmt::Debug for ObjectStore<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ObjectStore")
            .field("len", &self.data.len())
            .finish_non_exhaustive()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lru_evicts_the_least_recently_used_first() {
        let mut lru = Lru::new(30);
        lru.insert(1, 'a', 10);
        lru.insert(2, 'b', 10);
        lru.insert(3, 'c', 10);
        // Touch 1, so 2 is now the oldest.
        assert_eq!(lru.get(1), Some('a'));
        lru.insert(4, 'd', 10);
        assert_eq!(lru.get(2), None);
        assert_eq!(lru.get(1), Some('a'));
        assert_eq!(lru.get(3), Some('c'));
        assert_eq!(lru.get(4), Some('d'));
        assert_eq!(lru.bytes, 30);
    }

    #[test]
    fn lru_enforces_the_byte_budget() {
        let mut lru = Lru::new(100);
        for key in 0..1000 {
            lru.insert(key, key, 7);
            assert!(lru.bytes <= 100, "{} bytes after {key}", lru.bytes);
        }
        assert_eq!(lru.entries.len(), 100 / 7);
        assert_eq!(lru.entries.len(), lru.order.len());
    }

    #[test]
    fn lru_skips_a_value_heavier_than_the_cache_and_keeps_the_rest() {
        let mut lru = Lru::new(20);
        lru.insert(1, 'a', 10);
        lru.insert(2, 'b', 21);
        assert_eq!(lru.get(2), None);
        assert_eq!(lru.get(1), Some('a'));
        assert_eq!(lru.bytes, 10);
    }

    #[test]
    fn lru_replacing_a_key_does_not_leak_weight() {
        let mut lru = Lru::new(100);
        lru.insert(1, 'a', 40);
        lru.insert(1, 'b', 10);
        assert_eq!(lru.bytes, 10);
        assert_eq!(lru.get(1), Some('b'));
        lru.clear();
        assert_eq!((lru.bytes, lru.entries.len(), lru.order.len()), (0, 0, 0));
    }

    #[test]
    fn weight_grows_with_the_object() {
        let small = Object {
            kind: ObjectKind::Integer(1),
            span: 0..1,
        };
        let big = Object {
            kind: ObjectKind::Array(vec![small.clone(); 100]),
            span: 0..200,
        };
        assert!(weight(&big) >= 100 * weight(&small));
    }
}
