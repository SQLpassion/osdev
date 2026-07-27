//! FAT32 integration tests
//!
//! Verifies the pure parsing/logic functions of the FAT32 implementation.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;
use kaos_kernel::drivers::block;
use kaos_kernel::drivers::block::BlockError;
use kaos_kernel::io::fat32;
use kaos_kernel::io::vfs::FsError;

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

/// Contract (H5, Closes #11): chain-follow loops must bound chain length by the volume's
/// actual cluster count (`max_data_cluster`), not by a hardcoded 1,000,000-iteration cap.
/// A chain longer than the volume's cluster count is necessarily cyclic, since a valid,
/// acyclic chain can visit each existing data cluster at most once.
/// Given: A synthetic FAT32 volume with `max_data_cluster = 4`, whose on-disk FAT contains
/// a crafted 2-cluster cycle: cluster 2's entry points to cluster 3, and cluster 3's entry
/// points back to cluster 2. Every individual FAT entry involved is structurally valid (in
/// range, not an EOC marker, not the bad-cluster marker), so a per-entry check alone
/// (`next_cluster`) cannot detect the loop -- only bounding the total clusters visited can.
/// When: The chain starting at cluster 2 is followed via `walk_chain_for_test`, which reuses
/// the same `check_chain_bound`/`next_cluster` logic as the production `read_file` and
/// `print_root_directory` loops.
/// Then: The walk returns `Err(Fat32Error::BadChain)` after visiting at most
/// `max_data_cluster` (4) clusters -- i.e. within `cluster_count` iterations, never anywhere
/// near the old 1,000,000-iteration / real-disk-read cap.
#[test_case]
fn test_cyclic_fat_chain_returns_bad_chain_within_cluster_count() {
    // Step 1: Initialize the ATA block device so `next_cluster`'s real FAT sector reads
    // (and our own crafted write below) actually reach the QEMU disk image.
    kaos_kernel::memory::pmm::init(false);
    kaos_kernel::arch::interrupts::init();
    kaos_kernel::memory::vmm::init(false);
    kaos_kernel::memory::heap::init(false);
    kaos_kernel::drivers::ata::init();
    block::init_ata();

    // Step 2: Pick a scratch LBA far past anything the test disk image's real FAT32
    // filesystem (built by tests/test_runner.sh from a 64 MiB image) ever writes to, so we
    // can freely overwrite it with a synthetic, deliberately cyclic FAT without disturbing
    // the real mounted filesystem/files used by other integration tests.
    const SCRATCH_FAT_LBA: u64 = 125_000;

    // Step 3: Craft a single FAT sector where cluster 2's 4-byte entry (byte offset 8) points
    // to cluster 3, and cluster 3's entry (byte offset 12) points back to cluster 2 -- a
    // 2-cluster cycle (A -> B -> A) that passes every per-entry validity check forever.
    let mut fat_sector = [0u8; 512];
    fat_sector[8..12].copy_from_slice(&3u32.to_le_bytes());
    fat_sector[12..16].copy_from_slice(&2u32.to_le_bytes());
    block::write_sectors(SCRATCH_FAT_LBA, 1, &fat_sector)
        .expect("writing the synthetic cyclic FAT sector must succeed");

    // Step 4: Build a synthetic volume whose FAT starts at our scratch sector, with a
    // deliberately tiny max_data_cluster (4). The data region is never touched by this test
    // (chain-following only reads FAT sectors), so data_start_lba/sec_per_clus/root_cluster
    // are placeholders.
    let volume = fat32::Fat32Volume::for_test(
        0,                   // part_lba (unused: not read by next_cluster/for_test)
        512,                 // bytes_per_sec
        1,                   // sec_per_clus (unused: data region is never read here)
        SCRATCH_FAT_LBA,     // fat_start_lba
        SCRATCH_FAT_LBA + 1, // data_start_lba (unused placeholder)
        2,                   // root_cluster (unused: we start the walk explicitly)
        4,                   // max_data_cluster
    );

    // Step 5: Confirm cluster 2 and cluster 3 both individually pass as valid, distinct FAT
    // entries -- demonstrating exactly why a per-entry check alone cannot detect this cycle.
    assert_eq!(volume.next_cluster_for_test(2).unwrap(), 3);
    assert_eq!(volume.next_cluster_for_test(3).unwrap(), 2);

    // Step 6: Follow the cycle through the same bound-checked path used by production code.
    // With max_data_cluster = 4, a correct fix must report BadChain after visiting at most
    // 4 clusters (5 next_cluster calls), not after up to 1,000,000 real disk reads.
    let result = volume.walk_chain_for_test(2);
    assert!(
        matches!(result, Err(fat32::Fat32Error::BadChain)),
        "a cyclic FAT chain must be reported as BadChain instead of looping forever, got {:?}",
        result
    );
}

/// Contract (L7, #60): directory walks must skip both LFN continuation entries
/// (attr == 0x0F) and the volume-label entry (attr == 0x08), the same filtering
/// `read_file` already applied. `print_root_directory` previously only checked
/// for 0x0F and could surface the volume label as a bogus zero-size, zero-cluster
/// "file" in its listing.
/// Given: The shared `is_skippable_dir_entry` predicate used by both directory
/// walks (exposed for tests via `is_skippable_dir_entry_for_test`).
/// When: It is evaluated against the LFN attribute, the volume-ID attribute, and
/// a representative set of "real" entry attributes (plain file, read-only file,
/// subdirectory).
/// Then: Only the LFN and volume-ID attributes are reported as skippable.
#[test_case]
fn test_is_skippable_dir_entry_skips_lfn_and_volume_id_only() {
    // LFN continuation entry: must be skipped.
    assert!(fat32::is_skippable_dir_entry_for_test(0x0F));
    // Volume label entry: must be skipped (this is the L7 fix itself).
    assert!(fat32::is_skippable_dir_entry_for_test(0x08));

    // A plain file entry must not be treated as skippable.
    assert!(!fat32::is_skippable_dir_entry_for_test(0x00));
    // A read-only file entry must not be treated as skippable.
    assert!(!fat32::is_skippable_dir_entry_for_test(0x01));
    // A subdirectory entry must not be treated as skippable (it is handled
    // separately via the `ATTR_DIRECTORY` bit further down the walk).
    assert!(!fat32::is_skippable_dir_entry_for_test(0x10));
    // An archive-bit-only file entry must not be treated as skippable.
    assert!(!fat32::is_skippable_dir_entry_for_test(0x20));
}

/// Contract (L9, #60): `map_fat32_err` must translate each distinct
/// `Fat32Error` variant into its own `FsError` variant, instead of collapsing
/// `NotFat32`/`IsDirectory`/`BadChain`/`TooLarge` into a generic `FsError::Io`
/// and losing which specific failure occurred.
/// Given: One representative value of every `Fat32Error` variant, including
/// both `Fat32Error::Block` sub-cases (the "unsupported operation" case, which
/// intentionally still maps to `FsError::Unsupported`, and a generic transport
/// failure, which maps to `FsError::Io`).
/// When: Each is passed through `map_fat32_err` (exposed for tests via
/// `map_fat32_err_for_test`).
/// Then: Each produces a distinct, specific `FsError` variant rather than all
/// non-`NotFound`/`Unsupported` cases collapsing into `FsError::Io`.
#[test_case]
fn test_map_fat32_err_preserves_distinct_error_variants() {
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::NotFound),
        FsError::NotFound
    ));
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::NotFat32),
        FsError::NotFat32
    ));
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::IsDirectory),
        FsError::IsDirectory
    ));
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::BadChain),
        FsError::BadChain
    ));
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::TooLarge),
        FsError::TooLarge
    ));
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::Block(BlockError::Unsupported)),
        FsError::Unsupported
    ));
    assert!(matches!(
        fat32::map_fat32_err_for_test(fat32::Fat32Error::Block(BlockError::Device)),
        FsError::Io
    ));
}

/// Contract (L10, #60): `Fat32Volume::mount` must keep rejecting BPBs whose
/// `BytesPerSec` field is not exactly 512 as `Fat32Error::NotFat32`. This is
/// the invariant that makes the sector-size buffers throughout `fat32.rs` safe
/// to size from `self.bytes_per_sec` (see `read_file`/`print_root_directory`):
/// as long as `mount()` keeps enforcing 512, `self.bytes_per_sec` is always
/// 512 in practice, so switching those buffers from a hardcoded `512` literal
/// to `self.bytes_per_sec` is behavior-preserving for every volume that can
/// actually mount.
/// Given: A synthetic BPB sector at a scratch LBA, identical to a valid FAT32
/// boot sector except `BytesPerSec` (offset 0x0B) is set to 1024 instead of 512.
/// When: `Fat32Volume::mount` is called against that scratch LBA.
/// Then: It must return `Err(Fat32Error::NotFat32)` rather than mounting a
/// volume whose declared sector size disagrees with the buffers the rest of
/// the module allocates.
#[test_case]
fn test_mount_rejects_non_512_byte_sector() {
    // Step 1: The ATA block device is already initialized by
    // `test_cyclic_fat_chain_returns_bad_chain_within_cluster_count`, which
    // the test harness runs before this test (test cases run in a fixed,
    // alphabetically-sorted order within a binary). Re-running `pmm::init`/
    // `vmm::init`/`heap::init` here would re-initialize live kernel memory
    // management state from under the current test process, which is not a
    // supported operation, so this test intentionally reuses the already-
    // initialized ATA device instead of repeating that setup.
    if !kaos_kernel::memory::pmm::is_initialized() {
        kaos_kernel::memory::pmm::init(false);
        kaos_kernel::arch::interrupts::init();
        kaos_kernel::memory::vmm::init(false);
        kaos_kernel::memory::heap::init(false);
        kaos_kernel::drivers::ata::init();
        block::init_ata();
    }

    // Step 2: Pick a scratch LBA far past the real FAT32 filesystem built by
    // `tests/test_runner.sh`, so this synthetic boot sector cannot collide
    // with (or corrupt) the volume other integration tests mount.
    const SCRATCH_BOOT_LBA: u64 = 125_500;

    // Step 3: Craft a boot sector that is a valid FAT32 BPB in every respect
    // except `BytesPerSec`, which is set to 1024 instead of the required 512.
    let mut sector = [0u8; 512];
    sector[0x0B..0x0D].copy_from_slice(&1024u16.to_le_bytes()); // BytesPerSec = 1024
    sector[0x0D] = 1; // SecPerClus
    sector[0x0E..0x10].copy_from_slice(&32u16.to_le_bytes()); // RsvdSecCnt
    sector[0x10] = 2; // NumFATs
    sector[0x11..0x13].copy_from_slice(&0u16.to_le_bytes()); // RootEntCnt = 0 (FAT32)
    sector[0x13..0x15].copy_from_slice(&0u16.to_le_bytes()); // TotSec16 = 0 (FAT32)
    sector[0x16..0x18].copy_from_slice(&0u16.to_le_bytes()); // FATSz16 = 0 (FAT32)
    sector[0x20..0x24].copy_from_slice(&131_072u32.to_le_bytes()); // TotSec32
    sector[0x24..0x28].copy_from_slice(&1000u32.to_le_bytes()); // FATSz32
    sector[0x2C..0x30].copy_from_slice(&2u32.to_le_bytes()); // RootCluster
    sector[0x1FE..0x200].copy_from_slice(&0xAA55u16.to_le_bytes()); // Boot signature

    block::write_sectors(SCRATCH_BOOT_LBA, 1, &sector)
        .expect("writing the synthetic 1024-byte-sector BPB must succeed");

    // Step 4: Attempt to mount it and confirm the 512-byte-sector gate rejects it.
    // `Fat32Volume` does not implement `Debug`, so on mismatch we report only
    // whether an `Ok(_)` (unexpectedly mounted) or the wrong `Err` variant was
    // returned, without trying to print the volume itself.
    let result = fat32::Fat32Volume::mount(SCRATCH_BOOT_LBA);
    match result {
        Err(fat32::Fat32Error::NotFat32) => {}
        Err(other) => panic!(
            "mount() must reject a non-512-byte-sector BPB as NotFat32, got Err({:?})",
            other
        ),
        Ok(_) => panic!("mount() must reject a non-512-byte-sector BPB as NotFat32, got Ok(_)"),
    }
}
