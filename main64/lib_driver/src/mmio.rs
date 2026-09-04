//! Memory-mapped I/O (MMIO) hardware access abstractions.

use crate::kernel_types::{decode_result, SysError, SyscallId};
use crate::raw::{syscall2, syscall3};
use core::ptr;

/// Represents an active MMIO mapping in the driver's address space.
///
/// Hardware registers can be accessed with volatile reads and writes without
/// syscall overhead. Dropping this struct unmaps the physical region from the task.
#[derive(Debug)]
pub struct Mmio {
    base: *mut u8,
    len: usize,
}

impl Mmio {
    /// Maps a physical MMIO region into this task's virtual address space.
    ///
    /// Returns `Err` if the task lacks `Capabilities::MMIO` or the physical region
    /// is not covered by a granted MMIO range.
    pub fn map(phys: u64, len: usize) -> Result<Self, SysError> {
        // SAFETY:
        // - Invokes MapPhysical syscall (nr. 30).
        // - Arguments are verified by the kernel capability and page-table mapper.
        let raw_res = unsafe { syscall3(SyscallId::MAP_PHYSICAL, phys, len as u64, 0) };
        let va = decode_result(raw_res)?;

        Ok(Self {
            base: va as *mut u8,
            len,
        })
    }

    /// Returns the virtual base address of this MMIO mapping.
    #[inline]
    pub fn base(&self) -> *mut u8 {
        self.base
    }

    /// Returns the mapped length in bytes.
    #[inline]
    pub fn len(&self) -> usize {
        self.len
    }

    /// Returns whether this mapping is empty.
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Reads an 8-bit register at byte offset `off`.
    ///
    /// # Panics
    /// Panics if `off + 1 > self.len`.
    #[inline]
    pub fn read8(&self, off: usize) -> u8 {
        assert!(off < self.len, "MMIO read8 out of bounds");
        // SAFETY:
        // - `off < self.len`, so `ptr` lies within the mapped hardware MMIO window.
        // - Volatile read is used to bypass compiler caching and observe real device register state.
        unsafe { ptr::read_volatile(self.base.add(off)) }
    }

    /// Writes an 8-bit value to register at byte offset `off`.
    ///
    /// # Panics
    /// Panics if `off + 1 > self.len`.
    #[inline]
    pub fn write8(&self, off: usize, value: u8) {
        assert!(off < self.len, "MMIO write8 out of bounds");
        // SAFETY:
        // - `off < self.len`, so `ptr` lies within the mapped hardware MMIO window.
        // - Volatile write ensures side-effects reach hardware registers immediately.
        unsafe { ptr::write_volatile(self.base.add(off), value) }
    }

    /// Reads a 16-bit register at byte offset `off`.
    ///
    /// # Panics
    /// Panics if `off + 2 > self.len`.
    #[inline]
    pub fn read16(&self, off: usize) -> u16 {
        assert!(off + 2 <= self.len, "MMIO read16 out of bounds");
        // SAFETY:
        // - `off + 2 <= self.len`, so the 2-byte access is within the mapped MMIO window.
        // - Volatile read observes hardware register changes directly.
        unsafe { ptr::read_volatile(self.base.add(off) as *const u16) }
    }

    /// Writes a 16-bit value to register at byte offset `off`.
    ///
    /// # Panics
    /// Panics if `off + 2 > self.len`.
    #[inline]
    pub fn write16(&self, off: usize, value: u16) {
        assert!(off + 2 <= self.len, "MMIO write16 out of bounds");
        // SAFETY:
        // - `off + 2 <= self.len`, within the mapped hardware MMIO window.
        // - Volatile write delivers command/control register updates to hardware.
        unsafe { ptr::write_volatile(self.base.add(off) as *mut u16, value) }
    }

    /// Reads a 32-bit register at byte offset `off`.
    ///
    /// # Panics
    /// Panics if `off + 4 > self.len`.
    #[inline]
    pub fn read32(&self, off: usize) -> u32 {
        assert!(off + 4 <= self.len, "MMIO read32 out of bounds");
        // SAFETY:
        // - `off + 4 <= self.len`, within the mapped hardware MMIO window.
        // - Volatile read observes hardware register state.
        unsafe { ptr::read_volatile(self.base.add(off) as *const u32) }
    }

    /// Writes a 32-bit value to register at byte offset `off`.
    ///
    /// # Panics
    /// Panics if `off + 4 > self.len`.
    #[inline]
    pub fn write32(&self, off: usize, value: u32) {
        assert!(off + 4 <= self.len, "MMIO write32 out of bounds");
        // SAFETY:
        // - `off + 4 <= self.len`, within the mapped hardware MMIO window.
        // - Volatile write delivers updates to hardware registers.
        unsafe { ptr::write_volatile(self.base.add(off) as *mut u32, value) }
    }
}

impl Drop for Mmio {
    fn drop(&mut self) {
        if !self.base.is_null() && self.len > 0 {
            // SAFETY:
            // - Unmaps the physical memory window from the task's page table.
            // - self.base was returned by a successful MapPhysical call.
            let raw_res =
                unsafe { syscall2(SyscallId::UNMAP_PHYSICAL, self.base as u64, self.len as u64) };
            // Drop cannot return a Result, so a kernel-side failure here cannot be
            // propagated to the caller. Surface it loudly instead of silently
            // treating the window as unmapped while it may still be live.
            debug_assert!(
                decode_result(raw_res).is_ok(),
                "UnmapPhysical failed while dropping an Mmio mapping"
            );
            self.base = ptr::null_mut();
        }
    }
}
