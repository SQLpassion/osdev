//! Task capability and resource grant contracts.
//!
//! Provides coarse-grained privilege flags and fine-grained resource grants
//! for driver tasks running in user mode (Ring 3).

extern crate alloc;
use alloc::vec::Vec;

/// Coarse-grained privilege flags: which syscall classes a task may invoke.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Capabilities(u32);

impl Capabilities {
    /// No capabilities held.
    pub const NONE: Self = Self(0);

    /// May call MapPhysical / UnmapPhysical — but only on granted MMIO regions.
    pub const MMIO: Self = Self(1 << 0);

    /// May call SpawnDriver — reserved for the driver manager only.
    pub const SPAWN_DRIVER: Self = Self(1 << 2);

    /// May call Unload to terminate a registered driver task by name.
    /// Delegated by `Exec` only to the trusted `DRIVERS.BIN` binary, never to
    /// a spawned driver itself (see `driver_db::sanitize_driver_caps`).
    pub const UNLOAD_DRIVER: Self = Self(1 << 3);

    /// May call ListDrivers to enumerate currently registered driver tasks.
    /// Delegated by `Exec` only to the trusted `DRIVERS.BIN` binary, never to
    /// a spawned driver itself (see `driver_db::sanitize_driver_caps`).
    pub const LIST_DRIVERS: Self = Self(1 << 4);

    /// Creates capabilities from a raw bitmask, ignoring unknown bits.
    #[inline]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(
            bits & (Self::MMIO.0
                | Self::SPAWN_DRIVER.0
                | Self::UNLOAD_DRIVER.0
                | Self::LIST_DRIVERS.0),
        )
    }

    /// Returns the underlying raw bitmask value.
    #[inline]
    pub const fn bits(self) -> u32 {
        self.0
    }

    /// Checks whether this capability set contains all flags in `other`.
    #[inline]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    /// Returns the union of two capability sets.
    #[inline]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}

impl core::ops::BitOr for Capabilities {
    type Output = Self;

    #[inline]
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl core::ops::BitOrAssign for Capabilities {
    #[inline]
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

impl core::ops::BitAnd for Capabilities {
    type Output = Self;

    #[inline]
    fn bitand(self, rhs: Self) -> Self {
        Self(self.0 & rhs.0)
    }
}

/// Fine-grained resource grants: which concrete resources a task may touch.
/// Checked in addition to the coarse Capabilities flag.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ResourceGrants {
    /// Allowed physical MMIO regions: (phys_start, len_bytes).
    pub mmio_regions: Vec<(u64, u64)>,

    /// Bump pointer for the next free user-VA in the MMIO mapping window.
    /// Starts at USER_MMIO_BASE; advanced by MapPhysical.
    pub mmio_bump: u64,
}

/// Which allocator produced a virtual-address range mapped into the user MMIO
/// window (`USER_MMIO_BASE..USER_STACK_GUARD_BASE`).
///
/// `AllocDma` (physically-contiguous RAM frames) and `MapPhysical` (a device's
/// MMIO BAR window) share that same VA window and bump allocator, but the two
/// free paths must never be interchangeable: unmapping a `MapPhysical` range
/// through `FreeDma` would call `pmm::release_pfn` on a device register's
/// physical address, and unmapping an `AllocDma` range through
/// `UnmapPhysical` would leak its RAM frames forever (neither path releases
/// physical frames the way the other expects). See [`DriverCaps::allocations`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MmioAllocKind {
    /// Physically-contiguous RAM frames obtained via `AllocDma`.
    Dma,
    /// A device MMIO BAR window obtained via `MapPhysical`.
    Mmio,
}

/// Combined capability block — heap-allocated per driver task, null for normal tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverCaps {
    pub flags: Capabilities,
    pub grants: ResourceGrants,

    /// Live allocations in the MMIO VA window, recorded as
    /// `(page_va_start, num_pages, kind)` by a successful `AllocDma` or
    /// `MapPhysical` call and removed by the matching `FreeDma`/`UnmapPhysical`
    /// call. Consulted by the free paths so a VA can only be released through
    /// the allocator that produced it — see [`MmioAllocKind`].
    pub allocations: Vec<(u64, usize, MmioAllocKind)>,
}

impl DriverCaps {
    /// Creates a new driver capability block with the given flags and grants.
    pub fn new(flags: Capabilities, grants: ResourceGrants) -> Self {
        Self {
            flags,
            grants,
            allocations: Vec::new(),
        }
    }

    /// Records that `[page_va_start, page_va_start + num_pages * PAGE_SIZE)`
    /// was just mapped by the allocator identified by `kind`.
    ///
    /// Called only after every fallible step of the mapping syscall has
    /// already succeeded, so this recording itself is never rolled back.
    pub fn record_allocation(&mut self, page_va_start: u64, num_pages: usize, kind: MmioAllocKind) {
        self.allocations.push((page_va_start, num_pages, kind));
    }

    /// Removes and confirms an allocation record matching the given VA range
    /// and `kind` exactly, returning `true` if one was found.
    ///
    /// A free/unmap syscall must call this *before* touching the address
    /// space or PMM, and refuse to proceed at all when it returns `false` —
    /// that is what prevents `FreeDma` from being handed a `MapPhysical` VA
    /// (or vice versa).
    pub fn take_allocation(
        &mut self,
        page_va_start: u64,
        num_pages: usize,
        kind: MmioAllocKind,
    ) -> bool {
        match self
            .allocations
            .iter()
            .position(|&(va, pages, k)| va == page_va_start && pages == num_pages && k == kind)
        {
            Some(pos) => {
                self.allocations.remove(pos);
                true
            }
            None => false,
        }
    }
}
