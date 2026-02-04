//! Virtual memory manager for x86_64 4-level paging with recursive mapping.

use core::arch::asm;
use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::drivers::screen::Screen;
use crate::logging;
use crate::memory::pmm;

const PT_ENTRIES: usize = 512;
const SMALL_PAGE_SIZE: u64 = 4096;
const PAGE_MASK: u64 = !(SMALL_PAGE_SIZE - 1);

const PML4_TABLE_ADDR: u64 = 0xFFFF_FFFF_FFFF_F000;
const PDP_TABLE_BASE: u64 = 0xFFFF_FFFF_FFE0_0000;
const PD_TABLE_BASE: u64 = 0xFFFF_FFFF_C000_0000;
const PT_TABLE_BASE: u64 = 0xFFFF_FF80_0000_0000;

const ENTRY_PRESENT: u64 = 1 << 0;
const ENTRY_WRITABLE: u64 = 1 << 1;
const ENTRY_USER: u64 = 1 << 2;
const ENTRY_FRAME_MASK: u64 = 0x0000_FFFF_FFFF_F000;

#[derive(Clone, Copy)]
#[repr(transparent)]
struct PageTableEntry(u64);

impl PageTableEntry {
    #[inline]
    fn present(self) -> bool {
        (self.0 & ENTRY_PRESENT) != 0
    }

    #[inline]
    fn set_present(&mut self, val: bool) {
        if val {
            self.0 |= ENTRY_PRESENT;
        } else {
            self.0 &= !ENTRY_PRESENT;
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn writable(self) -> bool {
        (self.0 & ENTRY_WRITABLE) != 0
    }

    #[inline]
    fn set_writable(&mut self, val: bool) {
        if val {
            self.0 |= ENTRY_WRITABLE;
        } else {
            self.0 &= !ENTRY_WRITABLE;
        }
    }

    #[inline]
    #[allow(dead_code)]
    fn user(self) -> bool {
        (self.0 & ENTRY_USER) != 0
    }

    #[inline]
    fn set_user(&mut self, val: bool) {
        if val {
            self.0 |= ENTRY_USER;
        } else {
            self.0 &= !ENTRY_USER;
        }
    }

    #[inline]
    fn frame(self) -> u64 {
        (self.0 & ENTRY_FRAME_MASK) >> 12
    }

    #[inline]
    fn set_frame(&mut self, pfn: u64) {
        self.0 = (self.0 & !ENTRY_FRAME_MASK) | ((pfn << 12) & ENTRY_FRAME_MASK);
    }

    #[inline]
    fn set_mapping(&mut self, pfn: u64, present: bool, writable: bool, user: bool) {
        self.set_frame(pfn);
        self.set_present(present);
        self.set_writable(writable);
        self.set_user(user);
    }

    #[inline]
    fn clear(&mut self) {
        self.0 = 0;
    }
}

#[repr(C, align(4096))]
struct PageTable {
    entries: [PageTableEntry; PT_ENTRIES],
}

impl PageTable {
    #[inline]
    fn zero(&mut self) {
        for entry in self.entries.iter_mut() {
            entry.clear();
        }
    }
}

#[inline]
fn pml4_index(va: u64) -> usize {
    ((va >> 39) & 0x1FF) as usize
}

#[inline]
fn pdp_index(va: u64) -> usize {
    ((va >> 30) & 0x1FF) as usize
}

#[inline]
fn pd_index(va: u64) -> usize {
    ((va >> 21) & 0x1FF) as usize
}

#[inline]
fn pt_index(va: u64) -> usize {
    ((va >> 12) & 0x1FF) as usize
}

#[inline]
fn pdp_table_addr(va: u64) -> u64 {
    PDP_TABLE_BASE + ((va >> 27) & 0x0000_001F_F000)
}

#[inline]
fn pd_table_addr(va: u64) -> u64 {
    PD_TABLE_BASE + ((va >> 18) & 0x0000_3FFF_F000)
}

#[inline]
fn pt_table_addr(va: u64) -> u64 {
    PT_TABLE_BASE + ((va >> 9) & 0x0000_007F_FFFF_F000)
}

#[inline]
fn page_align_down(addr: u64) -> u64 {
    addr & PAGE_MASK
}

#[inline]
fn phys_to_pfn(addr: u64) -> u64 {
    addr / SMALL_PAGE_SIZE
}

unsafe fn read_cr3() -> u64 {
    let val: u64;
    unsafe {
        asm!("mov {}, cr3", out(reg) val, options(nomem, nostack, preserves_flags));
    }
    val
}

unsafe fn write_cr3(val: u64) {
    unsafe {
        asm!("mov cr3, {}", in(reg) val, options(nostack, preserves_flags));
    }
}

unsafe fn invlpg(addr: u64) {
    unsafe {
        asm!("invlpg [{}]", in(reg) addr, options(nostack, preserves_flags));
    }
}

struct VmmState {
    pml4_physical: u64,
    debug_enabled: bool,
}

struct GlobalVmm {
    inner: UnsafeCell<VmmState>,
    initialized: AtomicBool,
}

impl GlobalVmm {
    const fn new() -> Self {
        Self {
            inner: UnsafeCell::new(VmmState {
                pml4_physical: 0,
                debug_enabled: false,
            }),
            initialized: AtomicBool::new(false),
        }
    }
}

unsafe impl Sync for GlobalVmm {}

static VMM: GlobalVmm = GlobalVmm::new();

#[inline]
fn with_vmm<R>(f: impl FnOnce(&mut VmmState) -> R) -> R {
    debug_assert!(VMM.initialized.load(Ordering::Acquire), "VMM not initialized");
    unsafe { f(&mut *VMM.inner.get()) }
}

#[inline]
fn alloc_frame_phys() -> u64 {
    pmm::with_pmm(|mgr| {
        mgr.alloc_frame()
            .expect("VMM: out of physical memory while allocating page frame")
            .physical_address()
    })
}

#[inline]
unsafe fn table_at(addr: u64) -> &'static mut PageTable {
    unsafe { &mut *(addr as *mut PageTable) }
}

#[inline]
unsafe fn zero_phys_page(addr: u64) {
    unsafe {
        core::ptr::write_bytes(addr as *mut u8, 0, SMALL_PAGE_SIZE as usize);
    }
}

fn debug_enabled() -> bool {
    with_vmm(|state| state.debug_enabled)
}

/// Enables or disables VMM debug output and returns the previous setting.
pub fn set_debug_output(enabled: bool) -> bool {
    with_vmm(|state| {
        let old = state.debug_enabled;
        state.debug_enabled = enabled;
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
    logging::print_captured_target(screen, "vmm", |line| {
        line.starts_with("VMM: page fault raw=") || line.starts_with("VMM: indices pml4=")
    });
}

fn debug_alloc(level: &str, idx: usize, pfn: u64) {
    if debug_enabled() {
        logging::logln("vmm", format_args!(
            "VMM: allocated PFN 0x{:x} for {} entry 0x{:x}",
            pfn,
            level,
            idx
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
    let pml4 = alloc_frame_phys();
    let pdp_higher = alloc_frame_phys();
    let pd_higher = alloc_frame_phys();
    let pt_higher_0 = alloc_frame_phys();
    let pt_higher_1 = alloc_frame_phys();
    let pdp_identity = alloc_frame_phys();
    let pd_identity = alloc_frame_phys();
    let pt_identity_0 = alloc_frame_phys();
    let pt_identity_1 = alloc_frame_phys();

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
        unsafe { zero_phys_page(addr) };
    }

    unsafe {
        let pml4_tbl = table_at(pml4);
        pml4_tbl.entries[0].set_mapping(phys_to_pfn(pdp_identity), true, true, false);
        pml4_tbl.entries[256].set_mapping(phys_to_pfn(pdp_higher), true, true, false);
        pml4_tbl.entries[511].set_mapping(phys_to_pfn(pml4), true, true, false);

        let pdp_identity_tbl = table_at(pdp_identity);
        pdp_identity_tbl.entries[0].set_mapping(phys_to_pfn(pd_identity), true, true, false);

        let pd_identity_tbl = table_at(pd_identity);
        pd_identity_tbl.entries[0].set_mapping(phys_to_pfn(pt_identity_0), true, true, false);
        pd_identity_tbl.entries[1].set_mapping(phys_to_pfn(pt_identity_1), true, true, false);

        let pt_identity_tbl_0 = table_at(pt_identity_0);
        for i in 0..PT_ENTRIES {
            pt_identity_tbl_0.entries[i].set_mapping(i as u64, true, true, false);
        }

        let pt_identity_tbl_1 = table_at(pt_identity_1);
        for i in 0..PT_ENTRIES {
            pt_identity_tbl_1
                .entries[i]
                .set_mapping((PT_ENTRIES + i) as u64, true, true, false);
        }

        let pdp_higher_tbl = table_at(pdp_higher);
        pdp_higher_tbl.entries[0].set_mapping(phys_to_pfn(pd_higher), true, true, false);

        let pd_higher_tbl = table_at(pd_higher);
        pd_higher_tbl.entries[0].set_mapping(phys_to_pfn(pt_higher_0), true, true, false);
        pd_higher_tbl.entries[1].set_mapping(phys_to_pfn(pt_higher_1), true, true, false);

        let pt_higher_tbl_0 = table_at(pt_higher_0);
        for i in 0..PT_ENTRIES {
            pt_higher_tbl_0.entries[i].set_mapping(i as u64, true, true, false);
        }

        let pt_higher_tbl_1 = table_at(pt_higher_1);
        for i in 0..PT_ENTRIES {
            pt_higher_tbl_1
                .entries[i]
                .set_mapping((PT_ENTRIES + i) as u64, true, true, false);
        }
    }

    unsafe {
        (*VMM.inner.get()).pml4_physical = pml4;
        (*VMM.inner.get()).debug_enabled = debug_output;
    }
    VMM.initialized.store(true, Ordering::Release);

    unsafe { write_cr3(pml4) };
}

/// Returns the currently active kernel PML4 physical address.
#[allow(dead_code)]
pub fn get_pml4_address() -> u64 {
    with_vmm(|state| state.pml4_physical)
}

/// Switches to the provided page directory (physical PML4 address).
///
/// # Safety
/// The caller must ensure `pml4_phys` points to a valid, fully initialized
/// PML4 table in physical memory. Switching to an invalid CR3 target can
/// immediately crash the kernel due to page faults/triple fault.
#[allow(dead_code)]
pub unsafe fn switch_page_directory(pml4_phys: u64) {
    unsafe { write_cr3(pml4_phys) };
    with_vmm(|state| {
        state.pml4_physical = pml4_phys;
    });
}

#[inline]
unsafe fn ensure_tables_for(virtual_address: u64) {
    let pml4 = unsafe { table_at(PML4_TABLE_ADDR) };
    let pml4_idx = pml4_index(virtual_address);
    if !pml4.entries[pml4_idx].present() {
        let new_table_phys = alloc_frame_phys();
        pml4.entries[pml4_idx].set_mapping(phys_to_pfn(new_table_phys), true, true, false);
        unsafe { invlpg(pdp_table_addr(virtual_address)) };
        let new_pdp = unsafe { table_at(pdp_table_addr(virtual_address)) };
        new_pdp.zero();
        debug_alloc("PML4", pml4_idx, pml4.entries[pml4_idx].frame());
    }

    let pdp = unsafe { table_at(pdp_table_addr(virtual_address)) };
    let pdp_idx = pdp_index(virtual_address);
    if !pdp.entries[pdp_idx].present() {
        let new_table_phys = alloc_frame_phys();
        pdp.entries[pdp_idx].set_mapping(phys_to_pfn(new_table_phys), true, true, false);
        unsafe { invlpg(pd_table_addr(virtual_address)) };
        let new_pd = unsafe { table_at(pd_table_addr(virtual_address)) };
        new_pd.zero();
        debug_alloc("PDP", pdp_idx, pdp.entries[pdp_idx].frame());
    }

    let pd = unsafe { table_at(pd_table_addr(virtual_address)) };
    let pd_idx = pd_index(virtual_address);
    if !pd.entries[pd_idx].present() {
        let new_table_phys = alloc_frame_phys();
        pd.entries[pd_idx].set_mapping(phys_to_pfn(new_table_phys), true, true, false);
        unsafe { invlpg(pt_table_addr(virtual_address)) };
        let new_pt = unsafe { table_at(pt_table_addr(virtual_address)) };
        new_pt.zero();
        debug_alloc("PD", pd_idx, pd.entries[pd_idx].frame());
    }
}

/// Handles page faults by demand-allocating page tables and target page frame.
pub fn handle_page_fault(virtual_address: u64, error_code: u64) {
    let fault_address_raw = virtual_address;
    let virtual_address = page_align_down(fault_address_raw);

    if debug_enabled() {
        let cr3 = unsafe { read_cr3() };
        logging::logln("vmm", format_args!(
            "VMM: page fault raw=0x{:x} aligned=0x{:x} cr3=0x{:x} err=0x{:x}",
            fault_address_raw,
            virtual_address,
            cr3,
            error_code
        ));
        logging::logln("vmm", format_args!(
            "VMM: indices pml4={} pdp={} pd={} pt={}",
            pml4_index(virtual_address),
            pdp_index(virtual_address),
            pd_index(virtual_address),
            pt_index(virtual_address)
        ));
        logging::logln("vmm", format_args!(
            "VMM: err bits p={} w={} u={} rsv={} ifetch={}",
            (error_code & (1 << 0)) != 0,
            (error_code & (1 << 1)) != 0,
            (error_code & (1 << 2)) != 0,
            (error_code & (1 << 3)) != 0,
            (error_code & (1 << 4)) != 0
        ));
    }

    unsafe {
        ensure_tables_for(virtual_address);
        let pt = table_at(pt_table_addr(virtual_address));
        let pt_idx = pt_index(virtual_address);
        if !pt.entries[pt_idx].present() {
            let new_page_phys = alloc_frame_phys();
            pt.entries[pt_idx].set_mapping(phys_to_pfn(new_page_phys), true, true, false);
            debug_alloc("PT", pt_idx, pt.entries[pt_idx].frame());
        }
    }
}

/// Maps `virtual_address` to `physical_address` with present + writable flags.
#[allow(dead_code)]
pub fn map_virtual_to_physical(virtual_address: u64, physical_address: u64) {
    let virtual_address = page_align_down(virtual_address);
    let physical_address = page_align_down(physical_address);

    unsafe {
        ensure_tables_for(virtual_address);
        let pt = table_at(pt_table_addr(virtual_address));
        let pt_idx = pt_index(virtual_address);
        pt.entries[pt_idx].set_mapping(phys_to_pfn(physical_address), true, true, false);
        invlpg(virtual_address);
        debug_alloc("PT", pt_idx, pt.entries[pt_idx].frame());
    }
}

/// Unmaps the given virtual address and invalidates the corresponding TLB entry.
pub fn unmap_virtual_address(virtual_address: u64) {
    let virtual_address = page_align_down(virtual_address);

    unsafe {
        let pt = table_at(pt_table_addr(virtual_address));
        let pt_idx = pt_index(virtual_address);
        if pt.entries[pt_idx].present() {
            pt.entries[pt_idx].clear();
            invlpg(virtual_address);
        }
    }
}

/// Basic VMM smoke test that triggers page faults and verifies readback.
pub fn test_vmm() -> bool {
    logging::logln("vmm", format_args!("VMM test: start"));
    const TEST_ADDR1: u64 = 0xFFFF_8009_4F62_D000;
    const TEST_ADDR2: u64 = 0xFFFF_8034_C232_C000;
    const TEST_ADDR3: u64 = 0xFFFF_807F_7200_7000;
    let ok: bool;
    unsafe {
        logging::logln("vmm", format_args!("VMM test: write to 0x{:x}", TEST_ADDR1));
        let addr1 = TEST_ADDR1 as *mut u8;
        core::ptr::write_volatile(addr1, b'A');

        logging::logln("vmm", format_args!("VMM test: write to 0x{:x}", TEST_ADDR2));
        let ptr2 = TEST_ADDR2 as *mut u8;
        core::ptr::write_volatile(ptr2, b'B');

        logging::logln("vmm", format_args!("VMM test: write to 0x{:x}", TEST_ADDR3));
        let ptr3 = TEST_ADDR3 as *mut u8;
        core::ptr::write_volatile(ptr3, b'C');

        logging::logln("vmm", format_args!("VMM test: readback and verify"));
        let v1 = core::ptr::read_volatile(addr1);
        let v2 = core::ptr::read_volatile(ptr2);
        let v3 = core::ptr::read_volatile(ptr3);

        ok = v1 == b'A' && v2 == b'B' && v3 == b'C';
        if ok {
            logging::logln("vmm", format_args!("VMM test: readback OK (A, B, C)"));
        } else {
            logging::logln("vmm", format_args!(
                "VMM test: readback FAILED got [{:#x}, {:#x}, {:#x}] expected [0x41, 0x42, 0x43]",
                v1,
                v2,
                v3
            ));
        }

        // Unmap test pages so the next `vmmtest` run triggers page faults again.
        unmap_virtual_address(addr1 as u64);
        unmap_virtual_address(ptr2 as u64);
        unmap_virtual_address(ptr3 as u64);
        logging::logln("vmm", format_args!("VMM test: unmapped test pages"));
    }
    logging::logln("vmm", format_args!("VMM test: done (ok={})", ok));
    logging::logln("vmm", format_args!(""));
    ok
}
