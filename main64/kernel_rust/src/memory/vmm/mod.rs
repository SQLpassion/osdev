//! Virtual memory manager for x86_64 4-level paging with recursive mapping.
//!
//! Design summary:
//! - 4-level paging scheme (PML4, PDPT, PD, PT) on x86_64 architecture.
//! - Recursive mapping: PML4 entry 511 recursively points to the PML4 table itself.
//!   This maps all levels of active page tables directly into virtual memory:
//!   - PML4 table at `0xFFFF_FFFF_FFFF_F000`
//!   - PDP tables base at `0xFFFF_FFFF_FFE0_0000`
//!   - PD tables base at `0xFFFF_FFFF_C000_0000`
//!   - PT tables base at `0xFFFF_FF80_0000_0000`
//! - Identity maps the first 4 MiB of physical memory for kernel bootloader transition.
//! - Higher-half mapping mirrors kernel text/data starting at `0xFFFF_8000_0000_0000`.
//! - Page faults trigger demand-allocation of physical pages via PMM.
//! - User address spaces use separate PML4 roots cloned from the kernel PML4 root,
//!   preserving kernel space mappings.
//! - Backed by a global spinlock for synchronized multi-core access.
//!
//! Notes:
//! - User Code region is mapped read-only and executable.
//! - User Stack region grows downward, protected by the No-Execute (NX) bit and
//!   an unmapped guard page directly below the stack.
//! - User Heap region starts at `USER_HEAP_BASE` and is dynamically mapped.

use crate::sync::spinlock::SpinLock;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::screen::Screen;
use crate::logging;

pub mod page_table;
pub mod page_fault;
pub mod mapping;
pub mod diagnostics;

// Re-export constants.
pub use super::vmm_constants::*;

// Re-export main structures/functions.
#[allow(unused_imports)]
pub use page_table::{
    PAGE_MASK, PML4_TABLE_ADDR, read_cr3, write_cr3, invlpg,
};
#[allow(unused_imports)]
pub use page_fault::{
    PageFaultError, try_handle_page_fault, handle_page_fault,
};
#[allow(unused_imports)]
pub use mapping::{
    MapError, populate_page_table_path, try_map_virtual_to_physical, map_virtual_to_physical,
    unmap_virtual_address, clone_kernel_pml4_for_user, destroy_user_address_space,
    destroy_user_address_space_with_options, destroy_user_address_space_with_page_counts,
    unmap_user_heap_region, map_user_page, with_address_space, switch_page_directory,
};
#[allow(unused_imports)]
pub use diagnostics::{
    debug_table_pfns_for_va, debug_mapped_pfn_for_va, debug_mapping_flags_for_va,
    debug_no_execute_flag_for_va, test_vmm,
};

/// Temporary kernel virtual address used as a one-page scratch mapping when
/// cloning page-table roots.
pub const TEMP_CLONE_PML4_VA: u64 = 0xFFFF_8000_0DEA_D000;

/// Classified user virtual address region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UserRegion {
    /// Executable/text region for user program image.
    Code,

    /// User stack region (mapped pages).
    Stack,

    /// Guard page below stack (must stay unmapped).
    Guard,

    /// Writable heap region for user programs.
    Heap,
}

struct VmmState {
    kernel_pml4_physical: u64,
    serial_debug_enabled: bool,
}

/// Global VMM with thread-safe access via SpinLock.
///
/// The lock disables interrupts during page table operations to prevent race
/// conditions when multiple tasks attempt to map/unmap pages concurrently.
struct GlobalVmm {
    inner: SpinLock<VmmState>,
    initialized: AtomicBool,
}

impl GlobalVmm {
    /// Creates the zero-initialized global VMM container.
    const fn new() -> Self {
        Self {
            inner: SpinLock::new(VmmState {
                kernel_pml4_physical: 0,
                serial_debug_enabled: false,
            }),
            initialized: AtomicBool::new(false),
        }
    }
}

static VMM: GlobalVmm = GlobalVmm::new();

/// Executes a closure with mutable access to global VMM state.
///
/// Thread-safe: acquires a spinlock that disables interrupts during page table
/// operations to prevent race conditions.
#[inline]
fn with_vmm<R>(f: impl FnOnce(&mut VmmState) -> R) -> R {
    // Catch accidental use before `init()` in debug/test configurations.
    debug_assert!(
        VMM.initialized.load(Ordering::Acquire),
        "VMM not initialized"
    );
    // Serialize access to shared VMM state.
    let mut guard = VMM.inner.lock();
    f(&mut guard)
}

/// Returns whether VMM serial logging is enabled.
pub fn serial_debug_enabled() -> bool {
    with_vmm(|state| state.serial_debug_enabled)
}

/// Returns whether VMM debug logging is enabled.
pub fn debug_enabled() -> bool {
    serial_debug_enabled()
}

/// Enables or disables VMM debug output and returns the previous setting.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_debug_output(enabled: bool) -> bool {
    with_vmm(|state| {
        // Return previous state so callers can restore debug level later.
        let old = state.serial_debug_enabled;
        state.serial_debug_enabled = enabled;
        old
    })
}

/// Enables console debug mirroring capture.
///
/// When enabled, VMM debug lines are captured and can be dumped to screen.
pub fn set_console_debug_output(enabled: bool) {
    logging::set_capture_enabled(enabled);
}

/// Writes captured VMM debug output to the screen.
pub fn print_console_debug_output(screen: &mut Screen) {
    // Filter captured logs to high-signal page-fault traces for REPL output.
    logging::print_captured_target(screen, "vmm", |line| {
        line.starts_with("VMM: page fault raw=") || line.starts_with("VMM: indices pml4=")
    });
}

#[inline]
pub fn vmm_logln(args: core::fmt::Arguments<'_>) {
    logging::logln_with_options("vmm", args, serial_debug_enabled(), true);
}

/// Emits a structured allocation trace line when debug logging is enabled.
pub fn debug_alloc(level: &str, idx: usize, pfn: u64) {
    // Emit allocation traces only when debugging is enabled.
    if debug_enabled() {
        vmm_logln(format_args!(
            "VMM: allocated PFN 0x{:x} for {} entry 0x{:x}",
            pfn, level, idx
        ));
    }
}

/// Sets the initial VMM state before the initialized flag is published.
fn set_vmm_state_unchecked(pml4_physical: u64, debug_enabled: bool) {
    // Initialization-only write path before `initialized=true` is published.
    let mut state = VMM.inner.lock();
    state.kernel_pml4_physical = pml4_physical;
    state.serial_debug_enabled = debug_enabled;
}

/// Returns the configured user region for the given page-aligned address.
#[inline]
pub fn classify_user_region(virtual_address: u64) -> Option<UserRegion> {
    // Code window has priority when the address is inside executable range.
    if (USER_CODE_BASE..USER_CODE_END).contains(&virtual_address) {
        return Some(UserRegion::Code);
    }

    // Stack window represents the regular writable user stack.
    if (USER_STACK_BASE..USER_STACK_TOP).contains(&virtual_address) {
        return Some(UserRegion::Stack);
    }

    // Guard window must stay unmapped to detect stack overflows.
    if (USER_STACK_GUARD_BASE..USER_STACK_GUARD_END).contains(&virtual_address) {
        return Some(UserRegion::Guard);
    }

    // Heap window represents the user-mode heap memory range.
    if (USER_HEAP_BASE..USER_HEAP_END).contains(&virtual_address) {
        return Some(UserRegion::Heap);
    }

    // Any other VA is outside supported user ranges.
    None
}

/// Writes one byte to a mapped virtual address with volatile semantics.
#[inline]
pub fn write_virt_u8(addr: u64, value: u8) {
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - Caller guarantees `addr` is mapped and writable.
    // - Volatile write is used for deterministic test/probe behavior.
    unsafe {
        core::ptr::write_volatile(addr as *mut u8, value);
    }
}

/// Reads one byte from a mapped virtual address with volatile semantics.
#[inline]
pub fn read_virt_u8(addr: u64) -> u8 {
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - Caller guarantees `addr` is mapped and readable.
    // - Volatile read is used for deterministic test/probe behavior.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

/// Returns the kernel address-space root (kernel PML4 physical address).
///
/// This value is initialized once during `init()` and remains the canonical
/// kernel CR3 root even when the CPU temporarily runs in a user address space.
#[cfg_attr(not(test), allow(dead_code))]
pub fn get_pml4_address() -> u64 {
    with_vmm(|state| state.kernel_pml4_physical)
}

/// Returns the currently active CPU CR3 value.
///
/// Unlike `get_pml4_address()`, this reflects transient switches performed by
/// `with_address_space()` and scheduler user-task context switches.
#[cfg_attr(not(test), allow(dead_code))]
pub fn get_active_cr3() -> u64 {
    read_cr3()
}

/// Initializes the virtual memory manager and switches CR3.
///
/// The new tables map:
/// - identity mapping for 0..4MB
/// - higher-half mapping for 0xFFFF_8000_0000_0000..+4MB
/// - recursive mapping at PML4[511]
pub fn init(debug_output: bool) {
    use page_table::{
        alloc_frame_phys_or_panic, zero_phys_page, table_at, entry_ptr, phys_to_pfn,
        PT_ENTRIES,
    };

    // Step 1: allocate all paging-structure frames required for bootstrap layout.
    let pml4 =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating bootstrap PML4");
    let pdp_higher = alloc_frame_phys_or_panic(
        "VMM: out of physical memory while allocating bootstrap higher-half PDP",
    );
    let pd_higher =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating bootstrap PD");
    let pt_higher_0 =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating bootstrap PT0");
    let pt_higher_1 =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating bootstrap PT1");
    let pdp_identity = alloc_frame_phys_or_panic(
        "VMM: out of physical memory while allocating bootstrap identity PDP",
    );
    let pd_identity =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating identity PD");
    let pt_identity_0 =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating identity PT0");
    let pt_identity_1 =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating identity PT1");

    // Step 2: clear all fresh table pages before inserting entries.
    for addr in [
        pml4,
        pdp_higher,
        pd_higher,
        pt_higher_0,
        pt_higher_1,
        pdp_identity,
        pd_identity,
        pt_identity_0,
        pt_identity_1,
    ] {
        zero_phys_page(addr);
    }

    // Step 3: wire top-level roots:
    // - slot 0   -> identity map subtree
    // - slot 256 -> higher-half kernel subtree
    // - slot 511 -> recursive self-map
    let pml4_tbl = table_at(pml4);

    // SAFETY: `pml4_tbl` is a valid PML4 page, indices < PT_ENTRIES, boot context (single-threaded).
    unsafe {
        (*entry_ptr(pml4_tbl, 0)).set_mapping(phys_to_pfn(pdp_identity), true, true, false);
        (*entry_ptr(pml4_tbl, 256)).set_mapping(phys_to_pfn(pdp_higher), true, true, false);
        (*entry_ptr(pml4_tbl, 511)).set_mapping(phys_to_pfn(pml4), true, true, false);
    }

    // Build identity mapping subtree for first 4 MiB.
    let pdp_identity_tbl = table_at(pdp_identity);

    // SAFETY: `pdp_identity_tbl` is a valid PDP page, `0 < PT_ENTRIES`, boot context.
    unsafe {
        (*entry_ptr(pdp_identity_tbl, 0)).set_mapping(phys_to_pfn(pd_identity), true, true, false);
    }

    let pd_identity_tbl = table_at(pd_identity);

    // SAFETY: `pd_identity_tbl` is a valid PD page, indices < PT_ENTRIES, boot context.
    unsafe {
        (*entry_ptr(pd_identity_tbl, 0)).set_mapping(phys_to_pfn(pt_identity_0), true, true, false);
        (*entry_ptr(pd_identity_tbl, 1)).set_mapping(phys_to_pfn(pt_identity_1), true, true, false);
    }

    let pt_identity_tbl_0 = table_at(pt_identity_0);
    for i in 0..PT_ENTRIES {
        // SAFETY: `pt_identity_tbl_0` is a valid PT page, `i < PT_ENTRIES`, boot context.
        unsafe { (*entry_ptr(pt_identity_tbl_0, i)).set_mapping(i as u64, true, true, false) };
    }

    let pt_identity_tbl_1 = table_at(pt_identity_1);
    for i in 0..PT_ENTRIES {
        // SAFETY: `pt_identity_tbl_1` is a valid PT page, `i < PT_ENTRIES`, boot context.
        unsafe {
            (*entry_ptr(pt_identity_tbl_1, i)).set_mapping(
                (PT_ENTRIES + i) as u64,
                true,
                true,
                false,
            )
        };
    }

    // Build higher-half mapping subtree that mirrors same physical 0..4 MiB.
    let pdp_higher_tbl = table_at(pdp_higher);

    // SAFETY: `pdp_higher_tbl` is a valid PDP page, `0 < PT_ENTRIES`, boot context.
    unsafe {
        (*entry_ptr(pdp_higher_tbl, 0)).set_mapping(phys_to_pfn(pd_higher), true, true, false)
    };

    let pd_higher_tbl = table_at(pd_higher);

    // SAFETY: `pd_higher_tbl` is a valid PD page, indices < PT_ENTRIES, boot context.
    unsafe {
        (*entry_ptr(pd_higher_tbl, 0)).set_mapping(phys_to_pfn(pt_higher_0), true, true, false);
        (*entry_ptr(pd_higher_tbl, 1)).set_mapping(phys_to_pfn(pt_higher_1), true, true, false);
    }

    let pt_higher_tbl_0 = table_at(pt_higher_0);
    for i in 0..PT_ENTRIES {
        // SAFETY: `pt_higher_tbl_0` is a valid PT page, `i < PT_ENTRIES`, boot context.
        unsafe {
            let e = entry_ptr(pt_higher_tbl_0, i);
            (*e).set_mapping(i as u64, true, true, false);
            (*e).set_global(true);
        }
    }

    let pt_higher_tbl_1 = table_at(pt_higher_1);
    for i in 0..PT_ENTRIES {
        // SAFETY: `pt_higher_tbl_1` is a valid PT page, `i < PT_ENTRIES`, boot context.
        unsafe {
            let e = entry_ptr(pt_higher_tbl_1, i);
            (*e).set_mapping((PT_ENTRIES + i) as u64, true, true, false);
            (*e).set_global(true);
        }
    }

    // Mark higher-half page-directory and page-table entries as global
    // (but NOT the recursive PML4 entry, which must change per address space).
    // SAFETY: all tables are valid PT pages, indices < PT_ENTRIES, boot context.
    unsafe {
        (*entry_ptr(pd_higher_tbl, 0)).set_global(true);
        (*entry_ptr(pd_higher_tbl, 1)).set_global(true);
        (*entry_ptr(pdp_higher_tbl, 0)).set_global(true);
        (*entry_ptr(pml4_tbl, 256)).set_global(true);
    }

    // Step 4: publish VMM state and mark it initialized for runtime APIs.
    set_vmm_state_unchecked(pml4, debug_output);
    VMM.initialized.store(true, Ordering::Release);

    // Step 5: activate the new bootstrap root.
    write_cr3(pml4);

    // Enable global pages (CR4.PGE) to avoid flushing kernel TLB entries on CR3 switch.
    // Global pages marked with the G-bit persist in the TLB across address space switches.
    // SAFETY: Enabling CR4.PGE is a standard kernel optimization and safe after
    // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
    // global-bit configuration is complete.
    unsafe {
        page_table::enable_global_pages();
    }
}
