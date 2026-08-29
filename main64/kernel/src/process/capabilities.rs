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

    /// May call IrqSubscribe / IrqWait / IrqAck — but only on granted IRQ vectors.
    pub const IRQ: Self = Self(1 << 1);

    /// May call SpawnDriver — reserved for the driver manager only.
    pub const SPAWN_DRIVER: Self = Self(1 << 2);

    /// Creates capabilities from a raw bitmask, ignoring unknown bits.
    #[inline]
    pub const fn from_bits_truncate(bits: u32) -> Self {
        Self(bits & (Self::MMIO.0 | Self::IRQ.0 | Self::SPAWN_DRIVER.0))
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

    /// Allowed IRQ vectors.
    pub irqs: Vec<u8>,

    /// Bump pointer for the next free user-VA in the MMIO mapping window.
    /// Starts at USER_MMIO_BASE; advanced by MapPhysical.
    pub mmio_bump: u64,
}

/// Combined capability block — heap-allocated per driver task, null for normal tasks.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriverCaps {
    pub flags: Capabilities,
    pub grants: ResourceGrants,
}

impl DriverCaps {
    /// Creates a new driver capability block with the given flags and grants.
    pub fn new(flags: Capabilities, grants: ResourceGrants) -> Self {
        Self { flags, grants }
    }
}
