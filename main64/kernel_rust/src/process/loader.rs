//! FAT12-backed loader for flat user-mode binaries.

use alloc::vec::Vec;

use crate::io::fat12::{self, Fat12Error};

use super::types::{image_fits_user_code, ExecError, ExecResult};

/// Loads a flat user program from FAT12 and validates its image length.
///
/// Phase 2 scope:
/// - read file content from FAT12
/// - map FAT12-level errors into process exec errors
/// - reject images larger than the configured user code window
///
/// Caller requirements:
/// - ATA driver must be initialized before calling this function
/// - FAT12 layer must be initialized as part of normal kernel boot
///
/// Not part of this function:
/// - creating a dedicated user address space
/// - mapping code/stack pages
/// - spawning a scheduler task
pub fn load_program_image(file_name_8_3: &str) -> ExecResult<Vec<u8>> {
    let image = fat12::read_file(file_name_8_3).map_err(map_fat12_error)?;
    validate_program_image_len(image.len())?;

    Ok(image)
}

/// Validates that a program image length fits inside the user executable window.
#[inline]
pub const fn validate_program_image_len(image_len: usize) -> ExecResult<()> {
    if image_fits_user_code(image_len) {
        Ok(())
    } else {
        Err(ExecError::FileTooLarge)
    }
}

fn map_fat12_error(error: Fat12Error) -> ExecError {
    match error {
        Fat12Error::InvalidFileName => ExecError::InvalidName,
        Fat12Error::NotFound => ExecError::NotFound,
        Fat12Error::IsDirectory => ExecError::IsDirectory,
        Fat12Error::Ata(_)
        | Fat12Error::CorruptDirectoryEntry
        | Fat12Error::CorruptFatChain
        | Fat12Error::UnexpectedEof => ExecError::Io,
    }
}
