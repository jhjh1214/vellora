//! The shared tile region: a fixed number of equally sized slots that the UI hands out and the
//! engine fills (`docs/architecture/process-model.md`, "Shared-memory tiles").

use std::fs::File;
use std::ops::Range;

use memmap2::MmapMut;

use crate::Error;

/// Largest region either side will map. The default client cache budget is 256 MB.
pub const MAX_REGION_BYTES: u64 = 1 << 30;

/// How a region is cut into slots. Both processes must use the same values; the parent passes
/// them to the engine at launch.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SlotGeometry {
    slot_count: u32,
    slot_bytes: u32,
}

impl SlotGeometry {
    /// A region of `slot_count` slots of `slot_bytes` bytes each.
    ///
    /// # Errors
    ///
    /// [`Error::InvalidGeometry`] if either is zero, `slot_bytes` is not a multiple of 4 (a
    /// pixel), or the region would exceed [`MAX_REGION_BYTES`].
    pub fn new(slot_count: u32, slot_bytes: u32) -> Result<Self, Error> {
        if slot_count == 0 {
            return Err(Error::InvalidGeometry("a region needs at least one slot"));
        }
        if slot_bytes == 0 || !slot_bytes.is_multiple_of(4) {
            return Err(Error::InvalidGeometry(
                "a slot holds a whole number of 4-byte pixels",
            ));
        }
        if u64::from(slot_count) * u64::from(slot_bytes) > MAX_REGION_BYTES {
            return Err(Error::InvalidGeometry("the region exceeds 1 GiB"));
        }
        Ok(Self {
            slot_count,
            slot_bytes,
        })
    }

    /// Number of slots.
    #[must_use]
    pub const fn slot_count(self) -> u32 {
        self.slot_count
    }

    /// Size of one slot in bytes.
    #[must_use]
    pub const fn slot_bytes(self) -> u32 {
        self.slot_bytes
    }

    /// Size of the whole region in bytes. At most [`MAX_REGION_BYTES`].
    #[must_use]
    pub const fn total_bytes(self) -> u64 {
        self.slot_count as u64 * self.slot_bytes as u64
    }

    fn range(self, slot: u32) -> Result<Range<usize>, Error> {
        if slot >= self.slot_count {
            return Err(Error::SlotOutOfRange {
                slot,
                count: self.slot_count,
            });
        }
        // `new` caps the region at 1 GiB, so these fit `usize`.
        let size = self.slot_bytes as usize;
        let start = slot as usize * size;
        Ok(start..start + size)
    }
}

/// A writable mapping of the shared tile region.
///
/// The UI creates the region ([`create`](Self::create)) and passes the backing file to the
/// engine, which maps it with [`from_file`](Self::from_file). The UI decides which slot a tile
/// goes to and does not read a slot between sending its request and receiving the matching
/// `TileReady`; the engine writes only the slot it was assigned. The side that reads a slot does
/// not trust its contents.
///
/// The backing file is anonymous (unlinked, or deleted when the last handle closes), so the
/// region has no name another process could open.
///
/// # Known gap on macOS: a compromised engine can crash the UI
///
/// The engine maps the region writable, which needs a handle that also allows resizing the
/// file. On macOS a compromised engine can shrink the file; the UI's next read of a lost page
/// then raises SIGBUS and ends the UI process. (On Windows a mapped file cannot be shortened, and
/// on Linux the region is a `memfd` sealed with `F_SEAL_SHRINK`, `F_SEAL_GROW` and `F_SEAL_SEAL`,
/// falling back to a temporary file only if the kernel refuses.) The UI's data is not exposed and
/// no memory is corrupted, but the engine is meant to be untrusted, so the macOS gap is tracked
/// in the security model and closes with `shm_open` (ADR-0015) or a UI that reads slots through a
/// handler for the fault. Until then the UI must read slots
/// only after the matching `TileReady`, and a crashed UI loses at most the open view: the
/// document and its journal are the UI's, not the engine's.
#[derive(Debug)]
pub struct TileRegion {
    geometry: SlotGeometry,
    map: MmapMut,
}

impl TileRegion {
    /// Creates a zero-filled region and returns it with the file behind it, which the caller
    /// passes to the engine (see [`share_with_child`](crate::share_with_child)).
    ///
    /// The file lives in the temporary directory, so on a system where that is a disk the
    /// operating system may write finished tiles back to it. If profiling ever shows this, an
    /// anonymous in-memory backing (`memfd` on Linux, `shm_open` on macOS, a pagefile-backed
    /// section on Windows) can replace it behind this same API (ADR-0015).
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if the file cannot be created, sized or mapped.
    pub fn create(geometry: SlotGeometry) -> Result<(Self, File), Error> {
        // Linux: an anonymous in-memory file whose size is sealed, so the engine cannot shrink it.
        // If the kernel refuses (too old, or a filter forbids `memfd_create`), fall back to a
        // temporary file with the known gap.
        #[cfg(target_os = "linux")]
        let sealed = sealed_memfd(geometry.total_bytes());
        #[cfg(not(target_os = "linux"))]
        let sealed: Option<File> = None;
        let file = if let Some(file) = sealed {
            file
        } else {
            let file = tempfile::tempfile()
                .map_err(|source| Error::io("cannot create the tile region file", source))?;
            file.set_len(geometry.total_bytes())
                .map_err(|source| Error::io("cannot size the tile region file", source))?;
            file
        };
        let region = Self::from_file(&file, geometry)?;
        Ok((region, file))
    }

    /// Maps an existing region file, as the engine does with the file it was given.
    ///
    /// # Errors
    ///
    /// [`Error::RegionTooSmall`] if the file is shorter than the geometry needs (a mismatch
    /// between the two sides, or a hostile launch), [`Error::Io`] if it cannot be mapped (for
    /// example because it was opened read-only).
    pub fn from_file(file: &File, geometry: SlotGeometry) -> Result<Self, Error> {
        let have = file
            .metadata()
            .map_err(|source| Error::io("cannot read the size of the tile region", source))?
            .len();
        if have < geometry.total_bytes() {
            return Err(Error::RegionTooSmall {
                have,
                need: geometry.total_bytes(),
            });
        }
        let len = usize::try_from(geometry.total_bytes())
            .map_err(|_| Error::InvalidGeometry("the region does not fit the address space"))?;
        // SAFETY: the file is the region's anonymous backing file, created by the UI for this
        // purpose. Our own code never resizes it, so every mapped page stays backed. The peer
        // reads or writes the same pages concurrently by design; the protocol gives each slot
        // one writer at a time, and no safe code here relies on a slot's contents staying stable
        // between reads. One hazard remains and is documented on the type: the engine holds a
        // writable handle, so a *compromised* engine can shrink the file, and the UI then faults
        // when it touches a lost page. That is a denial of service, not memory corruption.
        let map = unsafe { memmap2::MmapOptions::new().len(len).map_mut(file) }
            .map_err(|source| Error::io("cannot map the tile region", source))?;
        Ok(Self { geometry, map })
    }

    /// The geometry this region was mapped with.
    #[must_use]
    pub fn geometry(&self) -> SlotGeometry {
        self.geometry
    }

    /// Gives `fill` the bytes of `slot` to write into.
    ///
    /// # Errors
    ///
    /// [`Error::SlotOutOfRange`] (converted into `E`) for a slot that is not in the region, in
    /// which case `fill` is not called; otherwise whatever `fill` returns.
    pub fn write_slot<T, E: From<Error>>(
        &mut self,
        slot: u32,
        fill: impl FnOnce(&mut [u8]) -> Result<T, E>,
    ) -> Result<T, E> {
        let range = self.geometry.range(slot)?;
        let bytes = self.map.get_mut(range).ok_or(Error::SlotOutOfRange {
            slot,
            count: self.geometry.slot_count,
        })?;
        fill(bytes)
    }

    /// Copies `slot` into `out`, which must be exactly one slot long. For the UI side and tests.
    ///
    /// # Errors
    ///
    /// [`Error::SlotOutOfRange`], or [`Error::InvalidGeometry`] if `out` has the wrong length.
    pub fn read_slot(&self, slot: u32, out: &mut [u8]) -> Result<(), Error> {
        let range = self.geometry.range(slot)?;
        if out.len() != range.len() {
            return Err(Error::InvalidGeometry(
                "the output buffer is not exactly one slot long",
            ));
        }
        let bytes = self.map.get(range).ok_or(Error::SlotOutOfRange {
            slot,
            count: self.geometry.slot_count,
        })?;
        out.copy_from_slice(bytes);
        Ok(())
    }

    /// Copies the first `out.len()` bytes of `slot` into `out`, which may be shorter than the
    /// slot. For tiles smaller than a slot, such as thumbnails: copying the whole slot would move
    /// many times what the tile holds.
    ///
    /// # Errors
    ///
    /// [`Error::SlotOutOfRange`], or [`Error::InvalidGeometry`] if `out` is longer than a slot.
    pub fn read_slot_prefix(&self, slot: u32, out: &mut [u8]) -> Result<(), Error> {
        let range = self.geometry.range(slot)?;
        if out.len() > range.len() {
            return Err(Error::InvalidGeometry(
                "the output buffer is longer than one slot",
            ));
        }
        let bytes = self.map.get(range).ok_or(Error::SlotOutOfRange {
            slot,
            count: self.geometry.slot_count,
        })?;
        out.copy_from_slice(&bytes[..out.len()]);
        Ok(())
    }
}

/// A `memfd` of `bytes` bytes sealed against shrinking and growing (and against further seals).
/// `None` if any step fails.
#[cfg(target_os = "linux")]
fn sealed_memfd(bytes: u64) -> Option<File> {
    use std::os::fd::{FromRawFd, OwnedFd};

    // SAFETY: the name is a NUL-terminated literal; the call returns a new descriptor or -1.
    let raw = unsafe {
        libc::memfd_create(
            c"vellora-tiles".as_ptr(),
            libc::MFD_CLOEXEC | libc::MFD_ALLOW_SEALING,
        )
    };
    if raw < 0 {
        return None;
    }
    // SAFETY: `raw` is a fresh descriptor that nothing else owns.
    let file = File::from(unsafe { OwnedFd::from_raw_fd(raw) });
    file.set_len(bytes).ok()?;
    // SAFETY: `F_ADD_SEALS` takes an integer and only changes the seals of this descriptor.
    let sealed = unsafe {
        libc::fcntl(
            raw,
            libc::F_ADD_SEALS,
            libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL,
        )
    };
    (sealed == 0).then_some(file)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The engine holds a writable handle to the same file; it must not be able to shrink it.
    #[cfg(target_os = "linux")]
    #[test]
    fn the_region_is_sealed_against_resizing_on_linux() {
        let (_region, file) = TileRegion::create(SlotGeometry::new(2, 4096).unwrap()).unwrap();
        assert!(file.set_len(0).is_err(), "shrinking must be refused");
        assert!(file.set_len(1 << 20).is_err(), "growing must be refused");
        assert_eq!(file.metadata().unwrap().len(), 8192);
        // SAFETY: `F_GET_SEALS` only reads the seals of a descriptor this test owns.
        let seals =
            unsafe { libc::fcntl(std::os::fd::AsRawFd::as_raw_fd(&file), libc::F_GET_SEALS) };
        let wanted = libc::F_SEAL_SHRINK | libc::F_SEAL_GROW | libc::F_SEAL_SEAL;
        assert_eq!(seals & wanted, wanted);
    }

    #[test]
    fn geometry_is_validated() {
        let ok = SlotGeometry::new(4, 1024).unwrap();
        assert_eq!(
            (ok.slot_count(), ok.slot_bytes(), ok.total_bytes()),
            (4, 1024, 4096)
        );
        for (count, bytes) in [
            (0, 1024),
            (4, 0),
            (4, 1022),
            (4, 3),
            (u32::MAX, 4),
            (1025, 1 << 20),
        ] {
            assert!(
                matches!(
                    SlotGeometry::new(count, bytes),
                    Err(Error::InvalidGeometry(_))
                ),
                "{count} x {bytes}"
            );
        }
        // Exactly the cap is allowed.
        SlotGeometry::new(1024, 1 << 20).unwrap();
    }

    #[test]
    fn two_mappings_of_one_file_share_their_slots() {
        let geometry = SlotGeometry::new(3, 16).unwrap();
        let (mut engine_side, file) = TileRegion::create(geometry).unwrap();
        let client_side = TileRegion::from_file(&file, geometry).unwrap();

        engine_side
            .write_slot::<_, Error>(1, |bytes| {
                bytes.fill(0xAB);
                Ok(())
            })
            .unwrap();

        let mut out = [0_u8; 16];
        client_side.read_slot(1, &mut out).unwrap();
        assert_eq!(out, [0xAB; 16]);
        // The neighbours are untouched.
        client_side.read_slot(0, &mut out).unwrap();
        assert_eq!(out, [0; 16]);
        client_side.read_slot(2, &mut out).unwrap();
        assert_eq!(out, [0; 16]);
    }

    #[test]
    fn out_of_range_slots_and_buffers_fail_cleanly() {
        let (mut region, _file) = TileRegion::create(SlotGeometry::new(2, 8).unwrap()).unwrap();
        let mut called = false;
        let error = region
            .write_slot::<(), Error>(2, |_| {
                called = true;
                Ok(())
            })
            .unwrap_err();
        assert!(
            matches!(error, Error::SlotOutOfRange { slot: 2, count: 2 }),
            "{error}"
        );
        assert!(!called, "the closure must not run for a bad slot");
        assert!(
            region
                .write_slot::<(), Error>(u32::MAX, |_| Ok(()))
                .is_err()
        );

        let mut short = [0_u8; 7];
        assert!(matches!(
            region.read_slot(0, &mut short),
            Err(Error::InvalidGeometry(_))
        ));
        let mut full = [0_u8; 8];
        assert!(region.read_slot(2, &mut full).is_err());
    }

    #[test]
    fn a_region_file_that_is_too_small_is_refused() {
        let (_region, file) = TileRegion::create(SlotGeometry::new(1, 16).unwrap()).unwrap();
        let bigger = SlotGeometry::new(2, 16).unwrap();
        let error = TileRegion::from_file(&file, bigger).unwrap_err();
        assert!(
            matches!(error, Error::RegionTooSmall { have: 16, need: 32 }),
            "{error}"
        );
    }

    #[test]
    fn a_write_closure_can_fail_with_its_own_error() {
        #[derive(Debug, PartialEq)]
        enum MyError {
            Shm,
            Render,
        }
        impl From<Error> for MyError {
            fn from(_: Error) -> Self {
                Self::Shm
            }
        }
        let (mut region, _file) = TileRegion::create(SlotGeometry::new(1, 8).unwrap()).unwrap();
        assert_eq!(
            region.write_slot(0, |_| Err::<(), _>(MyError::Render)),
            Err(MyError::Render)
        );
        assert_eq!(
            region.write_slot(5, |_| Ok::<(), MyError>(())),
            Err(MyError::Shm)
        );
    }
}
