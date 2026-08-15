//! Shared process/exec contracts for user-mode program loading.

use core::fmt;

use crate::memory::vmm;

/// Base address of the user code window (alias of [`vmm::USER_CODE_BASE`]).
///
/// The loader no longer uses this as an entry point: every program is ELF, so
/// `entry_rip` always comes from the image's own `e_entry` (see
/// [`crate::process::elf::ElfImage::entry`]). This constant remains as a
/// fixed reference value for tests and for `LoadedProgram` construction
/// sites that need a valid in-window address; it happens to still equal every
/// in-tree program's `e_entry` today because each program's linker script
/// places `_start` first at the window base, but that is a per-program linker
/// convention, not a loader guarantee.
pub const USER_PROGRAM_ENTRY_RIP: u64 = vmm::USER_CODE_BASE;

/// User stack alignment used by scheduler/user task bootstrap frames.
pub const USER_PROGRAM_STACK_ALIGNMENT: u64 = 16;

/// Initial user-mode stack pointer used when spawning a fresh process.
///
/// `iretq` restores this as ring-3 RSP. Keeping a 16-byte aligned stack
/// preserves ABI expectations for function prologues in user binaries.
pub const USER_PROGRAM_INITIAL_RSP: u64 = vmm::USER_STACK_TOP - USER_PROGRAM_STACK_ALIGNMENT;

/// Maximum executable image size accepted by the ELF loader.
pub const USER_PROGRAM_MAX_IMAGE_SIZE: usize = vmm::USER_CODE_SIZE as usize;

/// Returns whether a program image length fits inside the configured user code window.
#[inline]
pub const fn image_fits_user_code(image_len: usize) -> bool {
    image_len <= USER_PROGRAM_MAX_IMAGE_SIZE
}

/// Error space for process exec/load operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecError {
    /// Program name is invalid or not representable for the selected loader path.
    InvalidName,

    /// Program was not found in backing storage.
    NotFound,

    /// Entry exists but is a directory rather than a regular executable file.
    IsDirectory,

    /// Program image is empty and therefore has no executable payload.
    EmptyImage,

    /// Program image does not fit inside the user executable window.
    FileTooLarge,

    /// Program image starts with the ELF magic but fails ELF64 validation
    /// (see [`crate::process::elf::ElfError`] for the specific reason).
    InvalidElfImage,

    /// Physical-frame allocation failed while preparing code/stack pages.
    OutOfMemory,

    /// Mapping code/stack pages into user space failed.
    MappingFailed,

    /// Spawning the scheduler task for the process failed.
    SpawnFailed,

    /// Generic storage or transport I/O failure.
    Io,
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidName => f.write_str("invalid file name (expected 8.3 format)"),
            Self::NotFound => f.write_str("file not found"),
            Self::IsDirectory => f.write_str("path points to a directory, not a program file"),
            Self::EmptyImage => f.write_str("program image is empty"),
            Self::FileTooLarge => f.write_str("program image exceeds user code size limit"),
            Self::InvalidElfImage => f.write_str("program image failed ELF64 validation"),
            Self::OutOfMemory => f.write_str("out of memory while allocating program pages"),
            Self::MappingFailed => f.write_str("failed to map program into user address space"),
            Self::SpawnFailed => f.write_str("failed to start user task"),
            Self::Io => f.write_str("I/O error while loading program"),
        }
    }
}

/// Shared result alias for process exec/load operations.
pub type ExecResult<T> = Result<T, ExecError>;

/// Materialized process image ready to be passed to scheduler spawn logic.
#[must_use = "LoadedProgram owns a user address space (cr3) and must be consumed or explicitly handled"]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoadedProgram {
    /// Address-space root (physical PML4 address) for this process.
    pub cr3: u64,

    /// Initial ring-3 instruction pointer.
    pub entry_rip: u64,

    /// Initial ring-3 stack pointer.
    pub user_rsp: u64,

    /// Loaded executable image length in bytes.
    pub image_len: usize,

    /// Total number of mapped pages inside the user code window backing this
    /// program: the sum of every `PT_LOAD` segment's page count.
    ///
    /// This is only a fast-path hint for the first phase of
    /// [`vmm::destroy_user_address_space_with_page_counts`]'s teardown; that
    /// function's catch-all scan reclaims any other present mapping in the
    /// code window regardless of this value, so an undercount here cannot
    /// leak a frame — it can only make teardown fall through to the slower
    /// catch-all path for the pages it missed.
    pub code_page_count: usize,
}

impl LoadedProgram {
    /// Creates a new loaded-program descriptor.
    pub const fn new(
        cr3: u64,
        entry_rip: u64,
        user_rsp: u64,
        image_len: usize,
        code_page_count: usize,
    ) -> Self {
        Self {
            cr3,
            entry_rip,
            user_rsp,
            image_len,
            code_page_count,
        }
    }
}
