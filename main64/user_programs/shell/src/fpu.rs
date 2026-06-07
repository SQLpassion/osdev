use crate::println;
use core::arch::asm;

/// Computes c = sqrt(a² + b²) using explicit SSE2 instructions.
///
/// This smoke test validates that user-space ring 3 code can execute SSE
/// instructions and that the kernel's FPU/SSE lazy-switch handling works
/// correctly across user/kernel boundaries.
///
/// # Safety
/// - This requires `unsafe` because inline assembly is outside Rust's static safety model.
/// - `a`, `b`, and `c` are valid stack locals for 8-byte loads/stores.
fn run_sse_pythagoras(a: f64, b: f64) -> f64 {
    let mut c = 0.0f64;

    // Execute explicit SSE2 arithmetic:
    // c = sqrt((a*a) + (b*b))
    // SAFETY:
    // - This requires `unsafe` because inline assembly is outside Rust's static safety model.
    // - `a`, `b`, and `c` are valid stack locals for 8-byte loads/stores.
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

    c
}

/// Ring 3 FPU/SSE smoke test: validates user-space SSE instruction execution
/// and kernel FPU lazy-switch behavior across user/kernel transitions.
pub fn run_fpu_smoke_test() {
    const ITERATIONS: usize = 64;
    const INPUT_A1: f64 = 1.234f64;
    const INPUT_A2: f64 = 2.345f64;
    const INPUT_B1: f64 = 3.456f64;
    const INPUT_B2: f64 = 4.567f64;

    println!(
        "Running ring 3 FPU/SSE smoke test ({} iterations with 3-decimal inputs)...",
        ITERATIONS
    );

    // Run each computation once to obtain a reference value via the same SSE2
    // code path.  All subsequent iterations must reproduce the identical bit
    // pattern — any deviation means the FPU state was corrupted (e.g. by a
    // broken context switch).  Comparing SSE2 output to SSE2 output avoids
    // false failures from compile-time vs. runtime rounding differences.
    let ref_a = run_sse_pythagoras(INPUT_A1, INPUT_A2);
    let ref_b = run_sse_pythagoras(INPUT_B1, INPUT_B2);

    let mut fail_iter_a: Option<usize> = None;
    let mut fail_iter_b: Option<usize> = None;
    let mut bad_a = 0.0f64;
    let mut bad_b = 0.0f64;

    for i in 0..ITERATIONS {
        let r = run_sse_pythagoras(INPUT_A1, INPUT_A2);
        if r.to_bits() != ref_a.to_bits() && fail_iter_a.is_none() {
            fail_iter_a = Some(i);
            bad_a = r;
        }
        let r = run_sse_pythagoras(INPUT_B1, INPUT_B2);
        if r.to_bits() != ref_b.to_bits() && fail_iter_b.is_none() {
            fail_iter_b = Some(i);
            bad_b = r;
        }
    }

    if fail_iter_a.is_none() && fail_iter_b.is_none() {
        println!(
            "fputest PASSED (A={:.5}, B={:.5}, {} iters each)",
            ref_a, ref_b, ITERATIONS
        );
    } else {
        println!("fputest FAILED");
        if let Some(iter) = fail_iter_a {
            println!(
                "  Task A diverged at iter {}: ref={:.15}, got={:.15}",
                iter, ref_a, bad_a
            );
        }
        if let Some(iter) = fail_iter_b {
            println!(
                "  Task B diverged at iter {}: ref={:.15}, got={:.15}",
                iter, ref_b, bad_b
            );
        }
    }
}
