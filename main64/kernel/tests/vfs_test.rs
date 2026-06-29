//! VFS integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::boxed::Box;
use core::panic::PanicInfo;
use kaos_kernel::io::vfs::{self, FileMode, FsError};
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
/// Given: A valid Fat12Fs filesystem is mounted.
/// When: open is called on a non-existent filename.
/// Then: The call must return FsError::NotFound.
#[test_case]
fn test_vfs_mounted_open_missing_file_returns_not_found() {
    // Step 1: Mount the FAT12 filesystem.
    vfs::mount(Box::new(kaos_kernel::io::fat12::Fat12Fs));

    // Step 2: Attempt to open a missing file.
    let result = vfs::open("missing.txt", FileMode::Read);
    assert!(
        matches!(result, Err(FsError::NotFound)),
        "opening a missing file must return NotFound, got {:?}",
        result
    );
}

/// Contract: VFS read/write/seek/close return InvalidFd for invalid file descriptor indices.
/// Given: A valid filesystem is mounted.
/// When: Close, read, write, seek, or eof are called with a bogus file descriptor (e.g. 9999).
/// Then: They must return FsError::InvalidFd.
#[test_case]
fn test_vfs_invalid_fd_returns_invalid_fd() {
    // Step 1: Mount the FAT12 filesystem.
    vfs::mount(Box::new(kaos_kernel::io::fat12::Fat12Fs));

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
        matches!(vfs::write(bad_fd, &buf), Err(FsError::InvalidFd)),
        "write must reject invalid fd"
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
