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
pub mod vmm_constants;

// Re-export constants.
pub use vmm_constants::*;

// Re-export main structures/functions.
#[allow(unused_imports)]
pub use page_table::{
    PAGE_MASK, PML4_TABLE_ADDR, read_cr3, write_cr3, invlpg, reserve_firmware_page_tables,
    is_va_mapped,
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

/// Initializes the virtual memory manager and switches CR3 to a kernel-owned PML4.
///
/// The kernel address space is built as a SUPERSET of the firmware's: every firmware
/// PML4 entry is copied into a freshly allocated PML4 frame — preserving the full
/// identity map, the loader's higher-half mirror at slot 256, and all firmware
/// SMM/ACPI/MMIO/runtime mappings — then slot 511 is replaced with a recursive
/// self-map so the VMM can edit page tables through the recursive window.
///
/// This replaced an earlier hand-built minimal map (identity of only the low 4 MiB plus
/// a higher-half mirror), which reset real AMD hardware the instant CR3 loaded it:
/// discarding the firmware mappings breaks the platform's asynchronous SMM path. Cloning
/// the firmware PML4 keeps everything the platform needs.
pub fn init(debug_output: bool) {
    use page_table::{
        alloc_frame_phys_or_panic, build_kernel_pml4_from_firmware, table_at, zero_phys_page,
    };

    // Allocate and zero a fresh frame for the kernel's own PML4.
    let pml4 =
        alloc_frame_phys_or_panic("VMM: out of physical memory while allocating kernel PML4");
    zero_phys_page(pml4);

    // Build it as a superset of the firmware PML4 (still active in CR3): copy all 512
    // firmware entries, then install our recursive self-map at slot 511. Both PML4s are
    // reachable here via the firmware identity map (physical address == virtual address).
    let fw_pml4 = read_cr3() & 0x000F_FFFF_FFFF_F000;
    // SAFETY: firmware PML4 and our fresh PML4 are reachable via the active firmware
    // identity map; `pml4` is the physical frame backing the destination table.
    unsafe {
        build_kernel_pml4_from_firmware(table_at(fw_pml4), table_at(pml4), pml4);
    }

    // Publish VMM state and mark it initialized for runtime APIs.
    set_vmm_state_unchecked(pml4, debug_output);
    VMM.initialized.store(true, Ordering::Release);

    // Activate the new root and return to the boot sequence.
    write_cr3(pml4);
}
