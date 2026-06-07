//! Memory mapping syscall wrapper.

use crate::{raw::syscall2, SyscallId, SYSCALL_ERR_OUT_OF_MEMORY};

/// Maps `length` bytes of user-space memory at virtual address `addr`.
///
/// Returns a pointer to the mapped region on success.
#[inline(always)]
pub fn mmap(addr: usize, length: usize) -> Result<*mut u8, u64> {
    let raw = unsafe {
        // SAFETY:
        // - `Mmap` passes integer addresses, not dereferenceable pointers.
        // - Pointer validation is performed by the kernel.
        syscall2(SyscallId::Mmap as u64, addr as u64, length as u64)
    };
    if raw >= SYSCALL_ERR_OUT_OF_MEMORY {
        return Err(raw);
    }
    Ok(raw as *mut u8)
}
