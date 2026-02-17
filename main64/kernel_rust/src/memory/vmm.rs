//! Virtual memory manager for x86_64 4-level paging with recursive mapping.
//!
//! User virtual-address layout (current policy):
//!
//! ```text
//! Higher addresses
//!     ^
//!     |
//!     |  USER_STACK_TOP = 0x0000_7FFF_F000_0000   (exclusive upper bound)
//!     |  +--------------------------------------+
//!     |  |           User Stack Region          |
//!     |  |                                      |
//!     |  |  [USER_STACK_BASE .. USER_STACK_TOP) |
//!     |  |  size = USER_STACK_SIZE = 1 MiB      |
//!     |  +--------------------------------------+
//!     |  USER_STACK_BASE = 0x0000_7FFF_EFF0_0000
//!     |  +--------------------------------------+
//!     |  |         Guard Page (unmapped)        |
//!     |  | [USER_STACK_GUARD_BASE .. _END)      |
//!     |  | size = 4 KiB                         |
//!     |  +--------------------------------------+
//!     |  USER_STACK_GUARD_BASE = 0x0000_7FFF_EFEF_F000
//!     |
//!     |                 (large unmapped gap)
//!     |
//!     |  USER_CODE_BASE = 0x0000_7000_0000_0000
//!     |  +--------------------------------------+
//!     |  |            User Code Region          |
//!     |  | [USER_CODE_BASE .. USER_CODE_END)    |
//!     |  | size = USER_CODE_SIZE = 2 MiB        |
//!     |  +--------------------------------------+
//!     |  USER_CODE_END  = 0x0000_7000_0020_0000
//!     |
//!     +--------------------------------------------------> virtual address space
//! Lower addresses
//! ```
//!
//! Region classification:
//! - `[USER_CODE_BASE, USER_CODE_END)` => code (mappable)
//! - `[USER_STACK_BASE, USER_STACK_TOP)` => stack (mappable)
//! - `[USER_STACK_GUARD_BASE, USER_STACK_BASE)` => guard (must stay unmapped)
//! - everything else => not a user region

use crate::sync::spinlock::SpinLock;
use core::arch::asm;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::arch::interrupts;
use crate::drivers::screen::Screen;
use crate::logging;
use crate::memory::pmm;

const PT_ENTRIES: usize = 512;
const SMALL_PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = !(SMALL_PAGE_SIZE - 1);

/// Temporary kernel virtual address used as a one-page scratch mapping when
/// cloning page-table roots.
///
/// Why this is needed:
/// - The PMM returns a physical frame (`new_pml4_phys`) for the clone target.
/// - To copy bytes into that frame, the kernel needs a virtual mapping to it.
/// - `TEMP_CLONE_PML4_VA` provides exactly one reusable VA slot for that purpose.
///
/// Lifecycle in `clone_kernel_pml4_for_user()`:
/// 1. map `TEMP_CLONE_PML4_VA -> new_pml4_phys`
/// 2. copy current PML4 page bytes into that VA
/// 3. patch recursive entry 511 in the clone
/// 4. unmap `TEMP_CLONE_PML4_VA` again
///
/// The mapping is temporary and not released via PMM on unmap because the frame
/// remains owned by the caller as the returned user PML4 root.
const TEMP_CLONE_PML4_VA: u64 = 0xFFFF_8000_0DEA_D000;

const PML4_TABLE_ADDR: u64 = 0xFFFF_FFFF_FFFF_F000;
const PDP_TABLE_BASE: u64 = 0xFFFF_FFFF_FFE0_0000;
const PD_TABLE_BASE: u64 = 0xFFFF_FFFF_C000_0000;
const PT_TABLE_BASE: u64 = 0xFFFF_FF80_0000_0000;

const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_HUGE: u64 = 1 << 7;
const ENTRY_GLOBAL: u64 = 1 << 8;
const ENTRY_FRAME_MASK: u64 = 0x0000_FFFF_FFFF_F000;

const PF_ERR_PRESENT: u64 = 1 << 0;

/// User executable base virtual address.
pub const USER_CODE_BASE: u64 = 0x0000_7000_0000_0000;

/// User executable mapping size (2 MiB).
pub const USER_CODE_SIZE: u64 = 0x0020_0000;

/// User executable end address (exclusive).
pub const USER_CODE_END: u64 = USER_CODE_BASE + USER_CODE_SIZE;

/// User stack top (exclusive upper boundary).
pub const USER_STACK_TOP: u64 = 0x0000_7FFF_F000_0000;

/// User stack size (1 MiB).
pub const USER_STACK_SIZE: u64 = 0x0010_0000;

/// User stack start (inclusive).
pub const USER_STACK_BASE: u64 = USER_STACK_TOP - USER_STACK_SIZE;

/// Optional guard page below the user stack.
pub const USER_STACK_GUARD_BASE: u64 = USER_STACK_BASE - SMALL_PAGE_SIZE;

/// Optional guard page end (exclusive).
pub const USER_STACK_GUARD_END: u64 = USER_STACK_BASE;

/// Classified user virtual address region.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]

pub enum UserRegion {
    /// Executable/text region for user program image.
    Code,

    /// User stack region (mapped pages).
    Stack,

    /// Guard page below stack (must stay unmapped).
    Guard,
}

/// Error returned by the checked page-fault handling path.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFaultError {
    /// Fault had `P=1` in the CPU error code, i.e. a protection violation.
    /// These faults must not trigger demand allocation.
    ProtectionFault {
        virtual_address: u64,
        error_code: u64,
    },
}

/// Error returned by checked mapping operations.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    /// Virtual address is already mapped to a different physical frame.
    AlreadyMapped {
        virtual_address: u64,
        current_pfn: u64,
        requested_pfn: u64,
    },

    /// Address is outside configured user mapping regions.
    NotUserRegion { virtual_address: u64 },

    /// Address targets the configured guard page.
    UserGuardPage { virtual_address: u64 },
}

#[derive(Clone, Copy)]
#[repr(transparent)]
struct PageTableEntry(u64);

impl PageTableEntry {
    /// Returns whether this entry is marked present.
    #[inline]
    fn present(self) -> bool {
        (self.0 & ENTRY_PRESENT) != 0
    }

    /// Sets or clears the present bit.
    #[inline]
    fn set_present(&mut self, val: bool) {
        // Toggle only the present bit and keep all other fields intact.
        if val {
            self.0 |= ENTRY_PRESENT;
        } else {
            self.0 &= !ENTRY_PRESENT;
        }
    }

    /// Returns whether this entry is writable.
    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    fn writable(self) -> bool {
        (self.0 & ENTRY_WRITABLE) != 0
    }

    /// Sets or clears the writable bit.
    #[inline]
    fn set_writable(&mut self, val: bool) {
        // Toggle only the writable bit and keep all other fields intact.
        if val {
            self.0 |= ENTRY_WRITABLE;
        } else {
            self.0 &= !ENTRY_WRITABLE;
        }
    }

    /// Returns whether this entry is user-accessible.
    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    fn user(self) -> bool {
        (self.0 & ENTRY_USER) != 0
    }

    /// Sets or clears the user-accessible bit.
    #[inline]
    fn set_user(&mut self, val: bool) {
        // Toggle only the user-accessible bit and keep all other fields intact.
        if val {
            self.0 |= ENTRY_USER;
        } else {
            self.0 &= !ENTRY_USER;
        }
    }

    /// Returns whether the global bit is set.
    ///
    /// Global pages are not flushed from the TLB on CR3 writes when CR4.PGE is enabled.
    /// This is useful for kernel mappings that are shared across all address spaces.
    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    fn global(self) -> bool {
        (self.0 & ENTRY_GLOBAL) != 0
    }

    /// Sets or clears the global bit.
    ///
    /// Global pages persist in the TLB across CR3 switches (when CR4.PGE=1).
    /// Typically used for kernel code/data mappings to avoid TLB flush overhead.
    ///
    /// # Important
    /// The recursive PML4 entry (entry 511) should **not** be marked global,
    /// as it must change when switching to a different address space.
    #[inline]
    fn set_global(&mut self, val: bool) {
        // Toggle only the global bit and keep all other fields intact.
        if val {
            self.0 |= ENTRY_GLOBAL;
        } else {
            self.0 &= !ENTRY_GLOBAL;
        }
    }

    /// Returns the mapped page-frame number.
    #[inline]
    fn frame(self) -> u64 {
        (self.0 & ENTRY_FRAME_MASK) >> 12
    }

    /// Writes the page-frame number into the entry frame field.
    #[inline]
    fn set_frame(&mut self, pfn: u64) {
        self.0 = (self.0 & !ENTRY_FRAME_MASK) | ((pfn << 12) & ENTRY_FRAME_MASK);
    }

    /// Sets frame plus basic permission bits in one call.
    #[inline]
    fn set_mapping(&mut self, pfn: u64, present: bool, writable: bool, user: bool) {
        self.set_frame(pfn);
        self.set_present(present);
        self.set_writable(writable);
        self.set_user(user);
    }

    /// Clears the entry to an unmapped state.
    #[inline]
    fn clear(&mut self) {
        self.0 = 0;
    }

    /// Returns whether the huge-page bit is set.
    #[inline]
    fn huge(self) -> bool {
        (self.0 & ENTRY_HUGE) != 0
    }
}

#[repr(C, align(4096))]
struct PageTable {
    entries: [PageTableEntry; PT_ENTRIES],
}

impl PageTable {
    /// Clears all page-table entries.
    #[inline]
    fn zero(&mut self) {
        // Reset every entry to "not present".
        for entry in self.entries.iter_mut() {
            entry.clear();
        }
    }
}

/// Returns the PML4 index for a canonical virtual address.
#[inline]
fn pml4_index(va: u64) -> usize {
    ((va >> 39) & 0x1FF) as usize
}

/// Returns the PDP index for a canonical virtual address.
#[inline]
fn pdp_index(va: u64) -> usize {
    ((va >> 30) & 0x1FF) as usize
}

/// Returns the PD index for a canonical virtual address.
#[inline]
fn pd_index(va: u64) -> usize {
    ((va >> 21) & 0x1FF) as usize
}

/// Returns the PT index for a canonical virtual address.
#[inline]
fn pt_index(va: u64) -> usize {
    ((va >> 12) & 0x1FF) as usize
}

/// Returns the recursive-mapping virtual address of the PDP table for `va`.
#[inline]
fn pdp_table_addr(va: u64) -> u64 {
    PDP_TABLE_BASE + ((va >> 27) & 0x0000_001F_F000)
}

/// Returns the recursive-mapping virtual address of the PD table for `va`.
#[inline]
fn pd_table_addr(va: u64) -> u64 {
    PD_TABLE_BASE + ((va >> 18) & 0x0000_3FFF_F000)
}

/// Returns the recursive-mapping virtual address of the PT table for `va`.
#[inline]
fn pt_table_addr(va: u64) -> u64 {
    PT_TABLE_BASE + ((va >> 9) & 0x0000_007F_FFFF_F000)
}

/// Aligns an address down to a 4 KiB page boundary.
#[inline]
fn page_align_down(addr: u64) -> u64 {
    addr & PAGE_MASK
}

/// Returns the configured user region for the given page-aligned address.
#[inline]
fn classify_user_region(virtual_address: u64) -> Option<UserRegion> {
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
    // Any other VA is outside supported user ranges.
    None
}

/// Converts a physical byte address to a page-frame number.
#[inline]
fn phys_to_pfn(addr: u64) -> u64 {
    addr / SMALL_PAGE_SIZE
}

/// Reads the current CR3 value.
///
/// Caller contract: must run in ring 0 on x86_64.
fn read_cr3() -> u64 {
    let val: u64;
    // SAFETY:
    // - Reading CR3 is privileged and valid in ring 0.
    // - Does not dereference memory.
    unsafe {
        asm!("mov {}, cr3", out(reg) val, options(nomem, nostack, preserves_flags));
    }
    val
}

/// Writes a new CR3 value.
///
/// Caller contract: `val` must point to a valid PML4 frame.
fn write_cr3(val: u64) {
    // SAFETY:
    // - Caller guarantees `val` points to a valid PML4 root frame.
    // - Executed only in ring 0.
    unsafe {
        asm!("mov cr3, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

/// Invalidates one TLB entry for the given virtual address.
///
/// Caller contract: must run in ring 0 on x86_64.
fn invlpg(addr: u64) {
    // SAFETY:
    // - `invlpg` is privileged and valid in ring 0.
    // - Operand is treated as an address tag for TLB invalidation.
    unsafe {
        asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
    }
}

/// Enables global pages by setting CR4.PGE (Page Global Enable).
///
/// When CR4.PGE is set, page-table entries with the G-bit set are not
/// flushed from the TLB on CR3 writes. This is essential for kernel
/// performance as it avoids flushing kernel mappings on every context switch.
///
/// # Safety
/// Must be called only after global-bit configuration is complete.
/// Caller must be running in ring 0 on x86_64.
unsafe fn enable_global_pages() {
    const CR4_PGE: u64 = 1 << 7; // Page Global Enable bit

    let mut cr4: u64;
    unsafe {
        // Read current CR4 value
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        // Set PGE bit
        cr4 |= CR4_PGE;
        // Write back to CR4
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
}

struct VmmState {
    pml4_physical: u64,
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
                pml4_physical: 0,
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

/// Allocates one physical frame and returns its physical address.
#[inline]
fn alloc_frame_phys() -> u64 {
    pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("VMM: out of physical memory while allocating page frame")
            .physical_address()
    })
}

/// Interprets a recursive-mapped virtual address as a mutable page table.
///
/// Caller contract: `addr` must point to a valid, mapped page-table page.
#[inline]
fn table_at(addr: u64) -> &'static mut PageTable {
    // SAFETY:
    // - Caller guarantees `addr` points to a mapped page table page.
    // - Returned reference is used under page-table ownership conventions.
    unsafe { &mut *(addr as *mut PageTable) }
}

/// Zeros one 4 KiB page in physical memory.
///
/// Caller contract: `addr` must be writable and page-aligned physical memory.
#[inline]
fn zero_phys_page(addr: u64) {
    // SAFETY:
    // - Caller guarantees `addr` is writable and page-aligned.
    // - Writes exactly one 4 KiB page.
    unsafe {
        core::ptr::write_bytes(addr as *mut u8, 0, SMALL_PAGE_SIZE as usize);
    }
}

/// Zeros one already-mapped 4 KiB virtual page.
#[inline]
fn zero_virt_page(addr: u64) {
    // SAFETY:
    // - Caller guarantees `addr` points to a currently mapped writable page.
    // - Writes exactly one 4 KiB page.
    unsafe {
        core::ptr::write_bytes(addr as *mut u8, 0, SMALL_PAGE_SIZE as usize);
    }
}

#[inline]
fn vmm_logln(args: core::fmt::Arguments<'_>) {
    logging::logln_with_options("vmm", args, serial_debug_enabled(), true);
}

/// Returns whether VMM serial logging is enabled.
fn serial_debug_enabled() -> bool {
    with_vmm(|state| state.serial_debug_enabled)
}

/// Writes one byte to a mapped virtual address with volatile semantics.
#[inline]
fn write_virt_u8(addr: u64, value: u8) {
    // SAFETY:
    // - Caller guarantees `addr` is mapped and writable.
    // - Volatile write is used for deterministic test/probe behavior.
    unsafe {
        core::ptr::write_volatile(addr as *mut u8, value);
    }
}

/// Reads one byte from a mapped virtual address with volatile semantics.
#[inline]
fn read_virt_u8(addr: u64) -> u8 {
    // SAFETY:
    // - Caller guarantees `addr` is mapped and readable.
    // - Volatile read is used for deterministic test/probe behavior.
    unsafe { core::ptr::read_volatile(addr as *const u8) }
}

/// Sets the initial VMM state before the initialized flag is published.
fn set_vmm_state_unchecked(pml4_physical: u64, debug_enabled: bool) {
    // Initialization-only write path before `initialized=true` is published.
    let mut state = VMM.inner.lock();
    state.pml4_physical = pml4_physical;
    state.serial_debug_enabled = debug_enabled;
}

/// Returns whether VMM debug logging is enabled.
fn debug_enabled() -> bool {
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

/// Emits a structured allocation trace line when debug logging is enabled.
fn debug_alloc(level: &str, idx: usize, pfn: u64) {
    // Emit allocation traces only when debugging is enabled.
    if debug_enabled() {
        vmm_logln(format_args!(
            "VMM: allocated PFN 0x{:x} for {} entry 0x{:x}",
            pfn, level, idx
        ));
    }
}

/// Initializes the virtual memory manager and switches CR3.
///
/// The new tables map:
/// - identity mapping for 0..4MB
/// - higher-half mapping for 0xFFFF_8000_0000_0000..+4MB
/// - recursive mapping at PML4[511]
pub fn init(debug_output: bool) {
    // Step 1: allocate all paging-structure frames required for bootstrap layout.
    let pml4 = alloc_frame_phys();
    let pdp_higher = alloc_frame_phys();
    let pd_higher = alloc_frame_phys();
    let pt_higher_0 = alloc_frame_phys();
    let pt_higher_1 = alloc_frame_phys();
    let pdp_identity = alloc_frame_phys();
    let pd_identity = alloc_frame_phys();
    let pt_identity_0 = alloc_frame_phys();
    let pt_identity_1 = alloc_frame_phys();

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
    pml4_tbl.entries[0].set_mapping(phys_to_pfn(pdp_identity), true, true, false);
    pml4_tbl.entries[256].set_mapping(phys_to_pfn(pdp_higher), true, true, false);
    pml4_tbl.entries[511].set_mapping(phys_to_pfn(pml4), true, true, false);

    // Build identity mapping subtree for first 4 MiB.
    let pdp_identity_tbl = table_at(pdp_identity);
    pdp_identity_tbl.entries[0].set_mapping(phys_to_pfn(pd_identity), true, true, false);

    let pd_identity_tbl = table_at(pd_identity);
    pd_identity_tbl.entries[0].set_mapping(phys_to_pfn(pt_identity_0), true, true, false);
    pd_identity_tbl.entries[1].set_mapping(phys_to_pfn(pt_identity_1), true, true, false);

    let pt_identity_tbl_0 = table_at(pt_identity_0);

    // Identity-map physical 0..2 MiB.
    for i in 0..PT_ENTRIES {
        pt_identity_tbl_0.entries[i].set_mapping(i as u64, true, true, false);
    }

    let pt_identity_tbl_1 = table_at(pt_identity_1);

    // Identity-map physical 2..4 MiB.
    for i in 0..PT_ENTRIES {
        pt_identity_tbl_1.entries[i].set_mapping((PT_ENTRIES + i) as u64, true, true, false);
    }

    // Build higher-half mapping subtree that mirrors same physical 0..4 MiB.
    let pdp_higher_tbl = table_at(pdp_higher);
    pdp_higher_tbl.entries[0].set_mapping(phys_to_pfn(pd_higher), true, true, false);

    let pd_higher_tbl = table_at(pd_higher);
    pd_higher_tbl.entries[0].set_mapping(phys_to_pfn(pt_higher_0), true, true, false);
    pd_higher_tbl.entries[1].set_mapping(phys_to_pfn(pt_higher_1), true, true, false);

    let pt_higher_tbl_0 = table_at(pt_higher_0);

    // Map first 2 MiB into higher-half window and mark as global.
    for i in 0..PT_ENTRIES {
        pt_higher_tbl_0.entries[i].set_mapping(i as u64, true, true, false);

        // Mark kernel pages as global to avoid TLB flush on CR3 switch
        pt_higher_tbl_0.entries[i].set_global(true);
    }

    let pt_higher_tbl_1 = table_at(pt_higher_1);

    // Map second 2 MiB into higher-half window and mark as global.
    for i in 0..PT_ENTRIES {
        pt_higher_tbl_1.entries[i].set_mapping((PT_ENTRIES + i) as u64, true, true, false);

        // Mark kernel pages as global to avoid TLB flush on CR3 switch
        pt_higher_tbl_1.entries[i].set_global(true);
    }

    // Mark higher-half page-directory and page-table entries as global
    // (but NOT the recursive PML4 entry, which must change per address space)
    pd_higher_tbl.entries[0].set_global(true);
    pd_higher_tbl.entries[1].set_global(true);
    pdp_higher_tbl.entries[0].set_global(true);
    pml4_tbl.entries[256].set_global(true);

    // Step 4: publish VMM state and mark it initialized for runtime APIs.
    set_vmm_state_unchecked(pml4, debug_output);
    VMM.initialized.store(true, Ordering::Release);

    // Step 5: activate the new bootstrap root.
    write_cr3(pml4);

    // Enable global pages (CR4.PGE) to avoid flushing kernel TLB entries on CR3 switch.
    // Global pages marked with the G-bit persist in the TLB across address space switches.
    // SAFETY: Enabling CR4.PGE is a standard kernel optimization and safe after
    // global-bit configuration is complete.
    unsafe {
        enable_global_pages();
    }
}

/// Returns the currently active kernel PML4 physical address.
#[cfg_attr(not(test), allow(dead_code))]
pub fn get_pml4_address() -> u64 {
    with_vmm(|state| state.pml4_physical)
}

/// Executes `f` while `pml4_phys` is active in CR3, then restores previous state.
///
/// Interrupts are disabled for the whole critical section so timer preemption
/// cannot observe a temporary address-space switch.
#[cfg_attr(not(test), allow(dead_code))]
pub fn with_address_space<R>(pml4_phys: u64, f: impl FnOnce() -> R) -> R {
    // Preserve interrupt state and block preemption during temporary CR3 switch.
    let interrupts_were_enabled = interrupts::are_enabled();
    if interrupts_were_enabled {
        interrupts::disable();
    }

    // Capture current root so we can restore it unconditionally.
    let previous_cr3 = read_cr3();

    // Switch only when target differs from current root.
    if previous_cr3 != pml4_phys {
        // SAFETY:
        // - `pml4_phys` is supplied by trusted kernel code that owns the target root.
        // - Interrupts are disabled for the entire temporary switch.
        unsafe {
            switch_page_directory(pml4_phys);
        }
    }

    // Execute caller work while target address space is active.
    let result = f();

    // Restore original CR3 before leaving critical section.
    if previous_cr3 != pml4_phys {
        // SAFETY:
        // - `previous_cr3` was read from the CPU before switching and is valid.
        // - Restoring CR3 under disabled interrupts returns to the original context.
        unsafe {
            switch_page_directory(previous_cr3);
        }
    }

    // Restore interrupt enable state to exactly what caller had.
    if interrupts_were_enabled {
        interrupts::enable();
    }

    result
}

/// Switches to the provided page directory (physical PML4 address).
///
/// # Safety
/// The caller must ensure `pml4_phys` points to a valid, fully initialized
/// PML4 table in physical memory. Switching to an invalid CR3 target can
/// immediately crash the kernel due to page faults/triple fault.
pub unsafe fn switch_page_directory(pml4_phys: u64) {
    // CPU state update.
    write_cr3(pml4_phys);

    // Mirror the active root in software state for diagnostics/helpers.
    with_vmm(|state| {
        state.pml4_physical = pml4_phys;
    });
}

/// Builds any missing intermediate page tables (PML4/PDP/PD) for `virtual_address`.
///
#[inline]
fn populate_page_table_path(virtual_address: u64, user: bool) {
    // Level 1: PML4 entry.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);

    if !pml4.entries[pml4_idx].present() {
        // Allocate and zero a fresh PDP table.
        let new_table_phys = alloc_frame_phys();
        pml4.entries[pml4_idx].set_mapping(phys_to_pfn(new_table_phys), true, true, user);
        invlpg(pdp_table_addr(virtual_address));
        let new_pdp = table_at(pdp_table_addr(virtual_address));
        new_pdp.zero();
        debug_alloc("PML4", pml4_idx, pml4.entries[pml4_idx].frame());
    } else if user {
        // Existing path: elevate permissions for user mapping requests.
        pml4.entries[pml4_idx].set_user(true);
        pml4.entries[pml4_idx].set_writable(true);
    }

    // Level 2: PDP entry.
    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);

    if !pdp.entries[pdp_idx].present() {
        // Allocate and zero a fresh PD table.
        let new_table_phys = alloc_frame_phys();
        pdp.entries[pdp_idx].set_mapping(phys_to_pfn(new_table_phys), true, true, user);
        invlpg(pd_table_addr(virtual_address));
        let new_pd = table_at(pd_table_addr(virtual_address));
        new_pd.zero();
        debug_alloc("PDP", pdp_idx, pdp.entries[pdp_idx].frame());
    } else if user {
        // Existing path: elevate permissions for user mapping requests.
        pdp.entries[pdp_idx].set_user(true);
        pdp.entries[pdp_idx].set_writable(true);
    }

    // Level 3: PD entry.
    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);

    if !pd.entries[pd_idx].present() {
        // Allocate and zero a fresh PT table.
        let new_table_phys = alloc_frame_phys();
        pd.entries[pd_idx].set_mapping(phys_to_pfn(new_table_phys), true, true, user);
        invlpg(pt_table_addr(virtual_address));
        let new_pt = table_at(pt_table_addr(virtual_address));
        new_pt.zero();
        debug_alloc("PD", pd_idx, pd.entries[pd_idx].frame());
    } else if user {
        // Existing path: elevate permissions for user mapping requests.
        pd.entries[pd_idx].set_user(true);
        pd.entries[pd_idx].set_writable(true);
    }
}

/// Returns the PT containing `virtual_address` if all intermediate levels exist.
///
/// Returns `None` if any level is non-present or uses a huge page mapping.
///
#[inline]
fn pt_for_if_present(virtual_address: u64) -> Option<&'static mut PageTable> {
    // Resolve PML4 level and reject missing/huge entries.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = pml4.entries[pml4_idx];

    if !pml4e.present() || pml4e.huge() {
        return None;
    }

    // Resolve PDP level and reject missing/huge entries.
    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = pdp.entries[pdp_idx];

    if !pdpe.present() || pdpe.huge() {
        return None;
    }

    // Resolve PD level and reject missing/huge entries.
    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = pd.entries[pd_idx];

    if !pde.present() || pde.huge() {
        return None;
    }

    // All intermediate levels are present => return leaf PT table.
    Some(table_at(pt_table_addr(virtual_address)))
}

/// Returns whether a page-table page contains no present entries.
#[inline]
fn table_is_empty(table: &PageTable) -> bool {
    table.entries.iter().all(|entry| !entry.present())
}

/// Clears one mapped leaf page and prunes empty page-table levels for `virtual_address`.
///
/// This helper is used by address-space teardown paths and intentionally does
/// not log warnings when a leaf PFN is not PMM-managed.
///
/// If `release_leaf_pfn` is `true`, the leaf PFN is returned to PMM.
/// If `false`, the leaf mapping is only cleared.
fn unmap_page_and_prune_pagetable_hierarchy(virtual_address: u64, release_leaf_pfn: bool) {
    let virtual_address = page_align_down(virtual_address);

    // Step 1: Resolve the full 4-level path for `virtual_address`.
    // If any intermediate level is missing (or huge-mapped), there is no
    // normal 4KiB leaf to clear and therefore nothing to prune.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = pml4.entries[pml4_idx];

    if !pml4e.present() || pml4e.huge() {
        return;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = pdp.entries[pdp_idx];

    if !pdpe.present() || pdpe.huge() {
        return;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = pd.entries[pd_idx];

    if !pde.present() || pde.huge() {
        return;
    }

    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Step 2: Clear the leaf PTE.
    // Optionally release the old leaf PFN depending on caller policy:
    // - true  => regular owned user page, return frame to PMM
    // - false => alias/scratch mapping, only remove mapping
    if pt.entries[pt_idx].present() {
        let leaf_pfn = pt.entries[pt_idx].frame();
        pt.entries[pt_idx].clear();
        invlpg(virtual_address);
        if release_leaf_pfn {
            let _ = pmm::with_pmm(|mgr| mgr.release_pfn(leaf_pfn));
        }
    }

    // Step 3: Bottom-up pruning.
    // Only remove a parent-table entry if the child table became empty.
    // This guarantees we never drop shared siblings.

    // 3a) PT empty? remove PD -> PT edge and release PT frame.
    if !table_is_empty(pt) {
        return;
    }

    let pt_pfn = pd.entries[pd_idx].frame();
    pd.entries[pd_idx].clear();
    invlpg(pt_table_addr(virtual_address));
    let _ = pmm::with_pmm(|mgr| mgr.release_pfn(pt_pfn));

    // 3b) PD empty? remove PDP -> PD edge and release PD frame.
    if !table_is_empty(pd) {
        return;
    }

    let pd_pfn = pdp.entries[pdp_idx].frame();
    pdp.entries[pdp_idx].clear();
    invlpg(pd_table_addr(virtual_address));
    let _ = pmm::with_pmm(|mgr| mgr.release_pfn(pd_pfn));

    // 3c) PDP empty? remove PML4 -> PDP edge and release PDP frame.
    if !table_is_empty(pdp) {
        return;
    }

    let pdp_pfn = pml4.entries[pml4_idx].frame();
    pml4.entries[pml4_idx].clear();
    invlpg(pdp_table_addr(virtual_address));
    let _ = pmm::with_pmm(|mgr| mgr.release_pfn(pdp_pfn));
}

/// Handles page faults by demand-allocating page tables and target page frame.
///
/// Returns `Err(PageFaultError::ProtectionFault)` for protection faults (`P=1`),
/// and `Ok(())` for handled non-present faults.
pub fn try_handle_page_fault(virtual_address: u64, error_code: u64) -> Result<(), PageFaultError> {
    // Keep both raw and page-aligned addresses for diagnostics and mapping.
    let fault_address_raw = virtual_address;
    let virtual_address = page_align_down(fault_address_raw);

    // Optional structured debug trace for fault triage.
    if debug_enabled() {
        let cr3 = read_cr3();

        vmm_logln(format_args!(
            "VMM: page fault raw=0x{:x} aligned=0x{:x} cr3=0x{:x} err=0x{:x}",
            fault_address_raw, virtual_address, cr3, error_code
        ));

        vmm_logln(format_args!(
            "VMM: indices pml4={} pdp={} pd={} pt={}",
            pml4_index(virtual_address),
            pdp_index(virtual_address),
            pd_index(virtual_address),
            pt_index(virtual_address)
        ));

        vmm_logln(format_args!(
            "VMM: err bits p={} w={} u={} rsv={} ifetch={}",
            (error_code & (1 << 0)) != 0,
            (error_code & (1 << 1)) != 0,
            (error_code & (1 << 2)) != 0,
            (error_code & (1 << 3)) != 0,
            (error_code & (1 << 4)) != 0
        ));
    }

    // Only demand-map on non-present faults.
    // Protection faults indicate a real access violation and must not be hidden.
    if (error_code & PF_ERR_PRESENT) != 0 {
        vmm_logln(format_args!(
            "VMM: protection fault at 0x{:x} err=0x{:x} (allocation refused)",
            fault_address_raw, error_code
        ));
        return Err(PageFaultError::ProtectionFault {
            virtual_address: fault_address_raw,
            error_code,
        });
    }

    let user_region = classify_user_region(virtual_address);
    // Guard page faults are always treated as protection violations.
    if matches!(user_region, Some(UserRegion::Guard)) {
        return Err(PageFaultError::ProtectionFault {
            virtual_address: fault_address_raw,
            error_code,
        });
    }

    let user_access = matches!(
        user_region,
        Some(UserRegion::Code) | Some(UserRegion::Stack)
    );

    // User code pages default to read-only; everything else can start writable.
    let writable = !matches!(user_region, Some(UserRegion::Code));
    populate_page_table_path(virtual_address, user_access);
    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Allocate a leaf page only when page is currently non-present.
    if !pt.entries[pt_idx].present() {
        let new_page_phys = alloc_frame_phys();

        // Map writable first so zero-fill is valid even when final mapping
        // should be read-only (e.g. user code pages).
        pt.entries[pt_idx].set_mapping(phys_to_pfn(new_page_phys), true, true, user_access);
        invlpg(virtual_address);
        zero_virt_page(virtual_address);

        if !writable {
            // Tighten final permissions for code pages after zero-fill.
            pt.entries[pt_idx].set_writable(false);
            invlpg(virtual_address);
        }

        debug_alloc("PT", pt_idx, pt.entries[pt_idx].frame());
    }

    Ok(())
}

/// Handles page faults for production interrupt paths.
///
/// This wrapper preserves the existing behavior: protection faults are fatal.
pub fn handle_page_fault(virtual_address: u64, error_code: u64) {
    // Production path keeps historical behavior: protection faults are fatal.
    if let Err(PageFaultError::ProtectionFault {
        virtual_address,
        error_code,
    }) = try_handle_page_fault(virtual_address, error_code)
    {
        panic!(
            "VMM: protection page fault at 0x{:x} err=0x{:x}",
            virtual_address, error_code
        );
    }
}

/// Maps `virtual_address` to `physical_address` with present + writable flags.
///
/// Returns an error if the VA is already mapped to a different frame.
#[cfg_attr(not(test), allow(dead_code))]
pub fn try_map_virtual_to_physical(
    virtual_address: u64,
    physical_address: u64,
) -> Result<(), MapError> {
    // Normalize both addresses to page granularity.
    let virtual_address = page_align_down(virtual_address);
    let physical_address = page_align_down(physical_address);
    let requested_pfn = phys_to_pfn(physical_address);

    // Ensure intermediate levels exist for the target VA.
    populate_page_table_path(virtual_address, false);
    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Existing mapping path: only accept if PFN matches requested PFN.
    if pt.entries[pt_idx].present() {
        let current_pfn = pt.entries[pt_idx].frame();
        if current_pfn != requested_pfn {
            return Err(MapError::AlreadyMapped {
                virtual_address,
                current_pfn,
                requested_pfn,
            });
        }

        return Ok(());
    }

    // Fresh mapping path.
    pt.entries[pt_idx].set_mapping(requested_pfn, true, true, false);
    invlpg(virtual_address);
    debug_alloc("PT", pt_idx, pt.entries[pt_idx].frame());
    Ok(())
}

/// Maps `virtual_address` to `physical_address` with present + writable flags.
///
/// Panics if the VA is already mapped to another frame.
#[cfg_attr(not(test), allow(dead_code))]
pub fn map_virtual_to_physical(virtual_address: u64, physical_address: u64) {
    // Thin wrapper: convert mapping conflicts into a hard panic.
    if let Err(MapError::AlreadyMapped {
        virtual_address,
        current_pfn,
        requested_pfn,
    }) = try_map_virtual_to_physical(virtual_address, physical_address)
    {
        panic!(
            "VMM: mapping conflict for VA 0x{:x}: current PFN=0x{:x}, requested PFN=0x{:x}",
            virtual_address, current_pfn, requested_pfn
        );
    }
}

/// Unmaps the given virtual address and invalidates the corresponding TLB entry.
pub fn unmap_virtual_address(virtual_address: u64) {
    // Operate on page boundary regardless of caller offset.
    let virtual_address = page_align_down(virtual_address);

    // If the hierarchy does not exist, unmap is already satisfied.
    let Some(pt) = pt_for_if_present(virtual_address) else {
        return;
    };

    let pt_idx = pt_index(virtual_address);
    if pt.entries[pt_idx].present() {
        // Remove leaf mapping and invalidate stale translation.
        let old_pfn = pt.entries[pt_idx].frame();
        pt.entries[pt_idx].clear();
        invlpg(virtual_address);
        // Return physical frame ownership to PMM when possible.
        let released = pmm::with_pmm(|mgr| mgr.release_pfn(old_pfn));
        if !released {
            // Best-effort warning for non-PMM-managed mappings.
            vmm_logln(format_args!(
                "VMM: warning: unmapped VA 0x{:x} had non-PMM PFN 0x{:x}",
                virtual_address, old_pfn
            ));
        }
    }
}

/// Clears the given mapping without releasing the mapped PFN back to PMM.
///
/// Intended for temporary virtual mappings to already-owned frames.
fn unmap_without_release(virtual_address: u64) {
    // Keep semantics for the mapped leaf (do not release), but prune and
    // release now-empty table levels so temporary mapping paths do not leak.
    unmap_page_and_prune_pagetable_hierarchy(virtual_address, false);
}

/// Clones the active kernel PML4 into a new physical frame for a user address space.
///
/// The returned physical address points to a copied PML4 image with recursive
/// mapping updated to self-reference in entry 511.
///
/// Detailed flow:
/// - Allocate one new physical frame that will hold the clone root table.
/// - Temporarily map that frame at [`TEMP_CLONE_PML4_VA`].
/// - Copy the currently active PML4 page (`PML4_TABLE_ADDR`) byte-for-byte
///   into the temporary mapping. This preserves kernel-half mappings.
/// - Update entry 511 inside the clone so its recursive mapping points to the
///   clone frame itself (not the original kernel PML4).
/// - Remove the temporary VA mapping and return the clone frame physical address.
///
/// Why not write directly via physical address:
/// - Rust code executes in virtual-address context; writing to arbitrary physical
///   addresses requires either identity mapping assumptions or a temporary map.
/// - Using one fixed scratch VA is explicit, deterministic, and avoids keeping
///   permanent helper mappings around.
///
/// Safety/ownership note:
/// - The returned frame remains allocated and owned by the caller.
/// - `unmap_without_release` is used intentionally so PMM does not free it.
pub fn clone_kernel_pml4_for_user() -> u64 {
    let new_pml4_phys = alloc_frame_phys();

    // Reuse one temporary VA for clone operations.
    unmap_without_release(TEMP_CLONE_PML4_VA);
    map_virtual_to_physical(TEMP_CLONE_PML4_VA, new_pml4_phys);

    unsafe {
        // SAFETY:
        // - Source is the current recursively mapped kernel PML4 page.
        // - Destination is a freshly allocated page mapped at TEMP_CLONE_PML4_VA.
        // - Regions are disjoint and exactly one page long.
        core::ptr::copy_nonoverlapping(
            PML4_TABLE_ADDR as *const u8,
            TEMP_CLONE_PML4_VA as *mut u8,
            SMALL_PAGE_SIZE as usize,
        );
    }

    // Rebind recursive slot 511 inside the clone to point to the clone itself.
    //
    // Background:
    // - In this kernel, PML4 entry 511 is used for recursive page-table mapping.
    // - After the raw memcpy above, entry 511 in the clone still points to the
    //   original kernel PML4 frame (copied value), which is wrong for an
    //   independent address-space root.
    //
    // What these lines do:
    // 1) Interpret the temporary clone mapping as a mutable PML4 table.
    // 2) Overwrite entry 511 so it maps `new_pml4_phys` (the clone frame).
    //
    // Result:
    // - Recursive virtual windows (PML4/PDP/PD/PT helper addresses) operate on
    //   the cloned hierarchy once this CR3 is activated, not on the kernel root.
    let clone_pml4 = table_at(TEMP_CLONE_PML4_VA);
    clone_pml4.entries[511].set_mapping(phys_to_pfn(new_pml4_phys), true, true, false);
    unmap_without_release(TEMP_CLONE_PML4_VA);

    new_pml4_phys
}

/// Destroys a user address space rooted at `pml4_phys`.
///
/// Teardown semantics:
/// - unmaps user-code and user-stack ranges,
/// - releases mapped PMM-managed leaf frames in stack range,
/// - keeps code-range leaf PFNs reserved (alias-safe default),
/// - prunes and releases now-empty PT/PD/PDP pages,
/// - releases the root PML4 frame itself.
pub fn destroy_user_address_space(pml4_phys: u64) {
    // Keep legacy default: do not release USER_CODE PFNs (alias-safe mode).
    destroy_user_address_space_with_options(pml4_phys, false);
}

/// Destroys a user address space rooted at `pml4_phys` with explicit code-page policy.
///
/// `release_user_code_pfns`:
/// - `false`: clear user-code mappings but keep mapped code PFNs reserved
///   (safe for temporary user aliases of kernel text pages),
/// - `true`: release user-code PFNs back to PMM (required for loader-owned images).
pub fn destroy_user_address_space_with_options(pml4_phys: u64, release_user_code_pfns: bool) {
    // Always operate on a canonical page-aligned root frame.
    let pml4_phys = page_align_down(pml4_phys);

    // A zero root is treated as "no address space" and is therefore a no-op.
    if pml4_phys == 0 {
        return;
    }

    // Teardown must run while the target CR3 is active so recursive-table
    // helper addresses resolve to the correct hierarchy.
    with_address_space(pml4_phys, || {
        // Drop USER_CODE mappings page-by-page.
        // Caller controls whether mapped code PFNs are returned to PMM.
        let mut va = USER_CODE_BASE;
        while va < USER_CODE_END {
            unmap_page_and_prune_pagetable_hierarchy(va, release_user_code_pfns);
            va += SMALL_PAGE_SIZE;
        }

        // USER_STACK pages are always process-owned; release leaf PFNs.
        let mut stack_va = USER_STACK_BASE;
        while stack_va < USER_STACK_TOP {
            unmap_page_and_prune_pagetable_hierarchy(stack_va, true);
            stack_va += SMALL_PAGE_SIZE;
        }
    });

    // Finally release the root PML4 frame itself after its hierarchy has been pruned.
    let released = pmm::with_pmm(|mgr| mgr.release_pfn(phys_to_pfn(pml4_phys)));

    if !released {
        // Best-effort diagnostics: teardown already cleared mappings, but PMM
        // ownership metadata was not in the expected state for this root PFN.
        vmm_logln(format_args!(
            "VMM: warning: destroy_user_address_space could not release root PFN 0x{:x}",
            phys_to_pfn(pml4_phys)
        ));
    }
}

/// Returns page-table PFNs `(pdp, pd, pt)` for `virtual_address` in active CR3.
///
/// Intended for diagnostics and integration tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn debug_table_pfns_for_va(virtual_address: u64) -> Option<(u64, u64, u64)> {
    // Diagnostics always inspect page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = pml4.entries[pml4_idx];

    if !pml4e.present() || pml4e.huge() {
        return None;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = pdp.entries[pdp_idx];

    if !pdpe.present() || pdpe.huge() {
        return None;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = pd.entries[pd_idx];

    if !pde.present() || pde.huge() {
        return None;
    }

    // Return intermediate table frame numbers for caller-side inspection.
    Some((pml4e.frame(), pdpe.frame(), pde.frame()))
}

/// Returns the mapped leaf PFN for `virtual_address` in active CR3.
///
/// Intended for diagnostics and integration tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn debug_mapped_pfn_for_va(virtual_address: u64) -> Option<u64> {
    // Diagnostics always inspect page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pt = pt_for_if_present(virtual_address)?;
    let pte = pt.entries[pt_index(virtual_address)];

    // Return leaf PFN only when mapping is currently present.
    if pte.present() {
        Some(pte.frame())
    } else {
        None
    }
}

/// Returns mapping user/writable flags `(pml4_u, pdp_u, pd_u, pt_u, pt_w)` for `virtual_address`.
///
/// Intended for diagnostics and integration tests.
#[cfg_attr(not(test), allow(dead_code))]
pub fn debug_mapping_flags_for_va(virtual_address: u64) -> Option<(bool, bool, bool, bool, bool)> {
    // Diagnostics always inspect page-aligned address.
    let virtual_address = page_align_down(virtual_address);
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = pml4.entries[pml4_idx];
    if !pml4e.present() || pml4e.huge() {
        return None;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = pdp.entries[pdp_idx];
    if !pdpe.present() || pdpe.huge() {
        return None;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = pd.entries[pd_idx];
    if !pde.present() || pde.huge() {
        return None;
    }

    let pt = table_at(pt_table_addr(virtual_address));
    let pte = pt.entries[pt_index(virtual_address)];
    if !pte.present() {
        return None;
    }

    // Return "user" propagation across all levels plus leaf writability.
    Some((
        pml4e.user(),
        pdpe.user(),
        pde.user(),
        pte.user(),
        pte.writable(),
    ))
}

/// Maps one user virtual page to `pfn` using user-accessible permissions.
///
/// `virtual_address` must be within configured user code/stack regions and
/// must not target the configured guard page.
pub fn map_user_page(virtual_address: u64, pfn: u64, writable: bool) -> Result<(), MapError> {
    // Normalize to 4 KiB page granularity; callers may pass any address
    // within the target page.
    let virtual_address = page_align_down(virtual_address);

    // Enforce user-window policy before touching page tables.
    // Only USER_CODE and USER_STACK are valid targets.
    match classify_user_region(virtual_address) {
        Some(UserRegion::Code) | Some(UserRegion::Stack) => {}
        Some(UserRegion::Guard) => {
            return Err(MapError::UserGuardPage { virtual_address });
        }
        None => {
            return Err(MapError::NotUserRegion { virtual_address });
        }
    }

    // Ensure all intermediate levels exist and are marked user-accessible.
    populate_page_table_path(virtual_address, true);
    let pt = table_at(pt_table_addr(virtual_address));
    let pt_idx = pt_index(virtual_address);

    // Existing mapping: allow idempotent "same PFN, permission update".
    // Reject remap attempts to a different PFN to avoid silent alias changes.
    if pt.entries[pt_idx].present() {
        let current_pfn = pt.entries[pt_idx].frame();

        if current_pfn != pfn {
            return Err(MapError::AlreadyMapped {
                virtual_address,
                current_pfn,
                requested_pfn: pfn,
            });
        }

        // Keep `present` + physical frame, update only user/writable flags.
        pt.entries[pt_idx].set_writable(writable);
        pt.entries[pt_idx].set_user(true);

        // A permission change (e.g. writable → read-only) is not visible to
        // the processor until the stale TLB entry for this VA is evicted.
        // Without invalidation the CPU may keep using the old cached translation
        // with the previous writable bit, allowing user code to modify pages
        // that should be read-only.
        invlpg(virtual_address);

        return Ok(());
    }

    // Fresh mapping path for previously non-present leaf.
    pt.entries[pt_idx].set_mapping(pfn, true, writable, true);

    // Invalidate stale translation for this VA in current TLB context.
    invlpg(virtual_address);

    Ok(())
}

/// Basic VMM smoke test that triggers page faults and verifies readback.
pub fn test_vmm() -> bool {
    // Step 1: force demand-mapping by writing to three sparse addresses.
    vmm_logln(format_args!("VMM test: start"));
    const TEST_ADDR1: u64 = 0xFFFF_8009_4F62_D000;
    const TEST_ADDR2: u64 = 0xFFFF_8034_C232_C000;
    const TEST_ADDR3: u64 = 0xFFFF_807F_7200_7000;
    vmm_logln(format_args!("VMM test: write to 0x{:x}", TEST_ADDR1));
    write_virt_u8(TEST_ADDR1, b'A');

    vmm_logln(format_args!("VMM test: write to 0x{:x}", TEST_ADDR2));
    write_virt_u8(TEST_ADDR2, b'B');

    vmm_logln(format_args!("VMM test: write to 0x{:x}", TEST_ADDR3));
    write_virt_u8(TEST_ADDR3, b'C');

    // Step 2: read back and validate data integrity.
    vmm_logln(format_args!("VMM test: readback and verify"));
    let v1 = read_virt_u8(TEST_ADDR1);
    let v2 = read_virt_u8(TEST_ADDR2);
    let v3 = read_virt_u8(TEST_ADDR3);

    let ok = v1 == b'A' && v2 == b'B' && v3 == b'C';
    if ok {
        vmm_logln(format_args!("VMM test: readback OK (A, B, C)"));
    } else {
        vmm_logln(format_args!(
            "VMM test: readback FAILED got [{:#x}, {:#x}, {:#x}] expected [0x41, 0x42, 0x43]",
            v1, v2, v3
        ));
    }

    // Unmap test pages so the next `vmmtest` run triggers page faults again.
    unmap_virtual_address(TEST_ADDR1);
    unmap_virtual_address(TEST_ADDR2);
    unmap_virtual_address(TEST_ADDR3);
    vmm_logln(format_args!("VMM test: unmapped test pages"));
    vmm_logln(format_args!("VMM test: done (ok={})", ok));
    vmm_logln(format_args!(""));
    ok
}
