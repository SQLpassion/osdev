//! Global Descriptor Table (GDT) and Task-State Segment (TSS) setup.
//!
//! This module installs a minimal long-mode GDT with:
//! - kernel code/data segments
//! - user code/data segments
//! - one available 64-bit TSS descriptor
//!
//! It is the architectural foundation required before ring-3 tasks can be
//! introduced. No ring-3 task execution is implemented here yet.

use core::arch::{asm, global_asm};
use core::cell::UnsafeCell;
use core::mem::size_of;
use core::sync::atomic::{AtomicBool, Ordering};

// Layout of our GDT array:
//   [0] null
//   [1] kernel code
//   [2] kernel data
//   [3] user code
//   [4] user data
//   [5] TSS descriptor (low 8 bytes)
//   [6] TSS descriptor (high 8 bytes)
const GDT_ENTRY_COUNT: usize = 7;
const KERNEL_CODE_INDEX: u16 = 1;
const KERNEL_DATA_INDEX: u16 = 2;
const USER_CODE_INDEX: u16 = 3;
const USER_DATA_INDEX: u16 = 4;
const TSS_INDEX: u16 = 5;

/// Requested Privilege Level (RPL) for ring 3.
const RPL_RING3: u16 = 0x3;

/// Kernel code segment selector (ring 0).
pub const KERNEL_CODE_SELECTOR: u16 = KERNEL_CODE_INDEX << 3;

/// Kernel data segment selector (ring 0).
pub const KERNEL_DATA_SELECTOR: u16 = KERNEL_DATA_INDEX << 3;

/// User code segment selector (ring 3).
pub const USER_CODE_SELECTOR: u16 = (USER_CODE_INDEX << 3) | RPL_RING3;

/// User data segment selector (ring 3).
pub const USER_DATA_SELECTOR: u16 = (USER_DATA_INDEX << 3) | RPL_RING3;

/// TSS selector.
#[cfg_attr(not(test), allow(dead_code))]
pub const TSS_SELECTOR: u16 = TSS_INDEX << 3;

// x86 descriptor access-byte bits.
const ACCESS_PRESENT: u8 = 1 << 7;
const ACCESS_SEGMENT: u8 = 1 << 4; // 1 = code/data segment, 0 = system segment
const ACCESS_EXECUTABLE: u8 = 1 << 3; // code=1, data=0
const ACCESS_RW: u8 = 1 << 1; // readable code / writable data
const ACCESS_RING3: u8 = 0b11 << 5; // DPL=3
const ACCESS_TSS_AVAILABLE: u8 = 0x9; // 64-bit available TSS system type

// Granularity-byte upper nibble bits (G, DB, L, AVL).
// For long-mode code segments we set L=1 and keep others 0.
const FLAGS_LONG_MODE: u8 = 1 << 5;
const IST_STACK_SIZE: usize = 16 * 1024;

#[repr(C, packed)]
struct DescriptorTablePointer {
    limit: u16,
    base: u64,
}

/// 64-bit long-mode Task State Segment.
///
/// Modern x86_64 kernels use this mainly for privilege transitions:
/// - `rsp0` is loaded by the CPU when entering ring 0 from ring 3.
/// - IST entries can provide dedicated emergency stacks (not used yet here).
#[repr(C, packed)]
struct TaskStateSegment {
    _reserved0: u32,
    rsp0: u64,
    _rsp1: u64,
    _rsp2: u64,
    _reserved1: u64,
    _ist1: u64,
    _ist2: u64,
    _ist3: u64,
    _ist4: u64,
    _ist5: u64,
    _ist6: u64,
    _ist7: u64,
    _reserved2: u64,
    _reserved3: u16,
    io_map_base: u16,
}

impl TaskStateSegment {
    const fn new() -> Self {
        Self {
            _reserved0: 0,
            rsp0: 0,
            _rsp1: 0,
            _rsp2: 0,
            _reserved1: 0,
            _ist1: 0,
            _ist2: 0,
            _ist3: 0,
            _ist4: 0,
            _ist5: 0,
            _ist6: 0,
            _ist7: 0,
            _reserved2: 0,
            _reserved3: 0,
            io_map_base: 0,
        }
    }
}

struct GdtState {
    gdt: UnsafeCell<[u64; GDT_ENTRY_COUNT]>,
    tss: UnsafeCell<TaskStateSegment>,
}

impl GdtState {
    const fn new() -> Self {
        Self {
            gdt: UnsafeCell::new([0; GDT_ENTRY_COUNT]),
            tss: UnsafeCell::new(TaskStateSegment::new()),
        }
    }
}

// SAFETY:
// - This requires `unsafe` because the compiler cannot automatically verify the thread-safety invariants of this `unsafe impl`.
// - `GdtState` is a singleton accessed in controlled boot sequencing.
// - Mutable access uses `UnsafeCell` under kernel initialization invariants.
unsafe impl Sync for GdtState {}

// SAFETY:
// - The kernel runs on a single CPU core (no SMP).
// - Mutations happen during early boot init or explicit setter calls.
// - Interior mutability is synchronized by boot-time sequencing.
static STATE: GdtState = GdtState::new();
static INITIALIZED: AtomicBool = AtomicBool::new(false);

/// 16-byte aligned backing storage for the double-fault IST stack.
#[repr(align(16))]
struct IstStack([u8; IST_STACK_SIZE]);

// SAFETY:
// - Single-core kernel: initialization/writes happen in controlled boot order.
// - The stack memory is static for the entire kernel lifetime.
static mut DOUBLE_FAULT_IST_STACK: IstStack = IstStack([0; IST_STACK_SIZE]);

extern "C" {
    // Assembly helper that loads GDTR, refreshes data segments, then loads TR.
    // Arguments follow SysV x86_64 ABI:
    //   rdi = gdt_ptr
    //   rsi = data_selector
    //   rdx = code_selector (currently unused in asm helper)
    //   rcx = tss_selector
    fn gdt_flush_and_reload(
        gdt_ptr: *const DescriptorTablePointer,
        data_selector: u16,
        _code_selector: u16,
        tss_selector: u16,
    );
}

global_asm!(
    r#"
    .section .text
    .global gdt_flush_and_reload
    .type gdt_flush_and_reload, @function
gdt_flush_and_reload:
    # Load GDTR with the kernel-owned descriptor table pointer.
    lgdt [rdi]

    # Reload data-segment registers so they resolve against the new GDT.
    # In long mode these bases are mostly ignored, but selectors still must
    # reference valid descriptors for privilege checks and stack semantics.
    mov ax, si
    mov ds, ax
    mov es, ax
    mov fs, ax
    mov gs, ax
    mov ss, ax

    # NOTE:
    # - We intentionally do not reload CS here via far return/jump yet.
    #   The current long-mode code selector from boot remains valid.
    # - Load task register (TR) with the TSS selector so CPU can use TSS.RSP0
    #   for ring3->ring0 transitions.
    mov ax, cx
    ltr ax
    ret
"#,
);

#[inline]
const fn build_code_or_data_descriptor(access: u8, flags: u8) -> u64 {
    // Long-mode code/data descriptors keep base=0 and limit=0.
    // Descriptor byte layout (legacy format retained in long mode):
    // - access byte       -> bits 47:40
    // - granularity byte  -> bits 55:48
    //   (upper nibble contains G/DB/L/AVL flags, lower nibble extends limit)
    //
    // We keep base/limit at 0 for flat segments.
    ((access as u64) << 40) | (((flags as u64) & 0xF0) << 48)
}

#[inline]
const fn build_tss_descriptor(base: u64, limit: u32) -> (u64, u64) {
    // 64-bit TSS descriptor encoding (16 bytes):
    // - low qword contains limit, base[31:0], type/present, high limit bits
    // - high qword contains base[63:32]
    let mut low = 0u64;
    low |= (limit as u64) & 0xFFFF;
    low |= (base & 0xFFFF) << 16;
    low |= ((base >> 16) & 0xFF) << 32;
    low |= ((ACCESS_PRESENT | ACCESS_TSS_AVAILABLE) as u64) << 40;
    low |= (((limit >> 16) as u8 & 0x0F) as u64) << 48;
    low |= ((base >> 24) & 0xFF) << 56;

    let mut high = 0u64;
    high |= (base >> 32) & 0xFFFF_FFFF;

    (low, high)
}

#[inline]
fn read_rsp() -> u64 {
    let rsp: u64;
    // SAFETY:
    // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
    // - Reading `rsp` into a general-purpose register is side-effect free.
    // - This runs in ring 0 on x86_64.
    unsafe {
        asm!("mov {}, rsp", out(reg) rsp, options(nomem, nostack, preserves_flags));
    }
    rsp
}

/// Initializes and loads the kernel GDT/TSS.
///
/// This function is idempotent and can be called multiple times safely.
///
/// Initialization contract:
/// - build an internally consistent GDT image in memory
/// - publish a TSS whose `rsp0` points to the current kernel stack
/// - switch GDTR to this GDT and load TR from the TSS descriptor
pub fn init() {
    let current_rsp = read_rsp();

    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `STATE` is a process-wide singleton used during boot sequencing.
    // - We build a complete valid descriptor table before loading it.
    // - `gdt_flush_and_reload` executes privileged instructions expected in ring 0.
    // - `DOUBLE_FAULT_IST_STACK` is static storage dedicated to IST1.
    unsafe {
        let gdt = &mut *STATE.gdt.get();
        let tss = &mut *STATE.tss.get();

        *gdt = [0; GDT_ENTRY_COUNT];
        *tss = TaskStateSegment::new();

        // Seed RSP0 with the currently executing kernel stack.
        // Later, the scheduler can update this per selected user task.
        tss.rsp0 = current_rsp;

        // Route IST1 to a dedicated emergency stack used by double-fault handling.
        // The CPU expects IST pointers to reference the top of the stack.
        let ist1_base = core::ptr::addr_of!(DOUBLE_FAULT_IST_STACK.0) as u64;
        tss._ist1 = ist1_base + IST_STACK_SIZE as u64;

        // Disable I/O bitmap by placing the map offset beyond TSS size.
        // (No per-port permission bitmap is active.)
        tss.io_map_base = size_of::<TaskStateSegment>() as u16;

        // Kernel Code Segment
        gdt[KERNEL_CODE_INDEX as usize] = build_code_or_data_descriptor(
            ACCESS_PRESENT | ACCESS_SEGMENT | ACCESS_EXECUTABLE | ACCESS_RW,
            FLAGS_LONG_MODE,
        );

        // Kernel Data Segment
        gdt[KERNEL_DATA_INDEX as usize] =
            build_code_or_data_descriptor(ACCESS_PRESENT | ACCESS_SEGMENT | ACCESS_RW, 0);

        // User Code Segment
        gdt[USER_CODE_INDEX as usize] = build_code_or_data_descriptor(
            ACCESS_PRESENT | ACCESS_RING3 | ACCESS_SEGMENT | ACCESS_EXECUTABLE | ACCESS_RW,
            FLAGS_LONG_MODE,
        );

        // User Data Segment
        gdt[USER_DATA_INDEX as usize] = build_code_or_data_descriptor(
            ACCESS_PRESENT | ACCESS_RING3 | ACCESS_SEGMENT | ACCESS_RW,
            0,
        );

        // TSS descriptor occupies two consecutive GDT slots.
        let tss_base = tss as *const TaskStateSegment as u64;
        let tss_limit = (size_of::<TaskStateSegment>() - 1) as u32;
        let (tss_low, tss_high) = build_tss_descriptor(tss_base, tss_limit);
        gdt[TSS_INDEX as usize] = tss_low;
        gdt[TSS_INDEX as usize + 1] = tss_high;

        let ptr = DescriptorTablePointer {
            limit: (size_of::<u64>() * GDT_ENTRY_COUNT - 1) as u16,
            base: gdt.as_ptr() as u64,
        };

        // Load GDT + data selectors and activate TSS via LTR.
        // CS reload is still deferred to a dedicated bring-up step.
        gdt_flush_and_reload(
            &ptr,
            KERNEL_DATA_SELECTOR,
            KERNEL_CODE_SELECTOR,
            TSS_SELECTOR,
        );
    }

    INITIALIZED.store(true, Ordering::Release);
}

/// Returns whether GDT/TSS initialization has completed.
#[cfg_attr(not(test), allow(dead_code))]
pub fn is_initialized() -> bool {
    INITIALIZED.load(Ordering::Acquire)
}

/// Updates `RSP0` in the loaded TSS for future ring-3 to ring-0 transitions.
pub fn set_kernel_rsp0(rsp0: u64) {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - `STATE.tss` is the singleton active TSS for this CPU.
    // - Updating `rsp0` is an atomic 64-bit store on x86_64 and sufficient for this kernel.
    unsafe {
        (*STATE.tss.get()).rsp0 = rsp0;
    }
}

/// Returns the current `RSP0` value stored in the TSS.
#[cfg_attr(not(test), allow(dead_code))]
pub fn kernel_rsp0() -> u64 {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Reading from the singleton TSS is safe; callers get a plain value copy.
    unsafe { (*STATE.tss.get()).rsp0 }
}

/// Returns the current IST1 pointer stored in the TSS.
#[cfg_attr(not(test), allow(dead_code))]
pub fn kernel_ist1() -> u64 {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Reading from the singleton TSS returns a plain value copy.
    unsafe { (*STATE.tss.get())._ist1 }
}

/// Returns a snapshot copy of the active GDT entries.
#[cfg_attr(not(test), allow(dead_code))]
pub fn descriptor_snapshot() -> [u64; GDT_ENTRY_COUNT] {
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Reading the table into a by-value array copy does not create aliasing issues.
    unsafe { *STATE.gdt.get() }
}
