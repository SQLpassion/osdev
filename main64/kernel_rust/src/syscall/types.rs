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

impl SyscallId {
    /// Syscall number for Yield (cooperative reschedule).
    pub const YIELD: u64 = Self::Yield as u64;

    /// Syscall number for WriteSerial (debug output).
    pub const WRITE_SERIAL: u64 = Self::WriteSerial as u64;

    /// Syscall number for Exit (task termination).
    pub const EXIT: u64 = Self::Exit as u64;
}

/// Unknown syscall number.
pub const SYSCALL_ERR_UNSUPPORTED: u64 = u64::MAX;

/// Invalid argument combination for a known syscall.
pub const SYSCALL_ERR_INVALID_ARG: u64 = u64::MAX - 1;

/// I/O error during syscall execution.
pub const SYSCALL_ERR_IO: u64 = u64::MAX - 2;

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

/// Upper exclusive bound of user-accessible canonical virtual addresses.
const USER_CANONICAL_END: u64 = 0x0000_8000_0000_0000;

/// Returns `true` when `ptr..ptr+len` lies entirely within user canonical space.
///
/// Rejects null pointers, kernel-half addresses, and integer-overflow attempts.
/// A zero-length buffer is always considered valid (no memory access occurs).
///
/// # Alignment
/// This function does **not** check pointer alignment. Callers must ensure proper
/// alignment for their data types:
/// - `u8`: 1-byte alignment (always aligned)
/// - `u16`: 2-byte alignment
/// - `u32`: 4-byte alignment
/// - `u64`: 8-byte alignment
///
/// Misaligned accesses may cause undefined behavior or performance penalties
/// depending on the CPU architecture.
///
/// # Safety
/// A valid buffer range does not guarantee the memory is mapped or accessible.
/// The MMU will enforce page-level permissions at access time, potentially
/// causing a page fault if the memory is unmapped or inaccessible.
pub fn is_valid_user_buffer(ptr: *const u8, len: usize) -> bool {
    if len == 0 {
        return true;
    }
    let start = ptr as u64;
    if start == 0 {
        return false;
    }
    let end = match start.checked_add(len as u64) {
        Some(e) => e,
        None => return false,
    };
    start < USER_CANONICAL_END && end <= USER_CANONICAL_END
}

/// User-facing syscall error space.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SysError {
    /// Unknown or unsupported syscall number.
    UnsupportedSyscall,
    /// Invalid syscall arguments (e.g., null pointer, out-of-bounds buffer).
    InvalidArgument,
    /// I/O error during syscall execution.
    IoError,
    /// Any unclassified kernel return value in the error range.
    Unknown(u64),
}

/// Decodes a raw syscall return value into `Result`.
#[inline]
#[allow(dead_code)]
pub fn decode_result(raw: u64) -> Result<u64, SysError> {
    match raw {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        x if x >= SYSCALL_ERR_IO => Err(SysError::Unknown(x)),
        value => Ok(value),
    }
}
