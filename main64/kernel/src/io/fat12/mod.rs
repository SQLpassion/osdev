//! FAT12 File System Driver
//!
//! Reads the FAT12 root directory from disk and provides functionality
//! to list its entries. The FAT12 layout matches a standard 1.44 MB
//! floppy disk image used by the KAOS bootloader.

pub mod types;
pub mod disk;
pub mod cluster;
pub mod directory;
pub mod fs;
pub mod fd;

#[allow(unused_imports)]
pub use types::{Fat12Error, RootDirectoryRecord, FileMode, FileDescriptor};
#[allow(unused_imports)]
pub use directory::normalize_8_3_name;
#[allow(unused_imports)]
pub use fs::{init, read_file, parse_root_directory, print_root_directory, delete_file};
#[allow(unused_imports)]
pub use fd::{open_file, close_file, seek_file, eof_file, read_file_fd, write_file_fd};
