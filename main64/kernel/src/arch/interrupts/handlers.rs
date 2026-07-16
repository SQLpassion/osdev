//! Interrupt and exception handler entry points.

use core::arch::asm;
use core::mem::size_of;

use crate::arch::interrupts::dispatch_irq;
use crate::arch::interrupts::types::{InterruptStackFrame, SavedRegisters, IRQ_BASE};

const VGA_TEXT_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;

/// White-on-red attribute byte used for fatal exception banners.
const VGA_ATTR_FATAL: u8 = 0x4F;

/// Handles `#PF` (Page Fault, vector 14) exceptions.
///
/// Ring-3 faults may return only after the VMM grows the bounded user stack.
/// Every other Ring-3 fault terminates that task and selects a replacement
/// frame. Ring-0 faults retain the existing fatal VMM policy.
///
/// # Safety
///
/// - Must only be entered from `isr14_page_fault_stub` with interrupts disabled.
/// - `frame` must point to the complete register-save area created by that stub.
/// - The CPU error code and IRET frame must immediately follow `SavedRegisters`.
#[no_mangle]
pub unsafe extern "C" fn page_fault_handler_rust(
    frame: *mut SavedRegisters,
    faulting_address: u64,
    error_code: u64,
) -> *mut SavedRegisters {
    // Step 1: #PF always has a CPU-pushed error code, so locate the IRET frame
    // after that word and use its CS selector to determine the fault origin.
    let iret_ptr = (frame as usize) + size_of::<SavedRegisters>() + size_of::<u64>();
    let iret_frame = unsafe {
        // SAFETY:
        // - `frame` is supplied by the dedicated #PF assembly stub.
        // - That stub saves exactly one `SavedRegisters` block.
        // - #PF always pushes one error-code word before the CPU IRET frame.
        &*(iret_ptr as *const InterruptStackFrame)
    };

    // Step 2: Kernel faults remain fatal. The existing VMM handler may resolve
    // trusted kernel demand mappings, but preserves its panic path on failure.
    if !exception_originated_from_user_mode(iret_frame.cs) {
        crate::memory::vmm::handle_page_fault(faulting_address, error_code);
        return frame;
    }

    // Step 3: Resume a user task only for bounded, non-present stack growth.
    // Code, heap, guard-page, permission, reserved-bit, and OOM faults all
    // fall through to task termination below.
    if crate::memory::vmm::try_handle_user_page_fault(faulting_address, error_code).is_ok() {
        return frame;
    }

    // Step 4: A rejected Ring-3 fault is isolated to its process. The zombie
    // reaper delays freeing its address space and stack until this ISR no
    // longer references the old frame.
    let diagnostic = format_args!(
        "USER EXCEPTION #PF: terminating task at rip=0x{:016x} cr2=0x{:016x} err=0x{:016x} cs=0x{:04x}\n",
        iret_frame.rip, faulting_address, error_code, iret_frame.cs
    );
    crate::drivers::serial::_debug_print(diagnostic);
    crate::console::with_console(|console| {
        let _ = console.write_fmt(diagnostic);
    });
    crate::drivers::keyboard::clear_buffers();
    crate::scheduler::mark_current_as_zombie();

    // Step 5: Select the replacement while IF is clear. The assembly stub
    // restores its returned frame and lets `iretq` restore its saved RFLAGS.
    crate::arch::interrupts::disable();
    crate::scheduler::on_timer_tick(frame)
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

/// Returns whether an exception return frame targets Ring 3.
///
/// The low two bits of `CS` contain the requested privilege level (RPL).
/// Only RPL 3 is user mode; Ring 0 exceptions remain kernel-fatal.
#[inline]
pub const fn exception_originated_from_user_mode(cs: u64) -> bool {
    cs & 0b11 == 0b11
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

/// Handles `#UD` (Invalid Opcode, vector 6) exceptions.
///
/// Ring-3 invalid opcodes terminate only the faulting task. The returned frame
/// may belong to another task and is restored by `isr6_invalid_opcode_stub`.
/// Ring-0 invalid opcodes remain fatal and are delegated to the common fatal
/// exception sink.
///
/// # Safety
///
/// - Must only be entered from `isr6_invalid_opcode_stub` with interrupts disabled.
/// - `frame` must point to the complete register-save area created by that stub.
/// - A hardware IRET frame without an exception error code must immediately
///   follow `SavedRegisters`.
#[no_mangle]
pub unsafe extern "C" fn invalid_opcode_handler_rust(
    frame: *mut SavedRegisters,
) -> *mut SavedRegisters {
    // Step 1: Locate the CPU-pushed return frame. #UD does not carry an error
    // code, so it starts immediately after the saved general-purpose registers.
    let iret_ptr = (frame as usize) + size_of::<SavedRegisters>();
    let iret_frame = unsafe {
        // SAFETY:
        // - `frame` is provided by the dedicated #UD assembly stub.
        // - The stub saves exactly one `SavedRegisters` block before this call.
        // - #UD does not push an exception error code, so `iret_ptr` points at
        //   the CPU-pushed `InterruptStackFrame`.
        &*(iret_ptr as *const InterruptStackFrame)
    };

    // Step 2: Preserve the existing fatal behavior for invalid opcodes raised
    // while executing kernel code. Continuing could hide corrupted control flow.
    if !exception_originated_from_user_mode(iret_frame.cs) {
        exception_handler_rust(
            crate::arch::interrupts::types::EXCEPTION_INVALID_OPCODE,
            0,
            frame,
        );
    }

    // Step 3: All user-triggerable fatal exceptions use one termination path so
    // logging, input cleanup, deferred reaping, and frame switching stay equal.
    terminate_user_exception(frame, "#UD", 0, iret_frame)
}

/// Handles `#DE` (Divide Error, vector 0) raised by a user task.
///
/// A divide-by-zero or quotient-overflow fault cannot be repaired by retrying
/// the instruction, so the faulting task is terminated. Kernel-originated
/// faults retain the fatal exception policy.
///
/// # Safety
///
/// - Must only be entered from `isr0_divide_by_zero_stub` with interrupts disabled.
/// - `frame` must point to the complete register-save area created by that stub.
/// - The hardware IRET frame without an error code must follow `SavedRegisters`.
#[no_mangle]
pub unsafe extern "C" fn divide_error_handler_rust(
    frame: *mut SavedRegisters,
) -> *mut SavedRegisters {
    // Step 1: #DE has no CPU-pushed error code, so the IRET frame follows the
    // saved registers directly.
    let iret_ptr = (frame as usize) + size_of::<SavedRegisters>();
    let iret_frame = unsafe {
        // SAFETY:
        // - `frame` is supplied by the dedicated #DE assembly stub.
        // - The stub saves exactly one `SavedRegisters` block.
        // - #DE does not push an exception error code.
        &*(iret_ptr as *const InterruptStackFrame)
    };

    // Step 2: A kernel divide error indicates a kernel bug and must remain
    // fatal instead of being mistaken for a recoverable process fault.
    if !exception_originated_from_user_mode(iret_frame.cs) {
        exception_handler_rust(
            crate::arch::interrupts::types::EXCEPTION_DIVIDE_ERROR,
            0,
            frame,
        );
    }

    // Step 3: Isolate the unrecoverable arithmetic fault to this task.
    terminate_user_exception(frame, "#DE", 0, iret_frame)
}

/// Handles `#GP` (General Protection Fault, vector 13) raised by a user task.
///
/// User-mode protection violations terminate only the offending task. The
/// error code is retained in the diagnostic because it identifies the failing
/// selector or descriptor access. Kernel-originated faults remain fatal.
///
/// # Safety
///
/// - Must only be entered from `isr13_general_protection_fault_stub` with interrupts disabled.
/// - `frame` must point to the complete register-save area created by that stub.
/// - The CPU error code and IRET frame must follow `SavedRegisters`.
#[no_mangle]
pub unsafe extern "C" fn general_protection_fault_handler_rust(
    frame: *mut SavedRegisters,
    error_code: u64,
) -> *mut SavedRegisters {
    // Step 1: #GP pushes one error-code word before the hardware IRET frame.
    let iret_ptr = (frame as usize) + size_of::<SavedRegisters>() + size_of::<u64>();
    let iret_frame = unsafe {
        // SAFETY:
        // - `frame` is supplied by the dedicated #GP assembly stub.
        // - The stub saves exactly one `SavedRegisters` block.
        // - #GP always pushes one exception error-code word.
        &*(iret_ptr as *const InterruptStackFrame)
    };

    // Step 2: A kernel protection fault is never safe to resume.
    if !exception_originated_from_user_mode(iret_frame.cs) {
        exception_handler_rust(
            crate::arch::interrupts::types::EXCEPTION_GENERAL_PROTECTION,
            error_code,
            frame,
        );
    }

    // Step 3: Terminate only the user task that violated a protection rule.
    terminate_user_exception(frame, "#GP", error_code, iret_frame)
}

/// Terminates a user task after an unrecoverable synchronous CPU exception.
///
/// The current stack and address space remain owned by the scheduler until a
/// later reaper pass, because the exception stub still needs the old frame
/// while it switches to the returned task frame.
fn terminate_user_exception(
    frame: *mut SavedRegisters,
    name: &str,
    error_code: u64,
    iret_frame: &InterruptStackFrame,
) -> *mut SavedRegisters {
    // Step 1: Emit a compact diagnostic before changing task ownership.
    let diagnostic = format_args!(
        "USER EXCEPTION {}: terminating task at rip=0x{:016x} err=0x{:016x} cs=0x{:04x}\n",
        name, iret_frame.rip, error_code, iret_frame.cs
    );
    crate::drivers::serial::_debug_print(diagnostic);

    // Step 2: The user task cannot hold the console lock while executing the
    // exception, so the same diagnostic can safely reach the visible console.
    crate::console::with_console(|console| {
        let _ = console.write_fmt(diagnostic);
    });

    // Step 3: Remove input consumed by the dying task so the next task cannot
    // accidentally interpret its test/menu key as new input.
    crate::drivers::keyboard::clear_buffers();

    // Step 4: Defer stack/address-space reclamation until execution has moved
    // off the faulting exception stack.
    crate::scheduler::mark_current_as_zombie();

    // Step 5: Select a replacement with interrupts disabled; `iretq` restores
    // that task's saved RFLAGS and re-enables interrupts as appropriate.
    crate::arch::interrupts::disable();
    crate::scheduler::on_timer_tick(frame)
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
        // Disable interrupts before entering the scheduler and keep them
        // disabled until `iretq`.  The stub executed `sti` after saving the
        // GPRs, so without this `cli` the `SCHED` guard inside `on_timer_tick`
        // would re-enable interrupts on drop — after the scheduler has already
        // committed to the next task (running_slot/CR3/RSP0 switched, its
        // frame pointer about to be returned) but before the stub's own `cli`.
        // A timer IRQ in that window would consume the selected task's frame
        // and leave this task's saved `rax` pointing at a stale frame,
        // corrupting the resume path.  The invariant is: IF stays 0 from frame
        // selection until `iretq` — exactly what the hardware IRQ path already
        // guarantees via its interrupt gate.  `iretq` restores the task's
        // RFLAGS image (IF=1), so user-mode interrupt delivery is unaffected.
        crate::arch::interrupts::disable();
        return crate::scheduler::on_timer_tick(frame as *mut SavedRegisters);
    }

    frame as *mut SavedRegisters
}
