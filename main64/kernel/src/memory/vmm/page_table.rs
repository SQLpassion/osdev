use crate::arch::constants::PAGE_SIZE_U64;
use crate::memory::pmm;
use core::arch::asm;

pub const PT_ENTRIES: usize = 512;
pub const PAGE_MASK: u64 = !(PAGE_SIZE_U64 - 1);

/// PML4 slot used for the recursive self-map (PML4[511] -> the PML4 itself).
pub const RECURSIVE_SLOT: usize = 511;

pub const ENTRY_PRESENT: u64 = 1 << 0;
pub const ENTRY_WRITABLE: u64 = 1 << 1;
pub const ENTRY_USER: u64 = 1 << 2;
pub const ENTRY_PWT: u64 = 1 << 3;
pub const ENTRY_PCD: u64 = 1 << 4;
pub const ENTRY_HUGE: u64 = 1 << 7;
pub const ENTRY_GLOBAL: u64 = 1 << 8;
pub const ENTRY_FRAME_MASK: u64 = 0x0000_FFFF_FFFF_F000;

/// No-Execute bit (bit 63) in a page table leaf entry.
///
/// When set, instruction fetches from this page raise a #PF (page fault).
/// Only effective after EFER.NXE is set — enabled by `arch::msr::enable_no_execute`.
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

    /// Sets or clears the Page-Level Write-Through (PWT) bit.
    #[inline]
    pub fn set_pwt(&mut self, val: bool) {
        if val {
            self.0 |= ENTRY_PWT;
        } else {
            self.0 &= !ENTRY_PWT;
        }
    }

    /// Sets or clears the Page-Level Cache Disable (PCD) bit.
    #[inline]
    pub fn set_pcd(&mut self, val: bool) {
        if val {
            self.0 |= ENTRY_PCD;
        } else {
            self.0 &= !ENTRY_PCD;
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
    /// Requires EFER.NXE to be active (enabled by `arch::msr::enable_no_execute`);
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
    /// Requires EFER.NXE to be active (enabled by `arch::msr::enable_no_execute`).
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
    ///
    /// Used during page walks to detect/reject huge-page (1 GiB / 2 MiB) leaves. The
    /// kernel only ever *creates* 4 KiB mappings, so there is no `set_huge` setter.
    #[inline]
    pub fn huge(self) -> bool {
        (self.0 & ENTRY_HUGE) != 0
    }

    /// Returns the raw 64-bit entry value (frame bits + flags). Primarily for tests
    /// that assert an exact, verbatim copy of an entry.
    #[inline]
    pub fn raw(self) -> u64 {
        self.0
    }
}

#[repr(C, align(4096))]
pub struct PageTable {
    pub entries: [PageTableEntry; PT_ENTRIES],
}

impl Default for PageTable {
    #[inline]
    fn default() -> Self {
        Self::new()
    }
}

impl PageTable {
    /// Returns a fully zeroed (all-entries-not-present) page table. Useful for tests
    /// and for any caller that wants a blank table to fill.
    #[inline]
    pub const fn new() -> Self {
        Self {
            entries: [PageTableEntry(0); PT_ENTRIES],
        }
    }

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

/// Builds a kernel PML4 in `dst` as a SUPERSET of the firmware PML4 in `src`:
/// copies all 512 top-level entries verbatim, then installs the recursive self-map at
/// `RECURSIVE_SLOT` (511) pointing at `dst_phys` — the physical frame backing `dst`.
///
/// This is the core of `vmm::init` on the UEFI path, factored out as a pure function so
/// it can be unit-tested without switching CR3. See `vmm.md` §4 for the rationale (a
/// minimal hand-built map reset real AMD hardware; cloning the firmware PML4 does not).
///
/// # Safety
/// - `src` and `dst` must point to valid, mapped 4 KiB page-table pages.
/// - `dst_phys` must be the physical address of the frame backing `dst`.
/// - No other reference to either table may exist for the duration of the call.
pub unsafe fn build_kernel_pml4_from_firmware(
    src: *const PageTable,
    dst: *mut PageTable,
    dst_phys: u64,
) {
    // Copy every firmware top-level entry verbatim (full identity, higher-half mirror,
    // SMM/ACPI/MMIO/runtime regions).
    for i in 0..PT_ENTRIES {
        // SAFETY: caller guarantees both tables are valid; `i < PT_ENTRIES`.
        *entry_ptr(dst, i) = table_entry(src, i);
    }
    // Install the recursive self-map (overrides whatever firmware had at slot 511) so the
    // VMM can edit page tables through the recursive window.
    // SAFETY: `dst` is valid; `RECURSIVE_SLOT < PT_ENTRIES`.
    (*entry_ptr(dst, RECURSIVE_SLOT)).set_mapping(phys_to_pfn(dst_phys), true, true, false);
}

/// Walks the firmware page-table hierarchy rooted at the current CR3 and marks
/// every page-table frame (the PML4, plus all present PDPTs, PDs and PTs) as used
/// in the PMM.
///
/// `vmm::init` builds the kernel PML4 as a clone of the firmware's top-level
/// entries (see [`build_kernel_pml4_from_firmware`]), so the kernel keeps pointing
/// at firmware-owned PDPT/PD/PT frames. The PMM has no idea those frames are in
/// use and would otherwise hand them out via `alloc_frame()`, after which a later
/// write would corrupt the live page tables — a sporadic, hard-to-debug failure.
/// Reserving them here — right after `pmm::init` and *before* the first significant
/// allocation (the kernel PML4 frame in `vmm::init`) — closes that window.
///
/// Frames are reached through the firmware identity map (physical == virtual),
/// which is still active because CR3 has not been switched yet. Huge-page leaves
/// (1 GiB PDPT entries, 2 MiB PD entries) map data rather than sub-tables, so we
/// neither recurse into them nor reserve their targets; likewise PT entries are
/// 4 KiB data leaves, not table frames.
///
/// # Safety
/// - Must be called while the firmware identity map is active (before `write_cr3`),
///   so that each table's physical address is also a valid virtual address.
/// - The PMM must already be initialized.
pub unsafe fn reserve_firmware_page_tables() {
    let pml4_phys = read_cr3() & ENTRY_FRAME_MASK;

    // Hold the PMM lock for the whole walk: it runs once at boot with interrupts
    // disabled, and a single critical section keeps the reservation atomic.
    pmm::with_pmm(|mgr| {
        // The top-level table frame itself.
        mgr.mark_frame_used(pml4_phys);

        let pml4 = table_at(pml4_phys);
        for i in 0..PT_ENTRIES {
            let pml4e = table_entry(pml4, i);
            if !pml4e.present() {
                continue;
            }
            let pdpt_phys = pml4e.frame() * PAGE_SIZE_U64;
            mgr.mark_frame_used(pdpt_phys);

            let pdpt = table_at(pdpt_phys);
            for j in 0..PT_ENTRIES {
                let pdpte = table_entry(pdpt, j);
                // Skip non-present entries and 1 GiB huge-page leaves (no sub-table).
                if !pdpte.present() || pdpte.huge() {
                    continue;
                }
                let pd_phys = pdpte.frame() * PAGE_SIZE_U64;
                mgr.mark_frame_used(pd_phys);

                let pd = table_at(pd_phys);
                for k in 0..PT_ENTRIES {
                    let pde = table_entry(pd, k);
                    // Skip non-present entries and 2 MiB huge-page leaves (no sub-table).
                    if !pde.present() || pde.huge() {
                        continue;
                    }
                    let pt_phys = pde.frame() * PAGE_SIZE_U64;
                    mgr.mark_frame_used(pt_phys);
                    // PT entries are 4 KiB data leaves, not table frames — nothing to do.
                }
            }
        }
    });
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

/// Returns whether one virtual page is present and effectively user-writable.
///
/// Every paging level contributes to x86_64 access permissions. A supervisor
/// or read-only intermediate entry therefore makes the final mapping unusable
/// for a user write even when the leaf PTE itself is writable.
#[inline]
pub fn is_user_page_writable(virtual_address: u64) -> bool {
    // Step 1: Walk the PML4 and reject absent or supervisor/read-only paths.
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4e = table_entry(pml4, pml4_index(virtual_address));
    if !pml4e.present() || !pml4e.user() || !pml4e.writable() {
        return false;
    }

    // Step 2: Apply the same effective-permission check at the PDPT level.
    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdpe = table_entry(pdp, pdp_index(virtual_address));
    if !pdpe.present() || !pdpe.user() || !pdpe.writable() {
        return false;
    }
    if pdpe.huge() {
        return true;
    }

    // Step 3: Check the page-directory level before descending to a 4 KiB PT.
    let pd = table_at(pd_table_addr(virtual_address));
    let pde = table_entry(pd, pd_index(virtual_address));
    if !pde.present() || !pde.user() || !pde.writable() {
        return false;
    }
    if pde.huge() {
        return true;
    }

    // Step 4: Validate the final 4 KiB leaf entry.
    let pt = table_at(pt_table_addr(virtual_address));
    let pte = table_entry(pt, pt_index(virtual_address));
    pte.present() && pte.user() && pte.writable()
}

/// Returns whether `virtual_address` is currently mapped at ANY page granularity —
/// a 4 KiB leaf PTE, a 2 MiB PD huge page, or a 1 GiB PDPT huge page.
///
/// Unlike [`is_leaf_present`] / [`pt_for_if_present`] (which return `false`/`None` for
/// huge-page mappings because they only inspect the 4 KiB PT level), this recognizes huge
/// pages too. That distinction matters for firmware-established mappings: UEFI commonly maps
/// MMIO / framebuffer ranges with 2 MiB or 1 GiB pages, and those are cloned into the kernel
/// PML4 by `build_kernel_pml4_from_firmware`. Callers that decide whether to create a fresh
/// mapping (e.g. the framebuffer identity map) must use this, otherwise they would try to map
/// over an existing huge page and corrupt the page-table walk.
///
/// The walk short-circuits before dereferencing a level that does not exist (a huge page has
/// no lower table), so the recursive-mapping accesses below are always valid.
#[inline]
pub fn is_va_mapped(virtual_address: u64) -> bool {
    let pml4 = table_at(PML4_TABLE_ADDR);
    let pml4e = table_entry(pml4, pml4_index(virtual_address));
    if !pml4e.present() {
        return false;
    }

    let pdp = table_at(pdp_table_addr(virtual_address));
    let pdpe = table_entry(pdp, pdp_index(virtual_address));
    if !pdpe.present() {
        return false;
    }
    // 1 GiB huge page: mapped, and there is no PD below it.
    if pdpe.huge() {
        return true;
    }

    let pd = table_at(pd_table_addr(virtual_address));
    let pde = table_entry(pd, pd_index(virtual_address));
    if !pde.present() {
        return false;
    }
    // 2 MiB huge page: mapped, and there is no PT below it.
    if pde.huge() {
        return true;
    }

    let pt = table_at(pt_table_addr(virtual_address));
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
