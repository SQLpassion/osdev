//! Memory mapping syscall wrapper.

use crate::{decode_result, raw::syscall2, SysError, SyscallId};

/// Maps `length` bytes of user-space memory at virtual address `addr`.
///
/// Returns a pointer to the mapped region on success.
#[inline(always)]
pub fn mmap(addr: usize, length: usize) -> Result<*mut u8, SysError> {
    let raw = unsafe {
        // SAFETY:
        // - `Mmap` passes integer addresses, not dereferenceable pointers.
        // - Pointer validation is performed by the kernel.
        syscall2(SyscallId::Mmap as u64, addr as u64, length as u64)
    };
    decode_result(raw).map(|ptr| ptr as *mut u8)
}
