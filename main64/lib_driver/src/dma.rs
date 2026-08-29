//! Direct Memory Access (DMA) buffer management and address translation for user-space drivers.

use crate::kernel_types::{decode_result, SysError, SyscallId};
use crate::raw::{syscall1, syscall2};
use core::ptr;

/// A contiguous physical DMA buffer mapped into user space with uncacheable attributes.
#[derive(Debug)]
pub struct DmaBuffer {
    va: *mut u8,
    pa: u64,
    pages: usize,
}

impl DmaBuffer {
    /// Allocates `pages` contiguous physical 4 KiB frames and maps them into the driver address space.
    pub fn allocate(pages: usize) -> Result<Self, SysError> {
        let mut pa: u64 = 0;
        let raw_res = unsafe {
            // SAFETY:
            // - `&mut pa` is a valid stack pointer to 8 writable bytes.
            // - Arguments are validated by kernel AllocDma dispatcher.
            syscall2(
                SyscallId::ALLOC_DMA,
                pages as u64,
                &mut pa as *mut u64 as u64,
            )
        };
        let va = decode_result(raw_res)?;

        Ok(Self {
            va: va as *mut u8,
            pa,
            pages,
        })
    }

    /// Returns the virtual starting address of the DMA buffer.
    #[inline]
    pub fn va(&self) -> *mut u8 {
        self.va
    }

    /// Returns the physical starting address of the DMA buffer.
    #[inline]
    pub fn pa(&self) -> u64 {
        self.pa
    }

    /// Returns the total buffer length in bytes (`pages * 4096`).
    #[inline]
    pub fn len(&self) -> usize {
        self.pages * 4096
    }

    /// Returns whether the buffer is empty (0 bytes).
    #[inline]
    pub fn is_empty(&self) -> bool {
        self.pages == 0
    }

    /// Returns a slice view of the DMA buffer.
    #[inline]
    pub fn as_slice(&self) -> &[u8] {
        // SAFETY:
        // - `self.va` is mapped and valid for `self.len()` bytes.
        // - Single ownership of buffer memory is maintained.
        unsafe { core::slice::from_raw_parts(self.va, self.len()) }
    }

    /// Returns a mutable slice view of the DMA buffer.
    #[inline]
    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        // SAFETY:
        // - `self.va` is mapped and valid for `self.len()` bytes.
        // - Exclusive mutable access is guarded by `&mut self`.
        unsafe { core::slice::from_raw_parts_mut(self.va, self.len()) }
    }
}

impl Drop for DmaBuffer {
    fn drop(&mut self) {
        if !self.va.is_null() && self.pages > 0 {
            // SAFETY:
            // - `self.va` was returned by a successful AllocDma call.
            // - Unmaps the virtual memory and releases physical frames back to PMM.
            unsafe {
                let _ = syscall2(SyscallId::FREE_DMA, self.va as u64, self.pages as u64);
            }
            self.va = ptr::null_mut();
        }
    }
}

/// Translates a virtual address in the current driver process to its physical address.
pub fn virt_to_phys(va: u64) -> Result<u64, SysError> {
    // SAFETY:
    // - Invokes VirtToPhys syscall.
    // - Arguments are validated by kernel page-table walker.
    let raw = unsafe { syscall1(SyscallId::VIRT_TO_PHYS, va) };
    decode_result(raw)
}
