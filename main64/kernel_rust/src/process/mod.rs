//! Process loading/execution contracts.
//!
//! Phase 2 adds a FAT12-backed loader that reads and validates flat user
//! binaries. Address-space mapping and runtime spawn logic remain follow-up work.

mod loader;
mod types;

#[allow(unused_imports)]
pub use types::{
    image_fits_user_code, ExecError, ExecResult, LoadedProgram, USER_PROGRAM_ENTRY_RIP,
    USER_PROGRAM_INITIAL_RSP, USER_PROGRAM_MAX_IMAGE_SIZE, USER_PROGRAM_STACK_ALIGNMENT,
};

#[allow(unused_imports)]
pub use loader::{load_program_image, validate_program_image_len};
