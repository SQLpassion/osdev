//! System time query wrappers for Ring-3 programs.

use crate::{decode_result, raw::syscall1, SysError, SyscallId};

pub use crate::kernel_types::UserDateTime;

/// Copies the current high-precision calendar date and time into the output buffer.
#[inline(always)]
pub fn get_time(out: &mut UserDateTime) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY:
        // - `out` is a valid mutable reference whose address and size are safe to be written by the kernel.
        syscall1(SyscallId::GetTime as u64, out as *mut UserDateTime as u64)
    };

    decode_result(raw).map(|_| ())
}
