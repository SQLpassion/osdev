//! FAT32 integration tests
//!
//! Verifies the pure parsing/logic functions of the FAT32 implementation.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::io::fat32;

/// Entry point for the integration test kernel.
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();

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

// ============================================================================
// Integration Tests
// ============================================================================

#[test_case]
fn test_normalize_name_valid() {
    assert_eq!(fat32::normalize_name("shell.bin"), Some(*b"SHELL   BIN"));
    assert_eq!(fat32::normalize_name("KERNEL.BIN"), Some(*b"KERNEL  BIN"));
    assert_eq!(fat32::normalize_name("A.B"), Some(*b"A       B  "));
    assert_eq!(fat32::normalize_name("NOEXT"), Some(*b"NOEXT      "));
    assert_eq!(fat32::normalize_name("12345678.123"), Some(*b"12345678123"));
}

#[test_case]
fn test_normalize_name_invalid() {
    // Base name too long
    assert_eq!(fat32::normalize_name("toolongname.bin"), None);
    // Extension too long
    assert_eq!(fat32::normalize_name("shell.long"), None);
    // Multiple dots
    assert_eq!(fat32::normalize_name("a.b.c"), None);
}

/// Verifies that cluster_to_lba rejects clusters outside the valid data-cluster range.
/// This is the FAT32 equivalent of the R-21 upper-bound check.
#[test_case]
fn test_cluster_to_lba_rejects_out_of_range_clusters() {
    // Construct a minimal volume with 4 valid data clusters (2..=5).
    // data_start_lba=100, sec_per_clus=1 means cluster 2 -> LBA 100, cluster 5 -> LBA 103.
    let volume = fat32::Fat32Volume::for_test(0, 512, 1, 0, 100, 2, 5);

    // Valid boundary clusters must translate correctly.
    assert_eq!(volume.cluster_to_lba_for_test(2).unwrap(), 100);
    assert_eq!(volume.cluster_to_lba_for_test(5).unwrap(), 103);

    // Cluster 0 and 1 are reserved and must be rejected.
    assert!(volume.cluster_to_lba_for_test(0).is_err());
    assert!(volume.cluster_to_lba_for_test(1).is_err());

    // Cluster 6 is beyond max_data_cluster and must be rejected immediately.
    assert!(volume.cluster_to_lba_for_test(6).is_err());
}
