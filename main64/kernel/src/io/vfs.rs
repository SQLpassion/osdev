//! Single-mount filesystem facade. One filesystem (FAT32) is mounted at
//! boot; syscalls and the program loader call this instead of a concrete FS.

use crate::sync::spinlock::SpinLock;
use alloc::boxed::Box;
use alloc::vec::Vec;

/// Error conditions for filesystem facade operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FsError {
    /// No filesystem has been mounted yet.
    NotMounted,
    /// Requested file or directory entry was not found.
    NotFound,
    /// File descriptor index is out of bounds or points to an inactive slot.
    InvalidFd,
    /// Operation is not supported by the active filesystem backend (e.g. FAT32 writes).
    Unsupported,
    /// General I/O or transport-layer error occurred during execution.
    Io,
    /// The provided filename is invalid or cannot be parsed.
    InvalidName,
}

/// File opening mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    Append,
}

/// Operations the syscall layer + loader need. Writes may return `Unsupported`.
pub trait FileSystem: Send + Sync {
    /// Open a file by name. Returns the file descriptor index.
    fn open(&self, name: &str, mode: FileMode) -> Result<usize, FsError>;

    /// Close an active file descriptor.
    fn close(&self, fd: usize) -> Result<(), FsError>;

    /// Read data from an active file descriptor. Returns the number of bytes read.
    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Write data to an active file descriptor. Returns the number of bytes written.
    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Adjust the offset cursor of an active file descriptor.
    fn seek(&self, fd: usize, offset: u32) -> Result<(), FsError>;

    /// Return whether the offset cursor has reached or passed the end of the file.
    fn eof(&self, fd: usize) -> Result<bool, FsError>;

    /// Delete a file by name.
    fn delete(&self, name: &str) -> Result<(), FsError>;

    /// Whole-file read helper for the program loader.
    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError>;

    /// Print the root directory listing to the active system console.
    fn print_root_directory(&self);
}

static MOUNTED_FS: SpinLock<Option<Box<dyn FileSystem>>> = SpinLock::new(None);

/// Mount the active global filesystem. Call once during kernel boot path.
pub fn mount(fs: Box<dyn FileSystem>) {
    *MOUNTED_FS.lock() = Some(fs);
}

/// Executes a closure with a stable shared reference to the mounted filesystem.
///
/// Thread-safe: releases the spinlock before invoking the closure so the
/// backend file operations can block/yield without disabling interrupts.
fn with<R>(f: impl FnOnce(&dyn FileSystem) -> Result<R, FsError>) -> Result<R, FsError> {
    // Step 1: Acquire the lock briefly to copy out the raw fat pointer to the trait object.
    let ptr: *const dyn FileSystem = {
        let guard = MOUNTED_FS.lock();
        match guard.as_deref() {
            Some(fs) => fs as *const dyn FileSystem,
            None => return Err(FsError::NotMounted),
        }
    }; // The guard is dropped here, unlocking MOUNTED_FS and enabling interrupts.

    // SAFETY:
    // - `MOUNTED_FS` is a mount-once structure set during kernel boot and never replaced or freed.
    // - The `Box` containing the traits object remains allocated for the entire kernel lifetime.
    // - All backend implementations use interior mutability (such as spinlocks for descriptor tables),
    //   safely allowing concurrent access through immutable references.
    f(unsafe { &*ptr })
}

// Facade helpers used by syscalls and the loader.

/// Open a file by name. Returns the file descriptor index.
pub fn open(name: &str, mode: FileMode) -> Result<usize, FsError> {
    with(|fs| fs.open(name, mode))
}

/// Close an active file descriptor.
pub fn close(fd: usize) -> Result<(), FsError> {
    with(|fs| fs.close(fd))
}

/// Read data from an active file descriptor. Returns the number of bytes read.
pub fn read(fd: usize, buf: &mut [u8]) -> Result<usize, FsError> {
    with(|fs| fs.read(fd, buf))
}

/// Write data to an active file descriptor. Returns the number of bytes written.
pub fn write(fd: usize, buf: &[u8]) -> Result<usize, FsError> {
    with(|fs| fs.write(fd, buf))
}

/// Adjust the offset cursor of an active file descriptor.
pub fn seek(fd: usize, off: u32) -> Result<(), FsError> {
    with(|fs| fs.seek(fd, off))
}

/// Return whether the offset cursor has reached the end of the file.
pub fn eof(fd: usize) -> Result<bool, FsError> {
    with(|fs| fs.eof(fd))
}

/// Delete a file by name.
pub fn delete(name: &str) -> Result<(), FsError> {
    with(|fs| fs.delete(name))
}

/// Whole-file read helper for the program loader.
pub fn read_file(name: &str) -> Result<Vec<u8>, FsError> {
    with(|fs| fs.read_file(name))
}

/// Print the root directory listing to the active system console.
pub fn print_root_directory() {
    let _ = with(|fs| {
        fs.print_root_directory();
        Ok(())
    });
}

/// Reset the mounted filesystem to None (used for testing state isolation).
pub fn reset_mounted_fs() {
    *MOUNTED_FS.lock() = None;
}
