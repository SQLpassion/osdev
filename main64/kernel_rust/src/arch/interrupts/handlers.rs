//! Interrupt and exception handler entry points.

use core::arch::asm;
use core::mem::size_of;

use crate::arch::interrupts::types::{IRQ_BASE, InterruptStackFrame, SavedRegisters};
use crate::arch::interrupts::dispatch_irq;

const VGA_TEXT_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;

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

fn write_exception_banner(vector: u8, error_code: u64, frame: *const SavedRegisters) {
    let mut line = [b' '; VGA_COLS];
    line[0] = b'!';
    line[1] = b'!';
    line[2] = b' ';
    line[3] = b'E';
    line[4] = b'X';
    line[5] = b'C';
    line[6] = b' ';
    line[7] = b'v';
    line[8] = b'e';
    line[9] = b'c';
    line[10] = b'=';
    line[11] = hex_nibble_ascii((vector >> 4) & 0x0F);
    line[12] = hex_nibble_ascii(vector & 0x0F);
    line[13] = b' ';
    line[14] = b'e';
    line[15] = b'r';
    line[16] = b'r';
    line[17] = b'=';
    for i in 0..16 {
        let shift = (15 - i) * 4;
        line[18 + i] = hex_nibble_ascii(((error_code >> shift) & 0x0F) as u8);
    }
    line[34] = b' ';
    line[35] = b'f';
    line[36] = b'r';
    line[37] = b'm';
    line[38] = b'=';
    let frame_u64 = frame as u64;
    for i in 0..16 {
        let shift = (15 - i) * 4;
        line[39 + i] = hex_nibble_ascii(((frame_u64 >> shift) & 0x0F) as u8);
    }

    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - VGA text memory is MMIO-mapped at `VGA_TEXT_BUFFER`.
    // - We only write one in-bounds row (0..80 cells).
    // - Volatile writes are required for MMIO ordering/visibility.
    unsafe {
        for (col, ch) in line.iter().enumerate() {
            let cell = VGA_TEXT_BUFFER + col * 2;
            core::ptr::write_volatile(cell as *mut u8, *ch);
            core::ptr::write_volatile((cell + 1) as *mut u8, 0x4F);
        }
    }
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
    write_exception_banner(vector, error_code, frame);

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
