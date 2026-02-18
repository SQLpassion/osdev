//! Process loading/execution contracts.
//!
//! Phase 5 adds end-to-end FAT12 exec wiring through scheduler user-task spawn.

mod loader;
mod types;

#[allow(unused_imports)]
pub use types::{
    image_fits_user_code, ExecError, ExecResult, LoadedProgram, USER_PROGRAM_ENTRY_RIP,
    USER_PROGRAM_INITIAL_RSP, USER_PROGRAM_MAX_IMAGE_SIZE, USER_PROGRAM_STACK_ALIGNMENT,
};

#[allow(unused_imports)]
pub use loader::{
    exec_from_fat12, load_program_image, load_program_into_user_address_space,
    map_program_image_into_user_address_space, validate_program_image_len,
};
