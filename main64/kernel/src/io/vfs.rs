//! Single-mount filesystem facade. One filesystem (FAT32) is mounted at
//! boot; syscalls and the program loader call this instead of a concrete FS.

use crate::sync::spinlock::SpinLock;
use alloc::boxed::Box;
use alloc::sync::Arc;
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
    /// The mounted volume does not conform to the filesystem's expected on-disk
    /// structure (e.g. a bad boot-sector signature or an unsupported geometry).
    NotFat32,
    /// The requested name resolves to a directory, not a readable file.
    IsDirectory,
    /// A loop or structurally invalid entry was encountered while following the
    /// on-disk allocation chain for a file or directory.
    BadChain,
    /// The requested file exceeds the backend's defensively defined maximum size.
    TooLarge,
}

/// File opening mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileMode {
    Read,
    Write,
    Append,
}

/// Operations the syscall layer + loader need. Writes may return `Unsupported`.
///
/// # Fd-ownership contract (MUST)
///
/// File descriptors are not globally shared: each fd is created by `open()` on
/// behalf of the calling task and is only valid for that task afterwards. Every
/// method below that takes an `fd` **MUST** verify, before performing the
/// operation, that `fd` was opened by the task currently identified by
/// `crate::scheduler::current_task_id()` — and return `FsError::InvalidFd`
/// (the same error used for an unknown/closed fd) if it was opened by a
/// different task. This holds for `close`, `read`, `write`, `seek`, and `eof`.
///
/// This check is part of the trait's contract rather than something the VFS
/// facade (`with()`/the free functions below) enforces on the caller's behalf:
/// the facade just forwards the fd to the mounted backend. A backend that
/// omits the ownership check would silently allow one task to read, seek, or
/// close file descriptors belonging to another task. The in-tree FAT32
/// backend (`Fat32Fs` in `io/fat32.rs`) enforces this per-call; any future
/// second backend implementing `FileSystem` MUST do the same.
pub trait FileSystem: Send + Sync {
    /// Open a file by name. Returns the file descriptor index.
    fn open(&self, name: &str, mode: FileMode) -> Result<usize, FsError>;

    /// Close an active file descriptor.
    ///
    /// MUST reject `fd` with `FsError::InvalidFd` if it is not owned by the
    /// calling task (see the fd-ownership contract on this trait).
    fn close(&self, fd: usize) -> Result<(), FsError>;

    /// Read data from an active file descriptor. Returns the number of bytes read.
    ///
    /// MUST reject `fd` with `FsError::InvalidFd` if it is not owned by the
    /// calling task (see the fd-ownership contract on this trait).
    fn read(&self, fd: usize, buf: &mut [u8]) -> Result<usize, FsError>;

    /// Write data to an active file descriptor. Returns the number of bytes written.
    ///
    /// MUST reject `fd` with `FsError::InvalidFd` if it is not owned by the
    /// calling task (see the fd-ownership contract on this trait).
    fn write(&self, fd: usize, buf: &[u8]) -> Result<usize, FsError>;

    /// Adjust the offset cursor of an active file descriptor.
    ///
    /// MUST reject `fd` with `FsError::InvalidFd` if it is not owned by the
    /// calling task (see the fd-ownership contract on this trait).
    fn seek(&self, fd: usize, offset: u32) -> Result<(), FsError>;

    /// Return whether the offset cursor has reached or passed the end of the file.
    ///
    /// MUST reject `fd` with `FsError::InvalidFd` if it is not owned by the
    /// calling task (see the fd-ownership contract on this trait).
    fn eof(&self, fd: usize) -> Result<bool, FsError>;

    /// Delete a file by name.
    fn delete(&self, name: &str) -> Result<(), FsError>;

    /// Whole-file read helper for the program loader.
    fn read_file(&self, name: &str) -> Result<Vec<u8>, FsError>;

    /// Print the root directory listing to the active system console.
    fn print_root_directory(&self);

    /// Closes all file descriptors owned by the specified task ID.
    fn close_task_fds(&self, task_id: usize);
}

// `Arc` (rather than `Box`) is load-bearing here: `with()` below must release
// `MOUNTED_FS`'s spinlock before calling into the backend (see its doc comment),
// which means it cannot keep borrowing through the guard. Cloning an `Arc` is a
// cheap atomic refcount bump taken while the lock is held; the clone then keeps
// the filesystem allocation alive independently of the global slot for as long
// as the closure runs, even if `mount()`/`reset_mounted_fs()` replace the slot
// concurrently. This makes the use-after-free that motivated this module
// structurally impossible, without holding the lock across blocking disk I/O.
static MOUNTED_FS: SpinLock<Option<Arc<dyn FileSystem>>> = SpinLock::new(None);

/// Mount the active global filesystem. Call once during kernel boot path.
///
/// Write-once: if a filesystem is already mounted, this call is ignored and
/// the existing mount is left in place. This prevents a stray or duplicate
/// boot-path call from silently replacing (and dropping) a filesystem that
/// other code may still be using, which is the production-facing half of the
/// use-after-free concern this module guards against — see `with()` below for
/// the other half (in-flight borrows surviving a concurrent mount/reset).
pub fn mount(fs: Box<dyn FileSystem>) {
    // Step 1: Take the lock and check whether a filesystem is already mounted.
    let mut guard = MOUNTED_FS.lock();
    if guard.is_some() {
        crate::debugln!("vfs::mount: filesystem already mounted; ignoring redundant mount() call");
        return;
    }

    // Step 2: First mount: convert the incoming `Box` into an `Arc` so `with()`
    // can hand out cheap, independently-owned clones (see `MOUNTED_FS` above).
    *guard = Some(Arc::from(fs));
}

/// Executes a closure with a stable shared reference to the mounted filesystem.
///
/// Thread-safe: releases the spinlock before invoking the closure so the
/// backend file operations can block/yield without disabling interrupts.
///
/// The mount lock (`MOUNTED_FS`) is never held across blocking disk I/O.
/// Instead, the guard is used only to clone the `Arc<dyn FileSystem>` (a cheap
/// atomic refcount bump), and is dropped before the backend call. The cloned
/// `Arc` keeps the filesystem allocation alive for the duration of the
/// closure regardless of what `mount()` or `reset_mounted_fs()` do to the
/// global slot in the meantime — there is no dangling pointer to it, because
/// no raw pointer into the slot is ever taken.
fn with<R>(f: impl FnOnce(&dyn FileSystem) -> Result<R, FsError>) -> Result<R, FsError> {
    // Step 1: Acquire the lock briefly to clone out the Arc handle to the trait object.
    // Keep this critical section as short as possible; the backend call below
    // may yield (ATA/AHCI waits), so holding the mount lock here would deadlock
    // on single-core preemptive I/O.
    let fs: Arc<dyn FileSystem> = {
        let guard = MOUNTED_FS.lock();
        match guard.as_ref() {
            Some(fs) => Arc::clone(fs),
            None => return Err(FsError::NotMounted),
        }
    }; // The guard is dropped here, unlocking MOUNTED_FS and enabling interrupts.

    // No `unsafe` is required: `fs` is an owned `Arc` clone, so the reference
    // handed to the closure is backed by this call's own refcount and remains
    // valid for the closure's entire lifetime, independent of the global slot.
    f(&*fs)
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

/// Closes all file descriptors owned by the specified task ID.
pub fn close_task_fds(task_id: usize) {
    let _ = with(|fs| {
        fs.close_task_fds(task_id);
        Ok(())
    });
}

/// Test-only reset of the mounted filesystem back to `None`.
///
/// Hidden from public docs; used by integration tests to isolate state
/// between test cases so each can start from an unmounted VFS or remount a
/// fresh backend (`mount()` is write-once in production, see above, and would
/// otherwise reject a test's second `mount()` call).
///
/// Not gated behind `#[cfg(test)]`: this crate builds with `[lib] test = false`
/// and integration tests under `tests/*.rs` link `kaos_kernel` as an ordinary
/// (non-`--cfg test`) dependency, so `#[cfg(test)]` items in `src/` are never
/// visible to them. `#[doc(hidden)]` plus this doc comment is the same
/// test-only-escape-hatch convention already used elsewhere in this crate
/// (e.g. `Fat32Volume::for_test`, `block::reset_active_device`,
/// `scheduler::reset_initialization_for_test`).
///
/// Calling this from non-test code cannot reintroduce the use-after-free this
/// module guards against: `with()` never holds a raw pointer into
/// `MOUNTED_FS` across the closure, so even a misuse of this function while a
/// `with()` call is in flight only drops the global slot's `Arc` handle — the
/// clone held by the in-flight closure keeps the filesystem allocation alive
/// until that closure returns.
///
/// Residual caveat (not a memory-safety issue, but a logical foot-gun): this
/// function is intentionally ungated (no `#[cfg(test)]`) for the reasons
/// above, so nothing at the type level stops production code from calling it.
/// Doing so while another task's `with()` call is in flight does not corrupt
/// memory, but it does unmount the filesystem out from under that in-flight
/// operation's *subsequent* facade calls (e.g. a task mid-`read_file` that
/// then calls `open` again would see `FsError::NotMounted`), and any later
/// `mount()` call is a no-op only relative to whatever got mounted last. This
/// function must therefore only ever be called from single-threaded test
/// setup/teardown, never from a code path reachable while the kernel is
/// otherwise running.
#[doc(hidden)]
pub fn reset_mounted_fs() {
    *MOUNTED_FS.lock() = None;
}
