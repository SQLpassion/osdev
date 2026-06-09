//! Interrupt and exception handler entry points.

use core::arch::asm;
use core::mem::size_of;

use crate::arch::interrupts::types::{IRQ_BASE, InterruptStackFrame, SavedRegisters};
use crate::arch::interrupts::dispatch_irq;

const VGA_TEXT_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;

/// White-on-red attribute byte used for fatal exception banners.
const VGA_ATTR_FATAL: u8 = 0x4F;

#[no_mangle]
pub extern "C" fn page_fault_handler_rust(faulting_address: u64, error_code: u64) {
    crate::memory::vmm::handle_page_fault(faulting_address, error_code);
}

/// Entry point for the `#NM` (Device Not Available, vector 7) exception.
///
/// Called by `isr7_nm_stub` after all GPRs have been saved on the kernel stack.
/// Delegates to the scheduler's FPU trap handler which clears `CR0.TS` and
/// restores the current task's FPU state via `FXRSTOR64`.
///
/// After this returns the stub re-executes `iretq`, which re-runs the faulting
/// FPU/SSE instruction — this time successfully because `CR0.TS` is clear.
///
/// # Safety
///
/// Must only be entered from `isr7_nm_stub`.  Interrupts are disabled on entry
/// (the stub executes `cli`).
#[no_mangle]
pub extern "C" fn nm_rust_handler() {
    crate::scheduler::handle_fpu_trap();
}

/// Returns whether a CPU exception vector pushes an error code on entry.
pub const fn exception_has_error_code(vector: u8) -> bool {
    matches!(vector, 8 | 10 | 11 | 12 | 13 | 14 | 17 | 21 | 29 | 30)
}

#[inline]
const fn hex_nibble_ascii(nibble: u8) -> u8 {
    if nibble < 10 {
        b'0' + nibble
    } else {
        b'a' + (nibble - 10)
    }
}

/// Writes `key=hex(value)` into `buf` starting at `offset` and returns the
/// position right after the written hex digits.
///
/// `nibbles` controls the zero-padded hex width (2 for a byte, 4 for u16,
/// 16 for u64).  No bounds checking is performed; callers must ensure the
/// destination range fits inside `buf`.
fn write_field(buf: &mut [u8], offset: usize, key: &[u8], value: u64, nibbles: usize) -> usize {
    let mut pos = offset;
    for &c in key {
        buf[pos] = c;
        pos += 1;
    }
    buf[pos] = b'=';
    pos += 1;
    for i in 0..nibbles {
        let shift = (nibbles - 1 - i) * 4;
        buf[pos + i] = hex_nibble_ascii(((value >> shift) & 0x0F) as u8);
    }
    pos + nibbles
}

/// Writes one 80-cell VGA row using the fatal-banner color attribute.
fn write_vga_row(row: usize, line: &[u8; VGA_COLS]) {
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - VGA text memory is MMIO-mapped at `VGA_TEXT_BUFFER`.
    // - We only write one in-bounds row (0..80 cells) at the given `row`.
    // - Volatile writes are required for MMIO ordering/visibility.
    unsafe {
        let row_base = VGA_TEXT_BUFFER + row * VGA_COLS * 2;
        for (col, ch) in line.iter().enumerate() {
            let cell = row_base + col * 2;
            core::ptr::write_volatile(cell as *mut u8, *ch);
            core::ptr::write_volatile((cell + 1) as *mut u8, VGA_ATTR_FATAL);
        }
    }
}

fn write_exception_banner(
    vector: u8,
    error_code: u64,
    frame: *const SavedRegisters,
    iret: &InterruptStackFrame,
) {
    // Row 0: "!! EXC vec=XX err=...16... frm=...16... rip=...16..." (76 cols)
    let mut line0 = [b' '; VGA_COLS];
    line0[0..7].copy_from_slice(b"!! EXC ");
    let mut p = write_field(&mut line0, 7, b"vec", vector as u64, 2);
    p = write_field(&mut line0, p + 1, b"err", error_code, 16);
    p = write_field(&mut line0, p + 1, b"frm", frame as u64, 16);
    let _ = write_field(&mut line0, p + 1, b"rip", iret.rip, 16);
    write_vga_row(0, &line0);

    // Row 1: "   cs=XXXX rflags=...16... rsp=...16... ss=XXXX" (63 cols)
    let mut line1 = [b' '; VGA_COLS];
    let mut p = write_field(&mut line1, 3, b"cs", iret.cs, 4);
    p = write_field(&mut line1, p + 1, b"rflags", iret.rflags, 16);
    p = write_field(&mut line1, p + 1, b"rsp", iret.rsp, 16);
    let _ = write_field(&mut line1, p + 1, b"ss", iret.ss, 4);
    write_vga_row(1, &line1);
}

/// Fatal exception sink for vectors with dedicated stubs.
///
/// Called from assembly stubs for faults we currently treat as unrecoverable.
#[no_mangle]
pub extern "C" fn exception_handler_rust(
    vector: u8,
    error_code: u64,
    frame: *const SavedRegisters,
) -> ! {
    let has_error_code = exception_has_error_code(vector);
    let iret_ptr = (frame as usize)
        + size_of::<SavedRegisters>()
        + if has_error_code { size_of::<u64>() } else { 0 };
    let iret_frame = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it casts and dereferences a raw frame pointer.
        // - `frame` points at the register-save area pushed by the ISR stub.
        // - The CPU-pushed interrupt return frame immediately follows saved regs
        //   (plus optional error code for vectors that carry one).
        &*(iret_ptr as *const InterruptStackFrame)
    };
    crate::drivers::serial::_debug_print(format_args!(
        "FATAL EXCEPTION vec=0x{:02x} has_err={} err=0x{:016x} frame=0x{:016x} rip=0x{:016x} cs=0x{:016x} rflags=0x{:016x}\n",
        vector,
        has_error_code,
        error_code,
        frame as u64,
        iret_frame.rip,
        iret_frame.cs,
        iret_frame.rflags
    ));
    write_exception_banner(vector, error_code, frame, iret_frame);

    loop {
        // SAFETY:
        // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
        // - We are in a fatal exception path and intentionally stop forward progress.
        // - `cli; hlt` is the standard terminal halt sequence for kernel panic/fault sinks.
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack, preserves_flags));
        }
    }
}

/// Dispatch entry point called from the IRQ assembly trampoline.
///
/// # Safety
/// - Must be called with interrupts disabled (`cli` before entry).
/// - Must not be called reentrantly — the assembly stub does not
///   re-enable interrupts until after `iretq`.
/// - `vector` must be a valid IRQ vector number (`IRQ_BASE..IRQ_BASE + 16`).
#[no_mangle]
pub unsafe extern "C" fn irq_rust_dispatch(
    vector: u8,
    frame: *mut SavedRegisters,
) -> *mut SavedRegisters {
    if !(IRQ_BASE..IRQ_BASE + 16).contains(&vector) {
        return frame;
    }

    let frame = unsafe {
        // SAFETY:
        // - This requires `unsafe` because it reinterprets a raw pointer as a mutable reference.
        // - `frame` is provided by the IRQ assembly stubs and points to the
        //   register save area currently living on the active kernel stack.
        // - It remains valid until the stub restores registers and executes `iretq`.
        &mut *frame
    };

    dispatch_irq(vector, frame)
}

/// Dispatch entry point for software interrupt `int 0x80`.
///
/// # Safety
/// - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
/// - Must be entered only from the dedicated `int80_syscall_stub`.
/// - `frame` must point to the saved-register frame on the active kernel stack.
#[no_mangle]
pub unsafe extern "C" fn syscall_rust_dispatch(frame: *mut SavedRegisters) -> *mut SavedRegisters {
    let frame = unsafe {
        // SAFETY:
        // - `frame` is provided by the syscall assembly stub.
        // - It points at a live register-save area until the stub restores regs and returns via `iretq`.
        &mut *frame
    };

    let syscall_nr = frame.rax;
    let arg0 = frame.rdi;
    let arg1 = frame.rsi;
    let arg2 = frame.rdx;
    let arg3 = frame.r10;
    let result = crate::syscall::dispatch_checked(syscall_nr, arg0, arg1, arg2, arg3);
    frame.rax = crate::syscall::syscall_result_to_raw(result);

    // Yield and Exit both need an immediate reschedule via the scheduler.
    //
    // Instead of triggering a nested software interrupt (`int 32` inside
    // `int 0x80`), we call `on_timer_tick` directly with the current
    // interrupt frame.  The `int80_syscall_stub` restores whatever frame
    // pointer we return here, so returning a different task's frame
    // performs a seamless context switch with a single `iretq`.
    //
    // - **Yield**: the current task stays Ready and will be picked up
    //   again in a future round-robin cycle.
    // - **Exit**: `dispatch` has already marked the task as Zombie.
    //   `on_timer_tick` will never select it again, and `reap_zombies`
    //   reclaims its slot on the next scheduler tick once execution has
    //   safely moved off its kernel stack.
    if syscall_nr == crate::syscall::SyscallId::Yield as u64
        || syscall_nr == crate::syscall::SyscallId::Exit as u64
    {
        return crate::scheduler::on_timer_tick(frame as *mut SavedRegisters);
    }

    frame as *mut SavedRegisters
}
