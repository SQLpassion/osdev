/// Stable syscall numbers exposed to user mode.
#[repr(u64)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyscallId {
    /// Cooperative reschedule request.
    Yield = 0,
    /// Write bytes to debug serial (COM1).
    WriteSerial = 1,
    /// Terminate current task.
    Exit = 2,
}

/// Unknown syscall number.
pub const SYSCALL_ERR_UNSUPPORTED: u64 = u64::MAX;

/// Invalid argument combination for a known syscall.
pub const SYSCALL_ERR_INVALID_ARG: u64 = u64::MAX - 1;

/// Successful syscall return code for void-like operations.
pub const SYSCALL_OK: u64 = 0;

/// Computes the user-mode alias RIP for a kernel function page mapped at `code_page_user_va`.
///
/// The returned address keeps the original 4 KiB page offset of `kernel_entry_va`.
#[inline]
#[allow(dead_code)]
pub const fn user_alias_rip(code_page_user_va: u64, kernel_entry_va: u64) -> u64 {
    code_page_user_va + (kernel_entry_va & 0xFFF)
}

/// Maps a kernel virtual address into a user-code alias window.
///
/// Returns `None` when `kernel_va` is below `kernel_base` or when the offset
/// does not fit into the provided user code window size.
#[inline]
pub const fn user_alias_va_for_kernel(
    user_code_base: u64,
    user_code_size: u64,
    kernel_base: u64,
    kernel_va: u64,
) -> Option<u64> {
    if kernel_va < kernel_base {
        return None;
    }
    let offset = kernel_va - kernel_base;
    if offset >= user_code_size {
        return None;
    }
    Some(user_code_base + offset)
}

/// User-facing syscall error space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysError {
    /// Unknown syscall number.
    Enosys,
    /// Invalid syscall arguments.
    Einval,
    /// Any unclassified kernel return value in the error range.
    Unknown(u64),
}

/// Decodes a raw syscall return value into `Result`.
#[inline]
#[allow(dead_code)]
pub fn decode_result(raw: u64) -> Result<u64, SysError> {
    match raw {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::Enosys),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::Einval),
        x if x >= SYSCALL_ERR_INVALID_ARG => Err(SysError::Unknown(x)),
        value => Ok(value),
    }
}
