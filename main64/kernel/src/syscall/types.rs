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
    /// Write bytes to VGA text console.
    WriteConsole = 3,
    /// Read a single character from keyboard (blocking).
    GetChar = 4,
    /// Get current VGA cursor position (packed as `row<<32 | col`).
    GetCursor = 5,
    /// Set VGA cursor position (`arg0=row`, `arg1=col`).
    SetCursor = 6,
    /// Clear VGA text screen and reset cursor to origin.
    ClearScreen = 7,
    /// Open a file in the file system.
    OpenFile = 8,
    /// Close an active file descriptor.
    CloseFile = 9,
    /// Read data from an active file descriptor.
    ReadFile = 10,
    /// Write data to an active file descriptor.
    WriteFile = 11,
    /// Delete a file from the file system.
    DeleteFile = 12,
    /// Seek to a specific offset in a file descriptor.
    SeekFile = 13,
    /// Return whether the file descriptor has reached EOF.
    EndOfFile = 14,
    /// Print the root directory listing of the disk.
    PrintRootDirectory = 15,
    /// Map memory pages.
    Mmap = 16,
    /// Execute a flat binary from the mounted filesystem.
    Exec = 17,
    /// Wait for a user-space task to exit.
    Wait = 18,
    /// Shutdown the system.
    Shutdown = 19,
    /// Blit a full 80×25 frame buffer from user space to VGA in one step.
    WriteFramebuffer = 20,
    /// Read a single extended key event from the keyboard (blocking).
    ReadKey = 21,
    /// Configure VGA text-mode settings (cursor visibility, blink mode).
    SetVgaMode = 22,
    /// Retrieve the number of discovered PCI devices.
    GetPciDeviceCount = 23,
    /// Retrieve metadata for a specific PCI device by its index.
    GetPciDevice = 24,
    /// Retrieve the number of entries in the BIOS memory map.
    GetBiosMemoryMapEntryCount = 25,
    /// Retrieve metadata for a specific BIOS memory map entry by its index.
    GetBiosMemoryMapEntry = 26,
    /// Retrieve the current high-precision calendar date and time.
    GetTime = 27,
    /// Read a single extended key event from the keyboard (non-blocking).
    PollKey = 28,
    /// Retrieve the current console dimensions (packed as `rows << 32 | cols`).
    GetConsoleDimensions = 29,
}

impl SyscallId {
    /// Syscall number for Yield (cooperative reschedule).
    pub const YIELD: u64 = Self::Yield as u64;

    /// Syscall number for WriteSerial (debug output).
    pub const WRITE_SERIAL: u64 = Self::WriteSerial as u64;

    /// Syscall number for Exit (task termination).
    pub const EXIT: u64 = Self::Exit as u64;

    /// Syscall number for WriteConsole (VGA text output).
    pub const WRITE_CONSOLE: u64 = Self::WriteConsole as u64;

    /// Syscall number for GetChar (blocking keyboard input).
    pub const GET_CHAR: u64 = Self::GetChar as u64;

    /// Syscall number for GetCursor.
    pub const GET_CURSOR: u64 = Self::GetCursor as u64;

    /// Syscall number for SetCursor.
    pub const SET_CURSOR: u64 = Self::SetCursor as u64;

    /// Syscall number for ClearScreen.
    pub const CLEAR_SCREEN: u64 = Self::ClearScreen as u64;

    /// Syscall number for OpenFile.
    pub const OPEN_FILE: u64 = Self::OpenFile as u64;

    /// Syscall number for CloseFile.
    pub const CLOSE_FILE: u64 = Self::CloseFile as u64;

    /// Syscall number for ReadFile.
    pub const READ_FILE: u64 = Self::ReadFile as u64;

    /// Syscall number for WriteFile.
    pub const WRITE_FILE: u64 = Self::WriteFile as u64;

    /// Syscall number for DeleteFile.
    pub const DELETE_FILE: u64 = Self::DeleteFile as u64;

    /// Syscall number for SeekFile.
    pub const SEEK_FILE: u64 = Self::SeekFile as u64;

    /// Syscall number for EndOfFile.
    pub const END_OF_FILE: u64 = Self::EndOfFile as u64;

    /// Syscall number for PrintRootDirectory.
    pub const PRINT_ROOT_DIRECTORY: u64 = Self::PrintRootDirectory as u64;

    /// Syscall number for Mmap.
    pub const MMAP: u64 = Self::Mmap as u64;

    /// Syscall number for Exec.
    pub const EXEC: u64 = Self::Exec as u64;

    /// Syscall number for Wait.
    pub const WAIT: u64 = Self::Wait as u64;

    /// Syscall number for Shutdown.
    pub const SHUTDOWN: u64 = Self::Shutdown as u64;

    /// Syscall number for WriteFramebuffer (frame blit to VGA).
    pub const WRITE_FRAMEBUFFER: u64 = Self::WriteFramebuffer as u64;
    /// Syscall number for ReadKey (extended key event).
    pub const READ_KEY: u64 = Self::ReadKey as u64;
    /// Syscall number for SetVgaMode (VGA mode configuration).
    pub const SET_VGA_MODE: u64 = Self::SetVgaMode as u64;
    /// Syscall number for GetPciDeviceCount (PCI device count query).
    pub const GET_PCI_DEVICE_COUNT: u64 = Self::GetPciDeviceCount as u64;
    /// Syscall number for GetPciDevice (PCI device metadata query).
    pub const GET_PCI_DEVICE: u64 = Self::GetPciDevice as u64;
    /// Syscall number for GetBiosMemoryMapEntryCount (BIOS memory map entry count query).
    pub const GET_BIOS_MEMORY_MAP_ENTRY_COUNT: u64 = Self::GetBiosMemoryMapEntryCount as u64;
    /// Syscall number for GetBiosMemoryMapEntry (BIOS memory map entry query).
    pub const GET_BIOS_MEMORY_MAP_ENTRY: u64 = Self::GetBiosMemoryMapEntry as u64;
    /// Syscall number for GetTime (calendar date and time query).
    pub const GET_TIME: u64 = Self::GetTime as u64;
    /// Syscall number for PollKey (non-blocking keyboard query).
    pub const POLL_KEY: u64 = Self::PollKey as u64;
    /// Syscall number for GetConsoleDimensions.
    pub const GET_CONSOLE_DIMENSIONS: u64 = Self::GetConsoleDimensions as u64;
}

/// Unknown syscall number.
pub const SYSCALL_ERR_UNSUPPORTED: u64 = u64::MAX;

/// Invalid argument combination for a known syscall.
pub const SYSCALL_ERR_INVALID_ARG: u64 = u64::MAX - 1;

/// I/O error during syscall execution.
pub const SYSCALL_ERR_IO: u64 = u64::MAX - 2;

/// Out-of-memory error during syscall execution.
pub const SYSCALL_ERR_OUT_OF_MEMORY: u64 = u64::MAX - 3;

/// Successful syscall return code for void-like operations.
pub const SYSCALL_OK: u64 = 0;

/// Kernel-internal syscall error type used by dispatcher logic.
///
/// This keeps syscall implementations free from raw ABI sentinel values.
#[cfg_attr(not(test), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyscallError {
    /// Unknown or unsupported syscall number.
    Unsupported,

    /// Invalid syscall arguments (e.g., null pointer, out-of-bounds buffer).
    InvalidArg,

    /// I/O error during syscall execution.
    Io,

    /// Out-of-memory error (e.g., physical frame allocator exhausted).
    OutOfMemory,
}

/// Kernel-internal syscall result type.
pub type SyscallResult<T> = Result<T, SyscallError>;

/// Converts a kernel-internal syscall error into the raw ABI return value.
#[inline]
pub const fn syscall_error_to_raw(err: SyscallError) -> u64 {
    match err {
        SyscallError::Unsupported => SYSCALL_ERR_UNSUPPORTED,
        SyscallError::InvalidArg => SYSCALL_ERR_INVALID_ARG,
        SyscallError::Io => SYSCALL_ERR_IO,
        SyscallError::OutOfMemory => SYSCALL_ERR_OUT_OF_MEMORY,
    }
}

/// Converts a kernel-internal syscall result into the raw ABI return value.
#[inline]
pub const fn syscall_result_to_raw(result: SyscallResult<u64>) -> u64 {
    match result {
        Ok(value) => value,
        Err(err) => syscall_error_to_raw(err),
    }
}

/// Computes the user-mode alias RIP for a kernel function page mapped at `code_page_user_va`.
///
/// The returned address keeps the original 4 KiB page offset of `kernel_entry_va`.
#[inline]
#[cfg_attr(not(test), allow(dead_code))]
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

/// Returns `true` when every page in `ptr..ptr+len` is present and user-writable.
///
/// This extends [`is_valid_user_buffer`] with a read-only page-table walk under
/// the currently active address space. It does not fault in missing pages and
/// therefore never changes the task's memory layout. A zero-length buffer is
/// valid because no memory access will follow.
#[cfg(feature = "kernel")]
pub fn is_valid_user_buffer_writable(ptr: *const u8, len: usize) -> bool {
    // Step 1: Preserve the canonical-range and overflow checks shared by all
    // user buffers before inspecting page-table state.
    if !is_valid_user_buffer(ptr, len) {
        return false;
    }
    if len == 0 {
        return true;
    }

    let start = ptr as u64;
    let end = start + len as u64;
    let mut page = start & !(crate::arch::constants::PAGE_SIZE_U64 - 1);

    // Step 2: Check every page touched by the range, including partially used
    // first and last pages. Missing pages are rejected rather than demand-mapped.
    while page < end {
        if !crate::memory::vmm::is_user_page_writable(page) {
            return false;
        }
        page += crate::arch::constants::PAGE_SIZE_U64;
    }

    true
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
    /// Out-of-memory error (e.g., physical frame allocator exhausted).
    OutOfMemory,
    /// Any unclassified kernel return value in the error range.
    Unknown(u64),
}

impl core::fmt::Display for SysError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Step 1: Match the enum variant to its corresponding string representation.
        match self {
            SysError::UnsupportedSyscall => write!(f, "UnsupportedSyscall"),
            SysError::InvalidArgument => write!(f, "InvalidArgument"),
            SysError::IoError => write!(f, "IoError"),
            SysError::OutOfMemory => write!(f, "OutOfMemory"),
            SysError::Unknown(raw) => write!(f, "UnknownError(0x{:x})", raw),
        }
    }
}

impl core::fmt::LowerHex for SysError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Step 1: Map each variant back to its original raw ABI u64 value for hex formatting.
        let val = match self {
            SysError::UnsupportedSyscall => SYSCALL_ERR_UNSUPPORTED,
            SysError::InvalidArgument => SYSCALL_ERR_INVALID_ARG,
            SysError::IoError => SYSCALL_ERR_IO,
            SysError::OutOfMemory => SYSCALL_ERR_OUT_OF_MEMORY,
            SysError::Unknown(raw) => *raw,
        };
        core::fmt::LowerHex::fmt(&val, f)
    }
}

/// Decodes a raw syscall return value into `Result`.
#[inline]
#[cfg_attr(not(test), allow(dead_code))]
pub fn decode_result(raw: u64) -> Result<u64, SysError> {
    match raw {
        SYSCALL_ERR_UNSUPPORTED => Err(SysError::UnsupportedSyscall),
        SYSCALL_ERR_INVALID_ARG => Err(SysError::InvalidArgument),
        SYSCALL_ERR_IO => Err(SysError::IoError),
        SYSCALL_ERR_OUT_OF_MEMORY => Err(SysError::OutOfMemory),
        x if x >= SYSCALL_ERR_OUT_OF_MEMORY => Err(SysError::Unknown(x)),
        value => Ok(value),
    }
}

/// User-space representation of a PCI Base Address Register (BAR).
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserPciBar {
    /// The type of BAR (0 = None, 1 = Io, 2 = Memory32, 3 = Memory64).
    pub bar_type: u32,
    /// Memory prefetchable flag (1 if true, 0 if false).
    pub flags: u32,
    /// Base physical address or I/O port address.
    pub address: u64,
    /// Size of the address space in bytes.
    pub size: u64,
    /// Raw configuration register value.
    pub raw_value: u32,
    /// Explicit padding for 8-byte alignment structure size matching.
    pub _padding: u32,
}

/// User-space representation of a scanned PCI device.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserPciDevice {
    /// PCI Bus ID (0..=255).
    pub bus: u8,
    /// PCI Device/Slot ID (0..=31).
    pub device: u8,
    /// PCI Function ID (0..=7).
    pub function: u8,
    /// Device class code.
    pub class_code: u8,
    /// Device subclass code.
    pub subclass: u8,
    /// Programming Interface of the device.
    pub prog_if: u8,
    /// Revision number of the device.
    pub revision_id: u8,
    /// Header Type configuration byte.
    pub header_type: u8,
    /// Vendor ID.
    pub vendor_id: u16,
    /// Device ID.
    pub device_id: u16,
    /// Interrupt Line vector.
    pub interrupt_line: u8,
    /// Interrupt Pin value.
    pub interrupt_pin: u8,
    /// Explicit padding to align bars to 8-byte boundary.
    pub _padding: [u8; 2],
    /// Up to 6 Base Address Registers for this device.
    pub bars: [UserPciBar; 6],
}

/// User-space representation of a BIOS memory map region.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserBiosMemoryRegion {
    /// Physical start address of the region
    pub start: u64,
    /// Size of the region in bytes
    pub size: u64,
    /// Region type (1 = usable, others reserved/ACPI/etc.)
    pub region_type: u32,
    /// Explicit padding for 8-byte alignment structure size matching.
    pub _padding: u32,
}

/// User-space representation of the calendar date and time.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserDateTime {
    /// Calendar year (e.g. 2026).
    pub year: i32,
    /// Calendar month (1..=12).
    pub month: u8,
    /// Calendar day of month (1..=31).
    pub day: u8,
    /// Calendar hour (0..=23).
    pub hour: u8,
    /// Calendar minute (0..=59).
    pub minute: u8,
    /// Calendar second (0..=59).
    pub second: u8,
    /// Explicit padding for 8-byte alignment structure size matching.
    pub _padding: [u8; 7],
}
