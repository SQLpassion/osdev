//! FPU/SSE state management for lazy context switching.
//!
//! Rust's LLVM backend emits SSE2 instructions for general operations such as
//! `memcpy`, struct copies, and string operations — even in code that does not
//! contain explicit floating-point arithmetic.  Without FPU state preservation
//! across context switches, XMM registers are silently corrupted whenever two
//! tasks run concurrently.
//!
//! This module implements **lazy FPU switching** using the hardware-provided
//! `CR0.TS` mechanism:
//!
//! 1. On every context switch `select_next_task` sets `CR0.TS`.
//! 2. The first FPU/SSE instruction executed by the new task raises `#NM`
//!    (Device Not Available, vector 7).
//! 3. The `#NM` handler (`nm_rust_handler`) restores the task's saved FPU
//!    state via `FXRSTOR64`, clears `CR0.TS`, and returns.  The faulting
//!    instruction is re-executed successfully.
//! 4. For tasks that never use FPU/SSE the `#NM` trap never fires — zero
//!    overhead for pure integer workloads.
//!
//! `FXSAVE64` / `FXRSTOR64` are used instead of `XSAVE`/`XRSTOR` because
//! they cover everything Rust/LLVM emits (x87 + MMX + XMM0–XMM15) with a
//! fixed 512-byte layout, eliminating the need for CPUID feature detection.

use core::arch::asm;
use core::cell::UnsafeCell;

extern crate alloc;
use alloc::alloc as heap_alloc;
use core::alloc::Layout;

/// 512-byte buffer for `FXSAVE64` / `FXRSTOR64`.
///
/// The hardware mandates 16-byte alignment.  `#[repr(C, align(16))]` ensures
/// that Rust's global allocator satisfies this requirement when the type is
/// heap-allocated (via [`FpuState::allocate_default`]).
#[repr(C, align(16))]
pub struct FpuState(pub [u8; 512]);

/// Wraps the global `FpuState` template so it can be stored in a `static`
/// without requiring `static mut` (which would require a mutable reference).
struct FpuStateTemplate(UnsafeCell<FpuState>);

// SAFETY:
// - `DEFAULT_FPU_STATE` is written exactly once inside `init()`, which runs
//   before any task is spawned and before interrupts are enabled.
// - After `init()` the template is read-only from `allocate_default()`.
// - Single-core kernel: no concurrent mutation.
unsafe impl Sync for FpuStateTemplate {}

/// Template FXSAVE image captured by [`init`] after `FNINIT`.
///
/// All newly spawned tasks start with a copy of this state:
/// - FCW  = 0x037F (all x87 exceptions masked, round-to-nearest, 64-bit precision)
/// - MXCSR = 0x1F80 (all SSE exceptions masked, round-to-nearest)
static DEFAULT_FPU_STATE: FpuStateTemplate =
    FpuStateTemplate(UnsafeCell::new(FpuState([0u8; 512])));

impl FpuState {
    /// Allocates a heap buffer pre-filled with the default FPU state.
    ///
    /// Must only be called after [`init`] has run (template is populated then).
    ///
    /// Returns a non-null raw pointer on success, null on allocation failure.
    pub fn allocate_default() -> *mut Self {
        // SAFETY:
        // - This requires `unsafe` because unchecked `Layout` construction
        //   bypasses runtime validation of size/alignment constraints.
        // - 512 bytes at 16-byte alignment is a valid, non-zero layout.
        let layout = unsafe { Layout::from_size_align_unchecked(512, 16) };

        // SAFETY:
        // - `layout` has non-zero size and power-of-two alignment.
        let ptr = unsafe { heap_alloc::alloc(layout) } as *mut FpuState;
        if ptr.is_null() {
            return core::ptr::null_mut();
        }

        // Copy the template state captured during init() into the new buffer.
        // SAFETY:
        // - `DEFAULT_FPU_STATE` is initialized by `init()` before any task spawns.
        // - `ptr` is a valid 512-byte, 16-byte-aligned allocation.
        // - Source and destination do not overlap (one is a static, one a fresh heap block).
        unsafe {
            let src = DEFAULT_FPU_STATE.0.get() as *const u8;
            let dst = ptr as *mut u8;
            core::ptr::copy_nonoverlapping(src, dst, 512);
        }

        ptr
    }

    /// Frees a buffer previously returned by [`allocate_default`].
    ///
    /// # Safety
    ///
    /// - `ptr` must have been returned by `allocate_default`.
    /// - Must not be called more than once for the same pointer.
    /// - `ptr` must not be dereferenced after this call.
    pub unsafe fn deallocate(ptr: *mut Self) {
        if ptr.is_null() {
            return;
        }

        // SAFETY:
        // - Constants match the layout used by `allocate_default`.
        // - Size is non-zero and alignment is a valid power of two.
        let layout = Layout::from_size_align_unchecked(512, 16);
        heap_alloc::dealloc(ptr as *mut u8, layout);
    }

    /// Saves the current CPU FPU/SSE state into this buffer via `FXSAVE64`.
    ///
    /// # Safety
    ///
    /// - The buffer must be 16-byte aligned (guaranteed by `#[repr(align(16))]`).
    /// - `CR0.EM` must be 0 (FPU emulation disabled); otherwise `#UD` is raised.
    /// - `FXSAVE64` does **not** check `CR0.TS`, so this is safe to call even
    ///   when `CR0.TS = 1`.
    #[inline]
    pub unsafe fn save(&mut self) {
        // SAFETY:
        // - This requires `unsafe` because inline assembly and raw memory access
        //   are outside Rust's static safety model.
        // - `self.0.as_mut_ptr()` is a valid, 16-byte-aligned pointer to a
        //   512-byte buffer, satisfying FXSAVE64's memory operand requirement.
        asm!(
            "fxsave64 [{ptr}]",
            ptr = in(reg) self.0.as_mut_ptr(),
            options(nostack),
        );
    }

    /// Restores CPU FPU/SSE state from this buffer via `FXRSTOR64`.
    ///
    /// # Safety
    ///
    /// - The buffer must be 16-byte aligned (guaranteed by `#[repr(align(16))]`).
    /// - `CR0.TS` must be 0 before calling; `FXRSTOR64` raises `#NM` when
    ///   `CR0.TS = 1`.  Call [`clear_ts`] first if necessary.
    /// - The buffer must contain a valid `FXSAVE64` image (written by a prior
    ///   [`save`] or initialised by [`allocate_default`]).
    #[inline]
    pub unsafe fn restore(&self) {
        // SAFETY:
        // - This requires `unsafe` because inline assembly and raw memory access
        //   are outside Rust's static safety model.
        // - `self.0.as_ptr()` is a valid, 16-byte-aligned pointer to a 512-byte
        //   buffer holding a well-formed FXSAVE64 image.
        asm!(
            "fxrstor64 [{ptr}]",
            ptr = in(reg) self.0.as_ptr(),
            options(nostack),
        );
    }
}

/// Initialises the FPU subsystem and captures the default FPU state template.
///
/// Must be called exactly once, after GDT initialisation and before the IDT
/// is loaded.  The call order within this function is architecturally required:
///
/// 1. Clear `CR0.EM` — prevents `#UD` on FPU/SSE instructions.
/// 2. Set `CR0.MP`  — `WAIT`/`FWAIT` honour `CR0.TS`.
/// 3. Set `CR4.OSFXSR`   — enables `FXSAVE`/`FXRSTOR`.
/// 4. Set `CR4.OSXMMEXCPT` — allows `#XM` (SSE exception) to be signalled.
/// 5. `FNINIT`     — resets FPU to well-defined default state.
/// 6. `LDMXCSR`   — sets MXCSR to 0x1F80 (all SSE exceptions masked).
/// 7. `FXSAVE64`  — snapshots the reset state into `DEFAULT_FPU_STATE`.
pub fn init() {
    // SAFETY:
    // - This requires `unsafe` because it accesses privileged CPU control
    //   registers and executes FPU initialization instructions.
    // - Called exactly once at boot before interrupts and tasks are active.
    // - CR0/CR4 modifications affect all subsequent FPU/SSE behavior on this CPU.
    unsafe {
        // Step 1 & 2: Configure CR0.
        let mut cr0: u64;
        asm!("mov {r}, cr0", r = out(reg) cr0, options(nomem, nostack, preserves_flags));
        cr0 &= !(1u64 << 2); // clear EM (bit 2)
        cr0 |= 1u64 << 1; // set MP (bit 1)
        asm!("mov cr0, {r}", r = in(reg) cr0, options(nomem, nostack, preserves_flags));

        // Step 3 & 4: Configure CR4.
        let mut cr4: u64;
        asm!("mov {r}, cr4", r = out(reg) cr4, options(nomem, nostack, preserves_flags));
        cr4 |= (1u64 << 9) | (1u64 << 10); // set OSFXSR (bit 9) + OSXMMEXCPT (bit 10)
        asm!("mov cr4, {r}", r = in(reg) cr4, options(nomem, nostack, preserves_flags));

        // Step 5: Reset x87 FPU to default state.
        // FCW = 0x037F (all exceptions masked, round-to-nearest, 64-bit precision).
        // MXCSR is unaffected by FNINIT.
        asm!("fninit", options(nomem, nostack));

        // Step 6: Reset MXCSR to 0x1F80 (all SSE exceptions masked, round-to-nearest).
        let mxcsr: u32 = 0x1F80;
        asm!("ldmxcsr [{ptr}]", ptr = in(reg) &mxcsr, options(nostack));

        // Step 7: Capture the initialised state as the template for new tasks.
        let template = &mut *DEFAULT_FPU_STATE.0.get();
        template.save();
    }
}

/// Sets `CR0.TS` (Task Switched bit).
///
/// After this call the next FPU/SSE instruction executed by any task will
/// raise `#NM` (Device Not Available, vector 7).
///
/// # Safety
///
/// Must be called from ring 0 (kernel mode).
#[inline]
pub unsafe fn set_ts() {
    // SAFETY:
    // - This requires `unsafe` because it modifies a privileged CPU control
    //   register via inline assembly.
    // - Setting CR0.TS causes subsequent FPU/SSE instructions to raise #NM.
    // - Valid only in ring 0.
    asm!(
        "mov {r}, cr0",
        "or {r}, {bit}",
        "mov cr0, {r}",
        r = lateout(reg) _,
        bit = const 1u64 << 3,
        options(nomem, nostack, preserves_flags),
    );
}

/// Clears `CR0.TS` (Task Switched bit) using the `CLTS` instruction.
///
/// Allows FPU/SSE instructions to execute again without triggering `#NM`.
///
/// # Safety
///
/// Must be called from ring 0 (kernel mode).
#[inline]
pub unsafe fn clear_ts() {
    // SAFETY:
    // - This requires `unsafe` because it executes a privileged CPU instruction
    //   via inline assembly.
    // - `CLTS` is valid only in ring 0 and clears CR0.TS atomically.
    asm!("clts", options(nomem, nostack, preserves_flags));
}
