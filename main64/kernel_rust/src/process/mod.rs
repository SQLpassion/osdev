//! Process loading/execution contracts.
//!
//! Phase 1 intentionally exposes only shared types and constants. Loader and
//! runtime exec logic are added in follow-up commits.

mod types;

#[allow(unused_imports)]
pub use types::{
    image_fits_user_code, ExecError, ExecResult, LoadedProgram, USER_PROGRAM_ENTRY_RIP,
    USER_PROGRAM_INITIAL_RSP, USER_PROGRAM_MAX_IMAGE_SIZE, USER_PROGRAM_STACK_ALIGNMENT,
};
