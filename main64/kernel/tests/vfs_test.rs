//! VFS integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use alloc::vec::Vec;
use core::panic::PanicInfo;
use kaos_kernel::io::vfs::{self, FileMode, FileSystem, FsError};
use kaos_kernel::memory::{heap, pmm, vmm};

/// Entry point for the VFS integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

    pmm::init(false);
    kaos_kernel::arch::interrupts::init();
    vmm::init(false);
    heap::init(false);

    // Initialize ATA disk driver and block device manager.
    kaos_kernel::drivers::ata::init();
    kaos_kernel::drivers::block::init_ata();

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: VFS operations return NotMounted before a filesystem is mounted.
/// Given: The global MOUNTED_FS is None (initial state).
/// When: VFS open or read_file is invoked.
/// Then: They must return FsError::NotMounted.
#[test_case]
fn test_vfs_unmounted_returns_not_mounted() {
    vfs::reset_mounted_fs();
    let result = vfs::open("anyfile.txt", FileMode::Read);
    assert!(
        matches!(result, Err(FsError::NotMounted)),
        "open must return NotMounted when no FS is mounted"
    );

    let read_result = vfs::read_file("anyfile.txt");
    assert!(
        matches!(read_result, Err(FsError::NotMounted)),
        "read_file must return NotMounted when no FS is mounted"
    );
}

/// Contract: VFS opens fail with NotFound for missing files on disk.
/// Given: A valid Fat32Fs filesystem is mounted.
/// When: open is called on a non-existent filename.
/// Then: The call must return FsError::NotFound.
#[test_case]
fn test_vfs_mounted_open_missing_file_returns_not_found() {
    // Step 1: Reset any FS left mounted by a previous test, then mount the FAT32
    // filesystem (superfloppy VBR at LBA 0). The reset is required because `mount()`
    // is write-once in production (see vfs.rs) and would otherwise silently ignore
    // this call if a previous test (in this same QEMU boot/test binary) left a
    // filesystem mounted, regardless of `#[test_case]` execution order.
    vfs::reset_mounted_fs();
    let vol = kaos_kernel::io::fat32::Fat32Volume::mount(0).expect("FAT32 must mount at LBA 0");
    vfs::mount(Box::new(kaos_kernel::io::fat32::Fat32Fs::new(vol)));

    // Step 2: Attempt to open a missing file.
    let result = vfs::open("missing.txt", FileMode::Read);
    assert!(
        matches!(result, Err(FsError::NotFound)),
        "opening a missing file must return NotFound, got {:?}",
        result
    );
}

/// Contract: VFS read/seek/close/eof return InvalidFd for invalid file descriptor indices,
/// and write returns Unsupported on the read-only FAT32 backend.
/// Given: A valid (read-only) FAT32 filesystem is mounted.
/// When: Close, read, seek, or eof are called with a bogus file descriptor (e.g. 9999).
/// Then: They must return FsError::InvalidFd. Write is rejected as Unsupported before any
/// fd check because FAT32 is read-only.
#[test_case]
fn test_vfs_invalid_fd_returns_invalid_fd() {
    // Step 1: Reset any FS left mounted by a previous test, then mount the FAT32
    // filesystem (superfloppy VBR at LBA 0). See the comment in
    // `test_vfs_mounted_open_missing_file_returns_not_found` for why the reset is
    // required now that `mount()` is write-once.
    vfs::reset_mounted_fs();
    let vol = kaos_kernel::io::fat32::Fat32Volume::mount(0).expect("FAT32 must mount at LBA 0");
    vfs::mount(Box::new(kaos_kernel::io::fat32::Fat32Fs::new(vol)));

    // Step 2: Issue file operations with a bad descriptor ID.
    let bad_fd = 9999;
    let mut buf = [0u8; 10];

    assert!(
        matches!(vfs::close(bad_fd), Err(FsError::InvalidFd)),
        "close must reject invalid fd"
    );
    assert!(
        matches!(vfs::read(bad_fd, &mut buf), Err(FsError::InvalidFd)),
        "read must reject invalid fd"
    );
    assert!(
        matches!(vfs::write(bad_fd, &buf), Err(FsError::Unsupported)),
        "write must be unsupported on the read-only FAT32 backend"
    );
    assert!(
        matches!(vfs::seek(bad_fd, 0), Err(FsError::InvalidFd)),
        "seek must reject invalid fd"
    );
    assert!(
        matches!(vfs::eof(bad_fd), Err(FsError::InvalidFd)),
        "eof must reject invalid fd"
    );
}

/// Minimal `FileSystem` test double whose `open()` returns a caller-chosen
/// marker value as the "file descriptor". Used below to tell, from the
/// outside, which of two mounted instances is actually backing the facade
/// (the real `Fat32Fs` gives no such externally observable identity).
struct MarkerFs {
    marker: usize,
}

impl FileSystem for MarkerFs {
    fn open(&self, _name: &str, _mode: FileMode) -> Result<usize, FsError> {
        Ok(self.marker)
    }

    fn close(&self, _fd: usize) -> Result<(), FsError> {
        Ok(())
    }

    fn read(&self, _fd: usize, _buf: &mut [u8]) -> Result<usize, FsError> {
        Ok(0)
    }

    fn write(&self, _fd: usize, _buf: &[u8]) -> Result<usize, FsError> {
        Err(FsError::Unsupported)
    }

    fn seek(&self, _fd: usize, _offset: u32) -> Result<(), FsError> {
        Ok(())
    }

    fn eof(&self, _fd: usize) -> Result<bool, FsError> {
        Ok(true)
    }

    fn delete(&self, _name: &str) -> Result<(), FsError> {
        Err(FsError::Unsupported)
    }

    fn read_file(&self, _name: &str) -> Result<Vec<u8>, FsError> {
        Ok(Vec::new())
    }

    fn print_root_directory(&self) {}

    fn close_task_fds(&self, _task_id: usize) {}
}

/// Contract: `mount()` is write-once — once a filesystem is mounted, a second
/// `mount()` call must not replace (and therefore cannot drop) the active one.
/// Given: No filesystem is mounted.
/// When: `mount()` is called twice in a row with two distinguishable backends.
/// Then: The facade must keep dispatching to the *first* backend; the second
/// `mount()` call must be a no-op.
///
/// What this proves vs. what's structurally guaranteed: this is a single-core,
/// cooperative test harness with no real concurrent callers, so it cannot
/// reproduce the original racing mount/with() scenario from the issue
/// end-to-end. What it does concretely verify is the write-once guard in
/// `mount()` itself — the production-facing half of the fix. The other half
/// (an in-flight `with()` borrow surviving a concurrent mount/reset) is no
/// longer reachable by construction, because `with()` operates on an owned
/// `Arc` clone rather than a raw pointer into the global slot; that part is
/// guaranteed by `Arc`'s refcounting semantics, not by anything a
/// single-threaded test can directly exercise.
#[test_case]
fn test_vfs_mount_is_write_once() {
    // Step 1: Start from a clean, unmounted state.
    vfs::reset_mounted_fs();

    // Step 2: Mount the first backend, then attempt to mount a second, distinct
    // backend on top of it.
    vfs::mount(Box::new(MarkerFs { marker: 111 }));
    vfs::mount(Box::new(MarkerFs { marker: 222 }));

    // Step 3: The facade must still be dispatching to the first backend.
    let fd = vfs::open("whatever", FileMode::Read)
        .expect("open must succeed once a filesystem is mounted");
    assert_eq!(
        fd, 111,
        "a second mount() call must be ignored; the originally mounted FS must remain active"
    );

    // Step 4: Leave the global mount state clean for whichever test runs next
    // (execution order across #[test_case] functions is not guaranteed).
    vfs::reset_mounted_fs();
}

/// Contract: the mounted filesystem stays valid and usable across a
/// reset+remount cycle; the reset does not leave the facade in a broken or
/// stale state.
/// Given: A FAT32 filesystem is mounted and a known file can be read from it.
/// When: The mount is reset to `None` and then a fresh FAT32 volume is mounted
/// again.
/// Then: The facade must report `NotMounted` while reset, and must again
/// successfully read the same file once remounted, with matching content.
#[test_case]
fn test_vfs_reset_and_remount_cycle() {
    // Step 1: Mount the FAT32 filesystem and confirm a known file is readable.
    vfs::reset_mounted_fs();
    let vol = kaos_kernel::io::fat32::Fat32Volume::mount(0).expect("FAT32 must mount at LBA 0");
    vfs::mount(Box::new(kaos_kernel::io::fat32::Fat32Fs::new(vol)));
    let first_read =
        vfs::read_file("HELLO.BIN").expect("HELLO.BIN must be readable on the first mount");
    assert!(!first_read.is_empty(), "HELLO.BIN must not be empty");

    // Step 2: Reset the mount and confirm the facade reports NotMounted.
    vfs::reset_mounted_fs();
    assert!(
        matches!(vfs::read_file("HELLO.BIN"), Err(FsError::NotMounted)),
        "read_file must return NotMounted immediately after reset_mounted_fs()"
    );

    // Step 3: Remount a fresh volume and confirm the same file reads back
    // identically, proving the facade is fully usable again (not left in a
    // half-torn-down state by the reset).
    let vol_again =
        kaos_kernel::io::fat32::Fat32Volume::mount(0).expect("FAT32 must remount at LBA 0");
    vfs::mount(Box::new(kaos_kernel::io::fat32::Fat32Fs::new(vol_again)));
    let second_read =
        vfs::read_file("HELLO.BIN").expect("HELLO.BIN must be readable again after remount");
    assert_eq!(
        first_read, second_read,
        "file contents must be identical across the reset+remount cycle"
    );
}
