//! Shared process/exec contracts for user-mode program loading.

use crate::memory::vmm;

/// Fixed ring-3 entry point for flat user binaries in phase 1.
pub const USER_PROGRAM_ENTRY_RIP: u64 = vmm::USER_CODE_BASE;

/// User stack alignment used by scheduler/user task bootstrap frames.
pub const USER_PROGRAM_STACK_ALIGNMENT: u64 = 16;

/// Initial user-mode stack pointer used when spawning a fresh process.
///
/// `iretq` restores this as ring-3 RSP. Keeping a 16-byte aligned stack
/// preserves ABI expectations for function prologues in user binaries.
pub const USER_PROGRAM_INITIAL_RSP: u64 = vmm::USER_STACK_TOP - USER_PROGRAM_STACK_ALIGNMENT;

/// Maximum executable image size accepted by the phase-1 flat loader.
pub const USER_PROGRAM_MAX_IMAGE_SIZE: usize = vmm::USER_CODE_SIZE as usize;

/// Returns whether a flat image length fits inside the configured user code window.
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

    /// Physical-frame allocation failed while preparing code/stack pages.
    OutOfMemory,

    /// Mapping code/stack pages into user space failed.
    MappingFailed,

    /// Spawning the scheduler task for the process failed.
    SpawnFailed,

    /// Generic storage or transport I/O failure.
    Io,
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

    /// Number of mapped code pages backing `image_len`.
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
