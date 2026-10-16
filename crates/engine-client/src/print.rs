//! Slots for the bands of a page that is being printed.
//!
//! A printed page is rendered in bands, each into a slot of the shared region, and read out as it
//! is drawn on the printer. The bands are not cached (a page is printed once), so they need no
//! keys and no eviction: a few slots of their own, taken for a band, given back when it has been
//! read or the request is withdrawn, so that printing never takes room from the tiles or the
//! thumbnails.

use std::collections::HashMap;
use std::ops::Range;

use vellora_ipc::{RequestId, SlotId};

/// How many bands may be in flight or waiting to be read at once.
pub const DEFAULT_PRINT_SLOTS: u32 = 4;

/// A band that has a slot.
#[derive(Debug, Clone, Copy)]
struct Band {
    slot: SlotId,
    /// The size of the band in pixels, which is how much of the slot is meaningful.
    width: u32,
    height: u32,
    /// The engine has written it.
    ready: bool,
}

/// The slots reserved for print bands.
#[derive(Debug)]
pub(crate) struct PrintPool {
    free: Vec<SlotId>,
    bands: HashMap<RequestId, Band>,
}

impl PrintPool {
    /// A pool over the slots `slots` of the region.
    pub(crate) fn over_slots(slots: Range<u32>) -> Self {
        Self {
            // Handed out from the front of the range.
            free: slots.rev().map(SlotId).collect(),
            bands: HashMap::new(),
        }
    }

    /// A free slot, taken out of the pool; `None` when every slot is in use.
    pub(crate) fn take_slot(&mut self) -> Option<SlotId> {
        self.free.pop()
    }

    /// Gives `slot` to the band for `request`, `width` x `height` pixels.
    pub(crate) fn assign(&mut self, request: RequestId, slot: SlotId, width: u32, height: u32) {
        self.bands.insert(
            request,
            Band {
                slot,
                width,
                height,
                ready: false,
            },
        );
    }

    /// Marks the band for `request` as written; `false` if it is not a print band.
    pub(crate) fn mark_ready(&mut self, request: RequestId) -> bool {
        self.bands.get_mut(&request).is_some_and(|band| {
            band.ready = true;
            true
        })
    }

    /// The slot and the size of the band for `request`, if the engine has written it.
    pub(crate) fn ready(&self, request: RequestId) -> Option<(SlotId, u32, u32)> {
        self.bands
            .get(&request)
            .filter(|band| band.ready)
            .map(|band| (band.slot, band.width, band.height))
    }

    /// Gives the slot of the band for `request` back; `false` if there is no such band.
    pub(crate) fn release(&mut self, request: RequestId) -> bool {
        match self.bands.remove(&request) {
            Some(band) => {
                self.free.push(band.slot);
                true
            }
            None => false,
        }
    }

    /// The engine died: the bands it had not written will never arrive, so their slots are free
    /// again. The ones it had written can still be read.
    pub(crate) fn lose_unwritten(&mut self) {
        let lost: Vec<RequestId> = self
            .bands
            .iter()
            .filter(|(_, band)| !band.ready)
            .map(|(request, _)| *request)
            .collect();
        for request in lost {
            self.release(request);
        }
    }

    #[cfg(test)]
    fn free_count(&self) -> usize {
        self.free.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(n: u64) -> RequestId {
        RequestId(n)
    }

    #[test]
    fn slots_are_handed_out_until_none_is_left_and_come_back_when_released() {
        let mut pool = PrintPool::over_slots(10..12);
        let a = pool.take_slot().unwrap();
        let b = pool.take_slot().unwrap();
        assert_eq!((a, b), (SlotId(10), SlotId(11)));
        assert_eq!(pool.take_slot(), None);
        pool.assign(id(1), a, 4, 3);
        pool.assign(id(2), b, 8, 8);
        assert!(pool.release(id(1)));
        assert!(!pool.release(id(1)), "once");
        assert_eq!(pool.take_slot(), Some(SlotId(10)));
        assert_eq!(pool.free_count(), 0);
    }

    #[test]
    fn a_band_can_be_read_only_after_the_engine_wrote_it() {
        let mut pool = PrintPool::over_slots(0..2);
        let slot = pool.take_slot().unwrap();
        pool.assign(id(7), slot, 16, 4);
        assert_eq!(pool.ready(id(7)), None);
        assert!(pool.mark_ready(id(7)));
        assert_eq!(pool.ready(id(7)), Some((slot, 16, 4)));
        assert!(!pool.mark_ready(id(8)), "not a print band");
        assert_eq!(pool.ready(id(8)), None);
    }

    #[test]
    fn a_crash_frees_the_unwritten_bands_and_keeps_the_written_ones() {
        let mut pool = PrintPool::over_slots(0..3);
        for n in 1..=3 {
            let slot = pool.take_slot().unwrap();
            pool.assign(id(n), slot, 1, 1);
        }
        pool.mark_ready(id(2));
        pool.lose_unwritten();
        assert_eq!(pool.free_count(), 2);
        assert!(pool.ready(id(2)).is_some());
        assert_eq!(pool.ready(id(1)), None);
    }
}
