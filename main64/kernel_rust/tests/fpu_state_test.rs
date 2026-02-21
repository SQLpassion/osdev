//! FPU state management integration tests.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::arch::asm;
use core::panic::PanicInfo;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use kaos_kernel::arch::{fpu, gdt, interrupts};
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::scheduler as sched;

const CR0_TS_BIT: u64 = 1u64 << 3;
const MXCSR_DEFAULT: u32 = 0x1F80;
const MXCSR_TEST_VALUE: u32 = 0x1F81;
const PYTHAGORAS_EXPECTED_BITS: u64 = 5.0f64.to_bits();
const PYTHAGORAS_TASK_A_EXPECTED_BITS: u64 = 5.0f64.to_bits();
const PYTHAGORAS_TASK_B_EXPECTED_BITS: u64 = 13.0f64.to_bits();
const FPU_MULTI_TASK_ITERATIONS: usize = 64;

static FPU_TASK_DONE: AtomicBool = AtomicBool::new(false);
static FPU_TASK_RESULT_BITS: AtomicU64 = AtomicU64::new(0);
static FPU_MULTI_TASK_DONE_A: AtomicBool = AtomicBool::new(false);
static FPU_MULTI_TASK_DONE_B: AtomicBool = AtomicBool::new(false);
static FPU_MULTI_TASK_RESULT_A_BITS: AtomicU64 = AtomicU64::new(0);
static FPU_MULTI_TASK_RESULT_B_BITS: AtomicU64 = AtomicU64::new(0);

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    gdt::init();
    fpu::init();
    interrupts::init();
    pmm::init(false);
    vmm::init(false);
    heap::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Reads `MXCSR`.
fn read_mxcsr() -> u32 {
    let mut value = 0u32;
    // SAFETY:
    // - This requires `unsafe` because inline assembly is outside Rust's
    //   static safety model.
    // - `value` is a valid writable pointer for `stmxcsr`.
    unsafe {
        asm!("stmxcsr [{ptr}]", ptr = in(reg) &mut value, options(nostack));
    }
    value
}

/// Writes `MXCSR`.
fn write_mxcsr(value: u32) {
    // SAFETY:
    // - This requires `unsafe` because inline assembly is outside Rust's
    //   static safety model.
    // - `value` points to a valid 4-byte memory operand for `ldmxcsr`.
    unsafe {
        asm!("ldmxcsr [{ptr}]", ptr = in(reg) &value, options(nostack));
    }
}

/// Reads `CR0`.
fn read_cr0() -> u64 {
    let value: u64;
    // SAFETY:
    // - This requires `unsafe` because inline assembly with control register
    //   access is privileged and outside Rust's static safety model.
    // - Tests execute in ring 0 in the kernel test environment.
    unsafe {
        asm!(
            "mov {out}, cr0",
            out = out(reg) value,
            options(nomem, nostack, preserves_flags),
        );
    }
    value
}

/// Contract: allocated per-task FPU buffers are non-null and 16-byte aligned.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "allocated per-task FPU buffers are non-null and 16-byte aligned".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_fpu_allocate_default_alignment_contract() {
    let ptr = fpu::FpuState::allocate_default();
    assert!(!ptr.is_null(), "FPU state allocation must succeed");
    assert!(
        (ptr as usize) % 16 == 0,
        "FPU state buffer must be 16-byte aligned for FXSAVE64/FXRSTOR64"
    );

    // SAFETY:
    // - This requires `unsafe` because deallocation accepts a raw pointer.
    // - `ptr` was returned by `allocate_default` and has not been freed yet.
    unsafe {
        fpu::FpuState::deallocate(ptr);
    }
}

/// Contract: save/restore roundtrip restores the previous MXCSR value.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "save/restore roundtrip restores the previous MXCSR value".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_fpu_save_restore_roundtrip_preserves_mxcsr() {
    let mut state = fpu::FpuState([0u8; 512]);

    // Step 1: Establish baseline state and save it to the buffer.
    write_mxcsr(MXCSR_DEFAULT);
    // SAFETY:
    // - This requires `unsafe` because `save` executes privileged inline assembly.
    // - `state` is a properly aligned FXSAVE64 buffer.
    unsafe {
        state.save();
    }

    // Step 2: Mutate MXCSR so we can verify restore has an effect.
    write_mxcsr(MXCSR_TEST_VALUE);
    assert!(
        read_mxcsr() == MXCSR_TEST_VALUE,
        "test precondition failed: MXCSR mutation must be observable"
    );

    // Step 3: Restore the saved image and verify MXCSR returns to baseline.
    // SAFETY:
    // - This requires `unsafe` because `restore` executes privileged inline assembly.
    // - `state` contains a valid FXSAVE64 image written above by `save`.
    unsafe {
        state.restore();
    }
    assert!(
        read_mxcsr() == MXCSR_DEFAULT,
        "FXRSTOR64 must restore MXCSR from the saved state image"
    );
}

/// Contract: bootstrap #NM path clears CR0.TS even when no task is running.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "bootstrap #NM path clears CR0.TS even when no task is running".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_handle_fpu_trap_clears_ts_without_running_task() {
    sched::init();

    // SAFETY:
    // - This requires `unsafe` because `set_ts` writes a privileged control register.
    // - Tests execute in ring 0 in the kernel test environment.
    unsafe {
        fpu::set_ts();
    }
    assert!(
        (read_cr0() & CR0_TS_BIT) != 0,
        "CR0.TS must be set before entering the #NM handler path"
    );

    sched::handle_fpu_trap();

    assert!(
        (read_cr0() & CR0_TS_BIT) == 0,
        "handle_fpu_trap must clear CR0.TS in bootstrap/idle context"
    );
}

/// Contract: executing an SSE instruction with CR0.TS set triggers #NM and then succeeds.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "executing an SSE instruction with CR0.TS set triggers #NM and then succeeds".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_nm_handler_is_reached_by_real_sse_instruction() {
    sched::init();

    // Step 1: Arm lazy-FPU faulting by setting CR0.TS.
    // SAFETY:
    // - This requires `unsafe` because `set_ts` writes a privileged control register.
    // - Tests execute in ring 0 in the kernel test environment.
    unsafe {
        fpu::set_ts();
    }
    assert!(
        (read_cr0() & CR0_TS_BIT) != 0,
        "CR0.TS must be set before issuing the faulting SSE instruction"
    );

    // Step 2: Execute a real SSE/FPU-class instruction.
    // With TS=1 this instruction first raises #NM, the ISR clears TS via
    // `handle_fpu_trap`, then `iretq` retries `ldmxcsr` and it completes.
    write_mxcsr(MXCSR_TEST_VALUE);

    // Step 3: Verify post-conditions of the lazy-restore fault path.
    assert!(
        (read_cr0() & CR0_TS_BIT) == 0,
        "after #NM handling, CR0.TS must be cleared so FPU/SSE instructions can run"
    );
    assert!(
        read_mxcsr() == MXCSR_TEST_VALUE,
        "the retried SSE instruction must complete and update MXCSR"
    );
}

extern "C" fn pythagoras_kernel_task() -> ! {
    let a = 3.0f64;
    let b = 4.0f64;
    let mut c = 0.0f64;

    // Execute a real SSE arithmetic sequence in task context:
    // c = sqrt((a*a) + (b*b)) = 5.0
    // SAFETY:
    // - This requires `unsafe` because inline assembly is outside Rust's
    //   static safety model.
    // - All pointers reference local stack variables that are valid for
    //   8-byte reads/writes during this scope.
    unsafe {
        asm!(
            "movsd xmm0, [{a_ptr}]",
            "mulsd xmm0, xmm0",
            "movsd xmm1, [{b_ptr}]",
            "mulsd xmm1, xmm1",
            "addsd xmm0, xmm1",
            "sqrtsd xmm0, xmm0",
            "movsd [{c_ptr}], xmm0",
            a_ptr = in(reg) &a,
            b_ptr = in(reg) &b,
            c_ptr = in(reg) &mut c,
            options(nostack),
        );
    }

    FPU_TASK_RESULT_BITS.store(c.to_bits(), Ordering::Release);
    FPU_TASK_DONE.store(true, Ordering::Release);

    sched::request_stop();
    sched::yield_now();

    loop {
        core::hint::spin_loop();
    }
}

/// Contract: scheduler executes a kernel task that performs SSE/FPU arithmetic.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "scheduler executes a kernel task that performs SSE/FPU arithmetic".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_scheduler_runs_kernel_task_with_fpu_instructions() {
    FPU_TASK_DONE.store(false, Ordering::Release);
    FPU_TASK_RESULT_BITS.store(0, Ordering::Release);

    sched::init();
    sched::set_kernel_address_space_cr3(vmm::get_pml4_address());
    sched::spawn_kernel_task(pythagoras_kernel_task)
        .expect("pythagoras kernel task spawn should succeed");
    sched::start();

    interrupts::init_periodic_timer(250);
    interrupts::enable();

    let mut observed = false;
    for _ in 0..5_000_000usize {
        if FPU_TASK_DONE.load(Ordering::Acquire) && !sched::is_running() {
            observed = true;
            break;
        }
        core::hint::spin_loop();
    }

    interrupts::disable();

    assert!(
        observed,
        "kernel task with SSE workload did not complete through scheduler execution path"
    );
    assert!(
        FPU_TASK_RESULT_BITS.load(Ordering::Acquire) == PYTHAGORAS_EXPECTED_BITS,
        "pythagoras kernel task must compute sqrt(3^2 + 4^2) = 5.0"
    );
}

extern "C" fn multi_task_fpu_worker_a() -> ! {
    let mut result = 0.0f64;
    let a = 3.0f64;
    let b = 4.0f64;

    // Run repeated SSE arithmetic and force many task switches in between.
    for _ in 0..FPU_MULTI_TASK_ITERATIONS {
        // SAFETY:
        // - This requires `unsafe` because inline assembly is outside Rust's
        //   static safety model.
        // - `a`, `b`, and `result` are valid stack locals for 8-byte memory operands.
        unsafe {
            asm!(
                "movsd xmm0, [{a_ptr}]",
                "mulsd xmm0, xmm0",
                "movsd xmm1, [{b_ptr}]",
                "mulsd xmm1, xmm1",
                "addsd xmm0, xmm1",
                "sqrtsd xmm0, xmm0",
                "movsd [{out_ptr}], xmm0",
                a_ptr = in(reg) &a,
                b_ptr = in(reg) &b,
                out_ptr = in(reg) &mut result,
                options(nostack),
            );
        }
        sched::yield_now();
    }

    FPU_MULTI_TASK_RESULT_A_BITS.store(result.to_bits(), Ordering::Release);
    FPU_MULTI_TASK_DONE_A.store(true, Ordering::Release);
    sched::exit_current_task();
}

extern "C" fn multi_task_fpu_worker_b() -> ! {
    let mut result = 0.0f64;
    let a = 5.0f64;
    let b = 12.0f64;

    // Use different operands than worker A to catch FPU register leakage.
    for _ in 0..FPU_MULTI_TASK_ITERATIONS {
        // SAFETY:
        // - This requires `unsafe` because inline assembly is outside Rust's
        //   static safety model.
        // - `a`, `b`, and `result` are valid stack locals for 8-byte memory operands.
        unsafe {
            asm!(
                "movsd xmm0, [{a_ptr}]",
                "mulsd xmm0, xmm0",
                "movsd xmm1, [{b_ptr}]",
                "mulsd xmm1, xmm1",
                "addsd xmm0, xmm1",
                "sqrtsd xmm0, xmm0",
                "movsd [{out_ptr}], xmm0",
                a_ptr = in(reg) &a,
                b_ptr = in(reg) &b,
                out_ptr = in(reg) &mut result,
                options(nostack),
            );
        }
        sched::yield_now();
    }

    FPU_MULTI_TASK_RESULT_B_BITS.store(result.to_bits(), Ordering::Release);
    FPU_MULTI_TASK_DONE_B.store(true, Ordering::Release);
    sched::exit_current_task();
}

/// Contract: foreground wait over two FPU tasks preserves independent SSE state.
/// Given: The subsystem is initialized with the explicit preconditions in this test body, including any literal addresses, vectors, sizes, flags, and constants used below.
/// When: The exact operation sequence in this function is executed against that state.
/// Then: All assertions must hold for the checked values and state transitions, preserving the contract "foreground wait over two FPU tasks preserves independent SSE state".
/// Failure Impact: Indicates a regression in subsystem behavior, ABI/layout, synchronization, or lifecycle semantics and should be treated as release-blocking until understood.
#[test_case]
fn test_wait_for_task_exit_with_two_fpu_tasks_preserves_results() {
    // Decision note:
    // The worker tasks could compute the same formula using pure Rust f64
    // arithmetic, but this test intentionally uses inline-asm SSE blocks.
    // Reason: we want a deterministic, explicit FPU/SSE instruction stream in
    // each task so scheduler lazy-FPU switching is validated independent of
    // compiler optimization/codegen choices.
    FPU_MULTI_TASK_DONE_A.store(false, Ordering::Release);
    FPU_MULTI_TASK_DONE_B.store(false, Ordering::Release);
    FPU_MULTI_TASK_RESULT_A_BITS.store(0, Ordering::Release);
    FPU_MULTI_TASK_RESULT_B_BITS.store(0, Ordering::Release);

    sched::init();
    sched::set_kernel_address_space_cr3(vmm::get_pml4_address());

    let task_a = sched::spawn_kernel_task(multi_task_fpu_worker_a)
        .expect("FPU worker A kernel task spawn should succeed");
    let task_b = sched::spawn_kernel_task(multi_task_fpu_worker_b)
        .expect("FPU worker B kernel task spawn should succeed");
    sched::start();

    sched::wait_for_task_exit(task_a);
    sched::wait_for_task_exit(task_b);

    assert!(
        FPU_MULTI_TASK_DONE_A.load(Ordering::Acquire),
        "FPU worker A must report completion before task exit"
    );
    assert!(
        FPU_MULTI_TASK_DONE_B.load(Ordering::Acquire),
        "FPU worker B must report completion before task exit"
    );
    assert!(
        FPU_MULTI_TASK_RESULT_A_BITS.load(Ordering::Acquire) == PYTHAGORAS_TASK_A_EXPECTED_BITS,
        "worker A must compute sqrt(3^2 + 4^2) = 5.0 across context switches"
    );
    assert!(
        FPU_MULTI_TASK_RESULT_B_BITS.load(Ordering::Acquire) == PYTHAGORAS_TASK_B_EXPECTED_BITS,
        "worker B must compute sqrt(5^2 + 12^2) = 13.0 across context switches"
    );
}
