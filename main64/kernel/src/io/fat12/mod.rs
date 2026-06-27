//! FAT12 File System Driver
//!
//! Reads the FAT12 root directory from disk and provides functionality
//! to list its entries. The FAT12 layout matches a standard 1.44 MB
//! floppy disk image used by the KAOS bootloader.

pub mod cluster;
pub mod directory;
pub mod disk;
pub mod fd;
pub mod fs;
pub mod types;

#[allow(unused_imports)]
pub use directory::normalize_8_3_name;
#[allow(unused_imports)]
pub use fd::{close_file, eof_file, open_file, read_file_fd, seek_file, write_file_fd};
#[allow(unused_imports)]
pub use fs::{delete_file, init, parse_root_directory, print_root_directory, read_file};
#[allow(unused_imports)]
pub use types::{Fat12Error, FileDescriptor, FileMode, RootDirectoryRecord};
