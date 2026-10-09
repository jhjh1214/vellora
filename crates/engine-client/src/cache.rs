//! The UI-side tile cache: a slot allocator over the shared-memory region plus an LRU index.
//!
//! Every tile lives in one slot of the region the engine renders into, so the cache decides which
//! slot a request may write and which finished tile to give up when it needs room. It holds no
//! pixels and does no I/O; the caller reads a hit with [`crate::Client::read_slot`].
//!
//! # Life of an entry
//!
//! 1. [`TileCache::reserve`] claims a slot for a key (evicting the least recently used finished
//!    tiles if the budget or the slot supply demands it). The entry is *pending*: the engine may
//!    be writing the slot, so it is never evicted and never readable.
//! 2. [`TileCache::complete`] (on `TileReady`) makes it *ready*: readable, most recently used,
//!    evictable. [`TileCache::abandon`] (cancel, request error, crash) drops a pending entry.
//! 3. [`TileCache::get`] returns a ready tile's slot and marks it most recently used.
//!
//! # Budget
//!
//! The budget counts the bytes of tiles (`width * height * 4`) that are pending or ready, so a
//! page edge made of small tiles costs less than a full slot. A slot is the unit of allocation,
//! though, so the region also bounds the number of tiles: the cache never holds more entries than
//! the geometry has slots.
//!
//! # Slot reuse
//!
//! A cancelled request gets no answer, and the engine may still write its slot until its next
//! response (task 18). Reuse is safe as long as nobody reads a slot before the new request's
//! `TileReady`, which [`TileCache::get`] guarantees by only returning ready entries. Freed slots
//! also go to the back of the free list, so a slot is the last one to be handed out again.
//!
//! # Invalidation
//!
//! [`TileCache::invalidate_page`] drops a page's ready tiles at once. Its pending tiles keep their
//! slot until the engine answers (it is still writing), and [`TileCache::complete`] then frees the
//! slot instead of making the stale pixels readable.

use std::collections::{HashMap, VecDeque};

use thiserror::Error;
use vellora_ipc::SlotId;
use vellora_shm::SlotGeometry;

/// Default tile cache budget: 256 MiB (`docs/architecture/performance-targets.md`).
pub const DEFAULT_BUDGET_BYTES: u64 = 256 << 20;

/// Default budget of the thumbnail cache: 64 MiB, kept apart from the tile budget so that a
/// fling through the sidebar never evicts the tiles of the page being read.
pub const DEFAULT_THUMBNAIL_BUDGET_BYTES: u64 = 64 << 20;

/// Bytes of one slot in a region sized for the cache: a 512 x 512 tile of 4-byte pixels.
pub const SLOT_BYTES: u32 = 1 << 20;

/// Quarter-octave steps per doubling of the scale (a bucket spans a factor of about 1.19).
const BUCKETS_PER_OCTAVE: f32 = 4.0;

/// Scale quantised to a bucket, so that zooming by a hair reuses tiles instead of re-rendering
/// everything.
///
/// The cache only works if the caller renders at [`ScaleBucket::scale`], the bucket's canonical
/// scale, not at the scale it was asked for: two requests in one bucket then produce identical
/// tiles.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ScaleBucket(i16);

impl ScaleBucket {
    /// The bucket nearest to `scale` on a log scale, or `None` for a scale that is not finite and
    /// positive.
    #[must_use]
    pub fn from_scale(scale: f32) -> Option<Self> {
        if !scale.is_finite() || scale <= 0.0 {
            return None;
        }
        // The engine refuses scales above 64 (2^6), so valid buckets are far inside `i16`; the
        // clamp only keeps absurdly small scales from overflowing the cast.
        let steps = (scale.log2() * BUCKETS_PER_OCTAVE)
            .round()
            .clamp(-1000.0, 1000.0);
        #[allow(clippy::cast_possible_truncation)]
        Some(Self(steps as i16))
    }

    /// The scale tiles of this bucket are rendered at.
    #[must_use]
    pub fn scale(self) -> f32 {
        (f32::from(self.0) / BUCKETS_PER_OCTAVE).exp2()
    }
}

/// What identifies a tile: the page, the scale bucket and the tile's position in the grid of
/// tiles at that scale (not pixels).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TileKey {
    /// Zero-based page index.
    pub page: u32,
    /// Zoom level.
    pub scale: ScaleBucket,
    /// Tile column.
    pub x: u32,
    /// Tile row.
    pub y: u32,
}

/// Why [`TileCache::reserve`] could not hand out a slot.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Error)]
pub enum ReserveError {
    /// The key is already pending or ready; use [`TileCache::get`].
    #[error("the tile is already in the cache")]
    Present,
    /// The tile does not fit a slot, or alone exceeds the budget.
    #[error("a tile of {0} bytes cannot be cached")]
    TooLarge(u64),
    /// Every slot, or every byte of the budget, is held by requests still in flight. The caller
    /// asks again after some of them finish.
    #[error("all cache capacity is held by requests in flight")]
    Full,
}

#[derive(Debug)]
enum State {
    /// The engine owns the slot. `stale` is set when the page was invalidated meanwhile.
    Pending { stale: bool },
    /// Finished; `tick` orders recency (larger is more recent).
    Ready { tick: u64 },
}

#[derive(Debug)]
struct Entry {
    slot: SlotId,
    bytes: u64,
    state: State,
}

/// Slot allocator and LRU index over one region. See the module docs.
#[derive(Debug)]
pub struct TileCache {
    slot_bytes: u64,
    budget: u64,
    used: u64,
    entries: HashMap<TileKey, Entry>,
    /// Ready entries, oldest first. Touching a tile moves it to the back; the front is evicted
    /// first. Linear scans are fine at a few hundred entries (256 slots at the default).
    recency: VecDeque<TileKey>,
    free: VecDeque<SlotId>,
    tick: u64,
}

impl TileCache {
    /// A cache over a region of `geometry` that keeps at most `budget_bytes` of tiles.
    #[must_use]
    pub fn new(geometry: SlotGeometry, budget_bytes: u64) -> Self {
        Self::over_slots(geometry, 0..geometry.slot_count(), budget_bytes)
    }

    /// A cache that uses only the slots `slots` of a region of `geometry`, so that two caches can
    /// share one region without ever handing out the same slot (the tile cache and the thumbnail
    /// cache).
    #[must_use]
    pub fn over_slots(
        geometry: SlotGeometry,
        slots: std::ops::Range<u32>,
        budget_bytes: u64,
    ) -> Self {
        Self {
            slot_bytes: u64::from(geometry.slot_bytes()),
            budget: budget_bytes,
            used: 0,
            entries: HashMap::new(),
            recency: VecDeque::new(),
            free: slots.map(SlotId).collect(),
            tick: 0,
        }
    }

    /// The region geometry that gives a cache of `budget_bytes` room for its budget: as many
    /// [`SLOT_BYTES`] slots as the budget holds (at least one), capped by the region limit.
    ///
    /// # Errors
    ///
    /// [`vellora_shm::Error::InvalidGeometry`] never in practice for the capped value; the type
    /// is the constructor's.
    pub fn geometry_for_budget(budget_bytes: u64) -> Result<SlotGeometry, vellora_shm::Error> {
        let max = vellora_shm::MAX_REGION_BYTES / u64::from(SLOT_BYTES);
        let slots = (budget_bytes / u64::from(SLOT_BYTES)).clamp(1, max);
        // `slots <= 1024`, so the cast cannot truncate.
        #[allow(clippy::cast_possible_truncation)]
        SlotGeometry::new(slots as u32, SLOT_BYTES)
    }

    /// Bytes of tiles held, pending and ready.
    #[must_use]
    pub fn used_bytes(&self) -> u64 {
        self.used
    }

    /// The budget this cache enforces.
    #[must_use]
    pub fn budget_bytes(&self) -> u64 {
        self.budget
    }

    /// Number of tiles held, pending and ready.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// No tiles held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// The slot of a ready tile, which becomes the most recently used. Pending tiles are not
    /// readable and return `None`, like absent ones.
    pub fn get(&mut self, key: &TileKey) -> Option<SlotId> {
        let entry = self.entries.get_mut(key)?;
        let State::Ready { tick } = &mut entry.state else {
            return None;
        };
        self.tick += 1;
        *tick = self.tick;
        let slot = entry.slot;
        if let Some(at) = self.recency.iter().position(|k| k == key) {
            self.recency.remove(at);
        }
        self.recency.push_back(*key);
        Some(slot)
    }

    /// Whether the key is pending or ready, without touching recency.
    #[must_use]
    pub fn contains(&self, key: &TileKey) -> bool {
        self.entries.contains_key(key)
    }

    /// Claims a slot for a tile of `bytes` bytes (`width * height * 4`) that the caller is about
    /// to request. Evicts least recently used ready tiles until the budget and a free slot allow
    /// it.
    ///
    /// # Errors
    ///
    /// [`ReserveError`]; nothing is evicted when it fails.
    pub fn reserve(&mut self, key: TileKey, bytes: u64) -> Result<SlotId, ReserveError> {
        if self.entries.contains_key(&key) {
            return Err(ReserveError::Present);
        }
        if bytes == 0 || bytes > self.slot_bytes || bytes > self.budget {
            return Err(ReserveError::TooLarge(bytes));
        }
        // Decide how many tiles must go before touching anything, so a failure evicts nothing.
        let mut freed_bytes = 0;
        let mut freed_slots = 0;
        let mut victims = 0;
        while self.used - freed_bytes + bytes > self.budget || self.free.len() + freed_slots == 0 {
            let Some(victim) = self.recency.get(victims) else {
                return Err(ReserveError::Full);
            };
            freed_bytes += self.entries.get(victim).map_or(0, |e| e.bytes);
            freed_slots += 1;
            victims += 1;
        }
        for _ in 0..victims {
            if let Some(victim) = self.recency.pop_front() {
                self.release(&victim);
            }
        }
        // The loop above guarantees a free slot.
        let slot = self.free.pop_front().ok_or(ReserveError::Full)?;
        self.used += bytes;
        self.entries.insert(
            key,
            Entry {
                slot,
                bytes,
                state: State::Pending { stale: false },
            },
        );
        Ok(slot)
    }

    /// The engine answered for `key`: its pixels are in the slot. Returns `true` if the tile is
    /// now readable, `false` if it was invalidated meanwhile (the slot is freed) or was not
    /// pending.
    pub fn complete(&mut self, key: &TileKey) -> bool {
        match self.entries.get(key).map(|e| &e.state) {
            Some(State::Pending { stale: false }) => {}
            Some(State::Pending { stale: true }) => {
                self.release(key);
                return false;
            }
            _ => return false,
        }
        self.tick += 1;
        if let Some(entry) = self.entries.get_mut(key) {
            entry.state = State::Ready { tick: self.tick };
        }
        self.recency.push_back(*key);
        true
    }

    /// Drops a pending tile whose request was cancelled, failed or lost in a crash. Returns
    /// whether there was one; ready tiles are left alone.
    pub fn abandon(&mut self, key: &TileKey) -> bool {
        if matches!(
            self.entries.get(key).map(|e| &e.state),
            Some(State::Pending { .. })
        ) {
            self.release(key);
            true
        } else {
            false
        }
    }

    /// Drops every pending tile, for after an engine crash (their answers will never come).
    /// Returns how many there were. Ready tiles stay: the engine writes a slot only while its
    /// request is pending.
    pub fn abandon_pending(&mut self) -> usize {
        let pending: Vec<TileKey> = self
            .entries
            .iter()
            .filter(|(_, e)| matches!(e.state, State::Pending { .. }))
            .map(|(k, _)| *k)
            .collect();
        for key in &pending {
            self.release(key);
        }
        pending.len()
    }

    /// Forgets everything known about `page` (it changed, or was closed): ready tiles are freed
    /// now, pending ones when the engine answers. Returns the number of ready tiles freed.
    pub fn invalidate_page(&mut self, page: u32) -> usize {
        let mut ready = Vec::new();
        for (key, entry) in &mut self.entries {
            if key.page != page {
                continue;
            }
            match &mut entry.state {
                State::Pending { stale } => *stale = true,
                State::Ready { .. } => ready.push(*key),
            }
        }
        for key in &ready {
            self.release(key);
        }
        ready.len()
    }

    /// Removes an entry and returns its slot to the back of the free list.
    fn release(&mut self, key: &TileKey) {
        if let Some(entry) = self.entries.remove(key) {
            self.used -= entry.bytes;
            self.free.push_back(entry.slot);
            if let Some(at) = self.recency.iter().position(|k| k == key) {
                self.recency.remove(at);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(page: u32, x: u32) -> TileKey {
        TileKey {
            page,
            scale: ScaleBucket::from_scale(1.0).expect("valid scale"),
            x,
            y: 0,
        }
    }

    /// Four 1 KiB slots.
    fn cache(budget: u64) -> TileCache {
        TileCache::new(SlotGeometry::new(4, 1024).expect("geometry"), budget)
    }

    fn fill(cache: &mut TileCache, k: TileKey, bytes: u64) -> SlotId {
        let slot = cache.reserve(k, bytes).expect("reserve");
        assert!(cache.complete(&k));
        slot
    }

    #[test]
    fn evicts_in_least_recently_used_order() {
        let mut c = cache(1 << 20);
        for x in 0..4 {
            fill(&mut c, key(0, x), 1024);
        }
        // Touch 0 and 1: 2 is now the oldest, then 3.
        assert!(c.get(&key(0, 0)).is_some());
        assert!(c.get(&key(0, 1)).is_some());
        fill(&mut c, key(0, 4), 1024);
        assert!(!c.contains(&key(0, 2)));
        assert!(c.contains(&key(0, 3)));
        fill(&mut c, key(0, 5), 1024);
        assert!(!c.contains(&key(0, 3)));
        for x in [0, 1, 4, 5] {
            assert!(c.contains(&key(0, x)), "tile {x} should have survived");
        }
    }

    #[test]
    fn eviction_reuses_the_evicted_slot_last_in_line() {
        let mut c = cache(1 << 20);
        let slots: Vec<SlotId> = (0..4).map(|x| fill(&mut c, key(0, x), 1024)).collect();
        // Full: the new tile takes the slot of the oldest one.
        let reused = c.reserve(key(0, 9), 1024).expect("reserve");
        assert_eq!(reused, slots[0]);
    }

    #[test]
    fn budget_in_bytes_is_enforced_before_the_slot_count() {
        // Room for 2.5 slots of bytes: small tiles fit more than the budget-in-slots would say.
        let mut c = cache(2560);
        fill(&mut c, key(0, 0), 1024);
        fill(&mut c, key(0, 1), 1024);
        fill(&mut c, key(0, 2), 512);
        assert_eq!(c.used_bytes(), 2560);
        // One more byte-sized tile forces the oldest out even though a slot is free.
        fill(&mut c, key(0, 3), 1024);
        assert!(!c.contains(&key(0, 0)));
        assert_eq!(c.used_bytes(), 2560);
        assert!(c.used_bytes() <= c.budget_bytes());
    }

    #[test]
    fn used_bytes_never_exceeds_the_budget_under_churn() {
        let mut c = cache(3000);
        for x in 0..200_u32 {
            let bytes = 256 + u64::from(x % 4) * 256;
            let k = key(x % 3, x);
            if c.reserve(k, bytes).is_ok() && x % 5 != 0 {
                c.complete(&k);
            }
            assert!(c.used_bytes() <= 3000);
            assert!(c.len() <= 4);
        }
    }

    #[test]
    fn pending_tiles_are_never_evicted_and_never_readable() {
        let mut c = cache(1 << 20);
        for x in 0..4 {
            c.reserve(key(0, x), 1024).expect("reserve");
        }
        assert_eq!(c.get(&key(0, 0)), None);
        assert_eq!(c.reserve(key(0, 9), 1024), Err(ReserveError::Full));
        assert_eq!(c.len(), 4);
        // One finishes: it becomes the only candidate.
        assert!(c.complete(&key(0, 2)));
        c.reserve(key(0, 9), 1024).expect("room after completion");
        assert!(!c.contains(&key(0, 2)));
        assert!(c.contains(&key(0, 0)));
    }

    #[test]
    fn a_failed_reserve_evicts_nothing() {
        let mut c = cache(2048);
        fill(&mut c, key(0, 0), 512);
        c.reserve(key(0, 1), 1024).expect("reserve");
        c.reserve(key(0, 2), 512).expect("reserve");
        // Evicting the one ready tile frees 512 bytes, not the 1024 needed: nothing may go.
        assert_eq!(c.reserve(key(0, 3), 1024), Err(ReserveError::Full));
        assert_eq!(c.get(&key(0, 0)).map(|_| ()), Some(()));
        assert_eq!(c.used_bytes(), 2048);
        assert_eq!(c.len(), 3);
    }

    #[test]
    fn refuses_duplicates_and_tiles_that_cannot_fit() {
        let mut c = cache(2048);
        c.reserve(key(0, 0), 512).expect("reserve");
        assert_eq!(c.reserve(key(0, 0), 512), Err(ReserveError::Present));
        assert_eq!(c.reserve(key(0, 1), 0), Err(ReserveError::TooLarge(0)));
        assert_eq!(
            c.reserve(key(0, 1), 1025),
            Err(ReserveError::TooLarge(1025))
        );
        let mut small = cache(512);
        assert_eq!(
            small.reserve(key(0, 1), 1024),
            Err(ReserveError::TooLarge(1024))
        );
    }

    #[test]
    fn invalidate_page_frees_ready_tiles_and_keeps_other_pages() {
        let mut c = cache(1 << 20);
        fill(&mut c, key(1, 0), 1024);
        fill(&mut c, key(1, 1), 1024);
        fill(&mut c, key(2, 0), 1024);
        assert_eq!(c.invalidate_page(1), 2);
        assert!(!c.contains(&key(1, 0)));
        assert!(c.contains(&key(2, 0)));
        assert_eq!(c.used_bytes(), 1024);
        assert_eq!(c.get(&key(1, 1)), None);
    }

    #[test]
    fn a_pending_tile_of_an_invalidated_page_is_dropped_when_it_completes() {
        let mut c = cache(1 << 20);
        c.reserve(key(1, 0), 1024).expect("reserve");
        assert_eq!(c.invalidate_page(1), 0);
        // The slot is still the engine's until it answers.
        assert!(c.contains(&key(1, 0)));
        assert!(!c.complete(&key(1, 0)));
        assert!(!c.contains(&key(1, 0)));
        assert_eq!(c.get(&key(1, 0)), None);
        assert_eq!(c.used_bytes(), 0);
    }

    #[test]
    fn abandon_frees_pending_tiles_only() {
        let mut c = cache(1 << 20);
        c.reserve(key(0, 0), 1024).expect("reserve");
        fill(&mut c, key(0, 1), 1024);
        assert!(c.abandon(&key(0, 0)));
        assert!(!c.abandon(&key(0, 0)));
        assert!(!c.abandon(&key(0, 1)), "a ready tile is not abandoned");
        assert!(c.contains(&key(0, 1)));
        assert_eq!(c.used_bytes(), 1024);
    }

    #[test]
    fn abandon_pending_after_a_crash_keeps_ready_tiles() {
        let mut c = cache(1 << 20);
        fill(&mut c, key(0, 0), 1024);
        c.reserve(key(0, 1), 1024).expect("reserve");
        c.reserve(key(0, 2), 1024).expect("reserve");
        assert_eq!(c.abandon_pending(), 2);
        assert_eq!(c.len(), 1);
        assert!(c.get(&key(0, 0)).is_some());
        // The slots came back: the region is usable again.
        for x in 10..13 {
            c.reserve(key(0, x), 1024).expect("reserve");
        }
    }

    #[test]
    fn complete_of_an_unknown_or_ready_key_does_nothing() {
        let mut c = cache(1 << 20);
        assert!(!c.complete(&key(0, 0)));
        fill(&mut c, key(0, 1), 1024);
        assert!(!c.complete(&key(0, 1)));
        assert_eq!(c.len(), 1);
    }

    #[test]
    fn scale_buckets_group_nearby_scales_and_round_trip() {
        let a = ScaleBucket::from_scale(1.0).expect("bucket");
        assert_eq!(ScaleBucket::from_scale(1.05), Some(a));
        assert_ne!(ScaleBucket::from_scale(1.5), Some(a));
        assert!((a.scale() - 1.0).abs() < f32::EPSILON);
        for s in [0.01, 0.25, 1.0, 2.0, 3.7, 64.0] {
            let b = ScaleBucket::from_scale(s).expect("bucket");
            let ratio = b.scale() / s;
            assert!((0.88..=1.13).contains(&ratio), "scale {s} -> {}", b.scale());
            assert_eq!(ScaleBucket::from_scale(b.scale()), Some(b));
        }
        for bad in [0.0, -1.0, f32::NAN, f32::INFINITY] {
            assert_eq!(ScaleBucket::from_scale(bad), None);
        }
        assert!(ScaleBucket::from_scale(f32::MIN_POSITIVE).is_some());
    }

    #[test]
    fn different_scales_are_different_tiles() {
        let mut c = cache(1 << 20);
        let at = |s: f32| TileKey {
            scale: ScaleBucket::from_scale(s).expect("bucket"),
            ..key(0, 0)
        };
        fill(&mut c, at(1.0), 1024);
        assert_eq!(c.get(&at(2.0)), None);
        assert!(c.get(&at(1.0)).is_some());
    }

    #[test]
    fn region_geometry_follows_the_budget() {
        let g = TileCache::geometry_for_budget(DEFAULT_BUDGET_BYTES).expect("geometry");
        assert_eq!(g.slot_count(), 256);
        assert_eq!(g.slot_bytes(), SLOT_BYTES);
        let tiny = TileCache::geometry_for_budget(1).expect("geometry");
        assert_eq!(tiny.slot_count(), 1);
        let huge = TileCache::geometry_for_budget(u64::MAX).expect("geometry");
        assert_eq!(huge.total_bytes(), vellora_shm::MAX_REGION_BYTES);
    }
}
