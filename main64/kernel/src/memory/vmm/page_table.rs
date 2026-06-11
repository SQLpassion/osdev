use crate::arch::constants::PAGE_SIZE_U64;
use crate::memory::pmm;
use core::arch::asm;

pub const PT_ENTRIES: usize = 512;
pub const PAGE_MASK: u64 = !(PAGE_SIZE_U64 - 1);

pub const ENTRY_PRESENT: u64 = 1 << 0;
pub const ENTRY_WRITABLE: u64 = 1 << 1;
pub const ENTRY_USER: u64 = 1 << 2;
pub const ENTRY_HUGE: u64 = 1 << 7;
pub const ENTRY_GLOBAL: u64 = 1 << 8;
pub const ENTRY_FRAME_MASK: u64 = 0x0000_FFFF_FFFF_F000;

/// No-Execute bit (bit 63) in a page table leaf entry.
///
/// When set, instruction fetches from this page raise a #PF (page fault).
/// Only effective after EFER.NXE is set — see `kaosldr_16/longmode.asm`.
/// Applied to user stack pages to prevent code injection via stack buffer overflows.
/// Must NOT be set on code pages (USER_CODE region).
pub const ENTRY_NO_EXECUTE: u64 = 1 << 63;

pub const PML4_TABLE_ADDR: u64 = 0xFFFF_FFFF_FFFF_F000;
pub const PDP_TABLE_BASE: u64 = 0xFFFF_FFFF_FFE0_0000;
pub const PD_TABLE_BASE: u64 = 0xFFFF_FFFF_C000_0000;
pub const PT_TABLE_BASE: u64 = 0xFFFF_FF80_0000_0000;

#[derive(Clone, Copy)]
#[repr(transparent)]
pub struct PageTableEntry(u64);

impl PageTableEntry {
    /// Returns whether this entry is marked present.
    #[inline]
    pub fn present(self) -> bool {
        (self.0 & ENTRY_PRESENT) != 0
    }

    /// Sets or clears the present bit.
    #[inline]
    pub fn set_present(&mut self, val: bool) {
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
    pub fn writable(self) -> bool {
        (self.0 & ENTRY_WRITABLE) != 0
    }

    /// Sets or clears the writable bit.
    #[inline]
    pub fn set_writable(&mut self, val: bool) {
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
    pub fn user(self) -> bool {
        (self.0 & ENTRY_USER) != 0
    }

    /// Sets or clears the user-accessible bit.
    #[inline]
    pub fn set_user(&mut self, val: bool) {
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
    pub fn global(self) -> bool {
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
    pub fn set_global(&mut self, val: bool) {
        // Toggle only the global bit and keep all other fields intact.
        if val {
            self.0 |= ENTRY_GLOBAL;
        } else {
            self.0 &= !ENTRY_GLOBAL;
        }
    }

    /// Returns whether the No-Execute bit is set.
    ///
    /// When set, instruction fetches from this page raise a #PF.
    /// Requires EFER.NXE to be active (set in `kaosldr_16/longmode.asm`);
    /// without it the CPU ignores this bit and the page remains executable.
    #[inline]
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn no_execute(self) -> bool {
        (self.0 & ENTRY_NO_EXECUTE) != 0
    }

    /// Sets or clears the No-Execute bit (bit 63).
    ///
    /// Set this on stack and data pages to prevent code injection attacks.
    /// Never set this on code pages (USER_CODE region) — they must remain executable.
    /// Requires EFER.NXE to be active (set in `kaosldr_16/longmode.asm`).
    #[inline]
    pub fn set_no_execute(&mut self, val: bool) {
        if val {
            self.0 |= ENTRY_NO_EXECUTE;
        } else {
            self.0 &= !ENTRY_NO_EXECUTE;
        }
    }

    /// Returns the mapped page-frame number.
    #[inline]
    pub fn frame(self) -> u64 {
        (self.0 & ENTRY_FRAME_MASK) >> 12
    }

    /// Writes the page-frame number into the entry frame field.
    #[inline]
    pub fn set_frame(&mut self, pfn: u64) {
        self.0 = (self.0 & !ENTRY_FRAME_MASK) | ((pfn << 12) & ENTRY_FRAME_MASK);
    }

    /// Sets frame plus basic permission bits in one call.
    #[inline]
    pub fn set_mapping(&mut self, pfn: u64, present: bool, writable: bool, user: bool) {
        self.set_frame(pfn);
        self.set_present(present);
        self.set_writable(writable);
        self.set_user(user);
    }

    /// Clears the entry to an unmapped state.
    #[inline]
    pub fn clear(&mut self) {
        self.0 = 0;
    }

    /// Returns whether the huge-page bit is set.
    #[inline]
    pub fn huge(self) -> bool {
        (self.0 & ENTRY_HUGE) != 0
    }
}

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; PT_ENTRIES],
}

impl PageTable {
    /// Clears all page-table entries.
    #[inline]
    pub fn zero(&mut self) {
        // Reset every entry to "not present".
        for entry in self.entries.iter_mut() {
            entry.clear();
        }
    }
}

/// Returns the PML4 index for a canonical virtual address.
#[inline]
pub fn pml4_index(va: u64) -> usize {
    ((va >> 39) & 0x1FF) as usize
}

/// Returns the PDP index for a canonical virtual address.
#[inline]
pub fn pdp_index(va: u64) -> usize {
    ((va >> 30) & 0x1FF) as usize
}

/// Returns the PD index for a canonical virtual address.
#[inline]
pub fn pd_index(va: u64) -> usize {
    ((va >> 21) & 0x1FF) as usize
}

/// Returns the PT index for a canonical virtual address.
#[inline]
pub fn pt_index(va: u64) -> usize {
    ((va >> 12) & 0x1FF) as usize
}

/// Returns the recursive-mapping virtual address of the PDP table for `va`.
#[inline]
pub fn pdp_table_addr(va: u64) -> u64 {
    PDP_TABLE_BASE + ((va >> 27) & 0x0000_001F_F000)
}

/// Returns the recursive-mapping virtual address of the PD table for `va`.
#[inline]
pub fn pd_table_addr(va: u64) -> u64 {
    PD_TABLE_BASE + ((va >> 18) & 0x0000_3FFF_F000)
}

/// Returns the recursive-mapping virtual address of the PT table for `va`.
#[inline]
pub fn pt_table_addr(va: u64) -> u64 {
    PT_TABLE_BASE + ((va >> 9) & 0x0000_007F_FFFF_F000)
}

/// Aligns an address down to a 4 KiB page boundary.
#[inline]
pub fn page_align_down(addr: u64) -> u64 {
    addr & PAGE_MASK
}

/// Converts a physical byte address to a page-frame number.
#[inline]
pub fn phys_to_pfn(addr: u64) -> u64 {
    addr / PAGE_SIZE_U64
}

/// Reads the current CR3 value.
///
/// Caller contract: must run in ring 0 on x86_64.
pub fn read_cr3() -> u64 {
    let val: u64;
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
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
pub fn write_cr3(val: u64) {
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Caller guarantees `val` points to a valid PML4 root frame.
    // - Executed only in ring 0.
    unsafe {
        asm!("mov cr3, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

/// Invalidates one TLB entry for the given virtual address.
///
/// Caller contract: must run in ring 0 on x86_64.
pub fn invlpg(addr: u64) {
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
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
pub unsafe fn enable_global_pages() {
    const CR4_PGE: u64 = 1 << 7; // Page Global Enable bit

    let mut cr4: u64;
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Accessing CR4 is privileged and valid in ring 0.
    // - This sequence only toggles CR4.PGE and preserves all other CR4 bits.
    unsafe {
        // Read current CR4 value
        asm!("mov {}, cr4", out(reg) cr4, options(nomem, nostack, preserves_flags));
        // Set PGE bit
        cr4 |= CR4_PGE;
        // Write back to CR4
        asm!("mov cr4, {}", in(reg) cr4, options(nostack, preserves_flags));
    }
}

/// Interprets a recursive-mapped virtual address as a mutable page table.
///
/// Caller contract: `addr` must point to a valid, mapped page-table page.
#[inline]
pub(crate) fn table_at(addr: u64) -> *mut PageTable {
    addr as *mut PageTable
}

#[inline]
pub(crate) fn table_zero(table: *mut PageTable) {
    // SAFETY:
    // - This requires `unsafe` because raw-pointer dereference cannot be validated by Rust.
    // - Caller guarantees `table` points to a valid writable page-table page.
    unsafe {
        (*table).zero();
    }
}

#[inline]
pub(crate) fn table_entry(table: *const PageTable, idx: usize) -> PageTableEntry {
    // SAFETY:
    // - This requires `unsafe` because raw-pointer dereference cannot be validated by Rust.
    // - Caller guarantees `table` points to a valid page-table page and `idx < PT_ENTRIES`.
    unsafe { (*table).entries[idx] }
}

/// Returns a raw mutable pointer to the page-table entry at `idx` within `table`.
///
/// This is the single write primitive for all page-table modifications.
/// Callers dereference the returned pointer inside `unsafe` blocks that
/// document the site-specific safety invariants.
///
/// # Safety
/// - `table` must point to a valid, mapped page-table page.
/// - `idx < PT_ENTRIES` (512).
/// - No other mutable reference to `table.entries[idx]` may exist for the
///   duration of the dereference.
#[inline]
pub unsafe fn entry_ptr(table: *mut PageTable, idx: usize) -> *mut PageTableEntry {
    core::ptr::addr_of_mut!((*table).entries[idx])
}

/// Zeros one 4 KiB page in physical memory.
///
/// Caller contract: `addr` must be writable and page-aligned physical memory.
#[inline]
pub fn zero_phys_page(addr: u64) {
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - Caller guarantees `addr` is writable and page-aligned.
    // - Writes exactly one 4 KiB page.
    unsafe {
        core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE_U64 as usize);
    }
}

/// Zeros one already-mapped 4 KiB virtual page.
#[inline]
pub fn zero_virt_page(addr: u64) {
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - Caller guarantees `addr` points to a currently mapped writable page.
    // - Writes exactly one 4 KiB page.
    unsafe {
        core::ptr::write_bytes(addr as *mut u8, 0, PAGE_SIZE_U64 as usize);
    }
}

/// Returns the PT containing `virtual_address` if all intermediate levels exist.
///
/// Returns `None` if any level is non-present or uses a huge page mapping.
///
#[inline]
pub fn pt_for_if_present(virtual_address: u64) -> Option<*mut PageTable> {
    // Resolve PML4 level and reject missing/huge entries.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4_idx = pml4_index(virtual_address);
    let pml4e = table_entry(pml4, pml4_idx);

    if !pml4e.present() || pml4e.huge() {
        return None;
    }

    // Resolve PDP level and reject missing/huge entries.
    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdp_idx = pdp_index(virtual_address);
    let pdpe = table_entry(pdp, pdp_idx);

    if !pdpe.present() || pdpe.huge() {
        return None;
    }

    // Resolve PD level and reject missing/huge entries.
    let pd = table_at(pd_table_addr(virtual_address));
    let pd_idx = pd_index(virtual_address);
    let pde = table_entry(pd, pd_idx);

    if !pde.present() || pde.huge() {
        return None;
    }

    // All intermediate levels are present => return leaf PT table.
    Some(table_at(pt_table_addr(virtual_address)))
}

/// Returns whether a present leaf mapping exists for `virtual_address`.
#[inline]
pub fn is_leaf_present(virtual_address: u64) -> bool {
    let Some(pt) = pt_for_if_present(virtual_address) else {
        return false;
    };
    table_entry(pt, pt_index(virtual_address)).present()
}

/// Returns whether a page-table page contains no present entries.
#[inline]
pub fn table_is_empty(table: *const PageTable) -> bool {
    for idx in 0..PT_ENTRIES {
        if table_entry(table, idx).present() {
            return false;
        }
    }
    true
}

/// Attempts to allocate one physical frame and returns its physical address.
#[inline]
pub fn alloc_frame_phys() -> Option<u64> {
    pmm::with_pmm(|mgr| mgr.alloc_frame().map(|frame| frame.physical_address()))
}

/// Allocates one physical frame and panics with `context` on OOM.
///
/// Bootstrap paths use this helper because they cannot recover gracefully.
#[inline]
pub fn alloc_frame_phys_or_panic(context: &str) -> u64 {
    alloc_frame_phys().unwrap_or_else(|| panic!("{}", context))
}
