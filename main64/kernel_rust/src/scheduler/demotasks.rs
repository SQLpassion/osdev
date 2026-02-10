//! Round-robin scheduler demo tasks and demo entrypoint.

use core::ptr;

use crate::arch::interrupts;
use crate::drivers::keyboard;

use super::roundrobin::{init, is_running, request_stop, spawn, start};

const DEMO_SPIN_DELAY: u32 = 200_000;
const VGA_BUFFER: usize = 0xFFFF_8000_000B_8000;
const VGA_COLS: usize = 80;
const DEMO_ROWS: [usize; 3] = [18, 19, 20];

macro_rules! demo_task_fn {
    ($name:ident, $row:expr, $ch:expr, $attr:expr) => {
        extern "C" fn $name() -> ! {
            let mut col = 0usize;
            let mut previous_col = VGA_COLS - 1;
            loop {
                keyboard::poll();
                if let Some(ch) = keyboard::read_char() {
                    if ch == b'q' || ch == b'Q' {
                        request_stop();
                    }
                }

                // SAFETY:
                // - VGA text buffer is MMIO at `VGA_BUFFER`.
                // - Writes stay within one fixed visible row.
                // - Volatile access is required for MMIO semantics.
                unsafe {
                    let old_cell = VGA_BUFFER + ($row * VGA_COLS + previous_col) * 2;
                    ptr::write_volatile(old_cell as *mut u8, b' ');
                    ptr::write_volatile((old_cell + 1) as *mut u8, $attr);

                    let cell = VGA_BUFFER + ($row * VGA_COLS + col) * 2;
                    ptr::write_volatile(cell as *mut u8, $ch);
                    ptr::write_volatile((cell + 1) as *mut u8, $attr);
                }
                previous_col = col;
                col = wrap_next_col(col);

                let mut delay = 0u32;
                while delay < DEMO_SPIN_DELAY {
                    core::hint::spin_loop();
                    delay += 1;
                }
            }
        }
    };
}

#[inline]
pub const fn wrap_next_col(col: usize) -> usize {
    (col + 1) % VGA_COLS
}

demo_task_fn!(demo_task_a, 18, b'A', 0x1F);
demo_task_fn!(demo_task_b, 19, b'B', 0x2F);
demo_task_fn!(demo_task_c, 20, b'C', 0x4F);

pub fn start_round_robin_demo() {
    // Invariant: no IRQ0 during rrdemo setup.
    //
    // Rationale:
    // - PIT programming is a multi-write I/O sequence (mode + low/high divisor bytes).
    // - If a timer IRQ preempts in the middle, we can leave setup early by switching
    //   away from the bootstrap context before the sequence/state is complete.
    // - On real hardware this can leave the PIT/scheduler startup in a broken state
    //   (often only one task keeps running). QEMU tends to be more forgiving.
    //
    // Therefore: keep IF=0 from here until all scheduler state is fully initialized.
    interrupts::disable();

    // SAFETY:
    // - VGA text buffer is MMIO at `VGA_BUFFER`.
    // - Writes are bounded to rows 18..20 and visible columns.
    // - Volatile writes preserve MMIO semantics.
    unsafe {
        for row in DEMO_ROWS {
            for col in 0..VGA_COLS {
                let cell = VGA_BUFFER + (row * VGA_COLS + col) * 2;
                ptr::write_volatile(cell as *mut u8, b' ');
                ptr::write_volatile((cell + 1) as *mut u8, 0x07);
            }
        }
    }

    init();
    let _ = spawn(demo_task_a).expect("rrdemo: spawn A failed");
    let _ = spawn(demo_task_b).expect("rrdemo: spawn B failed");
    let _ = spawn(demo_task_c).expect("rrdemo: spawn C failed");
    start();

    // Do not reprogram PIT here:
    // - KernelMain already configured 250 Hz.
    // - Reprogramming in this path would reintroduce the IRQ-vs-setup race above.
    //
    // Re-enable interrupts only after init/spawn/start are complete so the first
    // timer tick sees a consistent scheduler state.
    interrupts::enable();

    while is_running() {
        // SAFETY:
        // - While demo is running, interrupts are enabled and IRQ0 drives scheduling.
        // - `hlt` sleeps until the next interrupt and is valid in ring 0.
        unsafe {
            core::arch::asm!("hlt", options(nomem, nostack, preserves_flags));
        }
    }
}
