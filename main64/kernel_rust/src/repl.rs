//! Interactive REPL (Read-Eval-Print Loop) for the kernel shell.
//!
//! Runs as a scheduled kernel task and provides a command prompt for
//! debugging, self-tests, and launching user-mode demos.

use crate::arch::power;
use crate::drivers::ata;
use crate::drivers::keyboard;
use crate::drivers::screen::{with_screen, Color, Screen};
use crate::io::fat12;
use crate::memory::bios;
use crate::memory::heap;
use crate::memory::pmm;
use crate::memory::vmm;
use crate::process;
use crate::scheduler;
use crate::user_tasks;
use core::arch::asm;
use core::fmt::Write;
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const PATTERN_DELAY_SPINS: usize = 500_000;
const VGA_TEXT_COLS: usize = 80;
const ATA_TEST_LBA: u32 = 2048;
const ATA_SECTOR_SIZE: usize = 512;
const ATA_TEST_MAX_INPUT: usize = ATA_SECTOR_SIZE - 2;
const FPU_SMOKE_EXPECTED_A_BITS: u64 = 5.0f64.to_bits();
const FPU_SMOKE_EXPECTED_B_BITS: u64 = 13.0f64.to_bits();
const FPU_SMOKE_ITERATIONS: usize = 128;

/// Kernel size stored by `KernelMain` so that the REPL task can display it
/// in the welcome banner.  Written once before the scheduler starts, read
/// only afterwards — no synchronization needed.
static mut KERNEL_SIZE: u64 = 0;
static FPU_SMOKE_DONE_A: AtomicBool = AtomicBool::new(false);
static FPU_SMOKE_DONE_B: AtomicBool = AtomicBool::new(false);
static FPU_SMOKE_RESULT_A: AtomicU64 = AtomicU64::new(0);
static FPU_SMOKE_RESULT_B: AtomicU64 = AtomicU64::new(0);

/// Store the kernel size for display in the welcome banner.
///
/// # Safety
/// - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
///
/// Must be called exactly once, before any task is spawned.
pub fn set_kernel_size(size: u64) {
    unsafe {
        KERNEL_SIZE = size;
    }
}

/// REPL task entry point — runs as a scheduled kernel task.
///
/// Uses the shared global `with_screen` writer and enters the interactive
/// command prompt loop.
pub extern "C" fn repl_task() -> ! {
    with_screen(|screen| {
        screen.clear();

        // Print welcome message
        // SAFETY:
        // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
        // - `KERNEL_SIZE` is written once during boot before scheduler start.
        // - Reads happen afterwards and are race-free in this single-core kernel.
        let kernel_size = unsafe { KERNEL_SIZE };
        screen.set_color(Color::LightGreen);
        writeln!(screen, "========================================").unwrap();
        writeln!(screen, "    KAOS - Klaus' Operating System").unwrap();
        writeln!(screen, "         Rust Kernel v0.1.0").unwrap();
        writeln!(screen, "========================================").unwrap();
        screen.set_color(Color::White);
        writeln!(screen, "Kernel loaded successfully!").unwrap();
        writeln!(screen, "Kernel size: {} bytes\n", kernel_size).unwrap();
    });

    command_prompt_loop();
}

/// Read a line into `buf`, echoing characters to the screen.
/// Returns the number of bytes written.
/// The newline is echoed but not stored in `buf`.
fn read_line(buf: &mut [u8]) -> usize {
    let mut len = 0;

    loop {
        let ch = keyboard::read_char_blocking();

        match ch {
            b'\r' | b'\n' => {
                with_screen(|screen| screen.print_char(b'\n'));
                break;
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    with_screen(|screen| screen.print_char(0x08));
                }
            }
            _ => {
                if len < buf.len() {
                    buf[len] = ch;
                    len += 1;
                    with_screen(|screen| screen.print_char(ch));
                }
            }
        }
    }

    len
}

/// Infinite prompt loop using read_line; echoes entered lines.
fn command_prompt_loop() -> ! {
    loop {
        with_screen(|screen| {
            write!(screen, "> ").unwrap();
        });

        let mut buf = [0u8; 128];
        let len = read_line(&mut buf);

        if let Ok(line) = core::str::from_utf8(&buf[..len]) {
            execute_command(line);
        } else {
            with_screen(|screen| {
                writeln!(screen, "(invalid UTF-8)").unwrap();
            });
        }
    }
}

/// Execute a simple command from a line of input.
fn execute_command(line: &str) {
    let line = line.trim();
    if line.is_empty() {
        with_screen(|screen| screen.print_char(b'\n'));
        return;
    }

    let mut parts = line.split_whitespace();
    let cmd = parts.next().unwrap();

    match cmd {
        "help" => {
            with_screen(|screen| {
                writeln!(screen, "Commands:\n").unwrap();
                writeln!(screen, "  help            - show this help").unwrap();
                writeln!(screen, "  echo <text>     - print text").unwrap();
                writeln!(screen, "  cls             - clear screen").unwrap();
                writeln!(screen, "  color <name>    - set color (white, cyan, green)").unwrap();
                writeln!(
                    screen,
                    "  mtdemo          - run VGA multitasking demo (press q to stop)"
                )
                .unwrap();
                writeln!(screen, "  meminfo         - display BIOS memory map").unwrap();
                writeln!(
                    screen,
                    "  pmm [n]         - run PMM self-test (default n=2048)"
                )
                .unwrap();
                writeln!(screen, "  vmmtest [--debug] - run VMM smoke test").unwrap();
                writeln!(screen, "  heaptest        - run heap self-test").unwrap();
                writeln!(
                    screen,
                    "  cursordemo      - run ring-3 cursor syscall demo (GetCursor/SetCursor)"
                )
                .unwrap();
                writeln!(
                    screen,
                    "  atatest         - read keyboard line, write/read ATA sector, print result"
                )
                .unwrap();
                writeln!(screen, "  dir             - list FAT12 root directory").unwrap();
                writeln!(
                    screen,
                    "  cat <file>      - print FAT12 file content (8.3 name)"
                )
                .unwrap();
                writeln!(
                    screen,
                    "  exec <file>     - run FAT12 user program in foreground"
                )
                .unwrap();
                writeln!(
                    screen,
                    "  fputest         - run scheduler FPU/SSE smoke tasks in foreground"
                )
                .unwrap();
                writeln!(screen, "  shutdown        - shutdown the system").unwrap();
            });
        }
        "echo" => {
            let rest = line[cmd.len()..].trim_start();
            if !rest.is_empty() {
                with_screen(|screen| {
                    writeln!(screen, "{}", rest).unwrap();
                });
            } else {
                with_screen(|screen| screen.print_char(b'\n'));
            }
        }
        "cls" | "clear" => {
            with_screen(|screen| screen.clear());
        }
        "color" => {
            if let Some(name) = parts.next() {
                with_screen(|screen| {
                    if name.eq_ignore_ascii_case("white") {
                        screen.set_color(Color::White);
                    } else if name.eq_ignore_ascii_case("cyan") {
                        screen.set_color(Color::LightCyan);
                    } else if name.eq_ignore_ascii_case("green") {
                        screen.set_color(Color::LightGreen);
                    } else {
                        writeln!(screen, "Unknown color: {}", name).unwrap();
                    }
                });
            } else {
                with_screen(|screen| {
                    writeln!(screen, "Usage: color <white|cyan|green>").unwrap();
                });
            }
        }
        "shutdown" => {
            with_screen(|screen| {
                writeln!(screen, "Shutting down...").unwrap();
            });
            power::shutdown();
        }
        "mtdemo" => {
            run_multitasking_vga_demo();
        }
        "meminfo" => {
            with_screen(bios::BiosInformationBlock::print_memory_map);
        }
        "pmm" => match (parts.next(), parts.next()) {
            (None, None) => pmm::run_self_test(2048),
            (Some(n_str), None) => match n_str.parse::<u32>() {
                Ok(n) if n > 0 => pmm::run_self_test(n),
                _ => with_screen(|screen| {
                    writeln!(screen, "Usage: pmm [n]  (n must be > 0)").unwrap();
                }),
            },
            _ => {
                with_screen(|screen| {
                    writeln!(screen, "Usage: pmm [n]").unwrap();
                });
            }
        },
        "testvmm" | "vmmtest" => {
            let console_debug = match (parts.next(), parts.next()) {
                (None, None) => false,
                (Some("--debug"), None) => true,
                _ => {
                    with_screen(|screen| {
                        writeln!(screen, "Usage: vmmtest [--debug]").unwrap();
                    });
                    return;
                }
            };

            vmm::set_console_debug_output(console_debug);
            let ok = vmm::test_vmm();
            if console_debug {
                with_screen(vmm::print_console_debug_output);
            }
            vmm::set_console_debug_output(false);
            if ok {
                with_screen(|screen| {
                    writeln!(screen, "VMM test complete (readback OK).").unwrap();
                });
            } else {
                with_screen(|screen| {
                    writeln!(screen, "VMM test complete (readback FAILED).").unwrap();
                });
            }
        }
        "heaptest" => {
            with_screen(heap::run_self_test);
        }
        "cursordemo" => {
            user_tasks::run_user_mode_cursor_demo();
        }
        "atatest" => {
            run_ata_interactive_roundtrip();
        }
        "dir" => {
            fat12::print_root_directory();
        }
        "cat" => match (parts.next(), parts.next()) {
            (Some(file_name), None) => match fat12::read_file(file_name) {
                Ok(content) => print_fat12_file_content(&content),
                Err(err) => with_screen(|screen| {
                    writeln!(screen, "cat failed for '{}': {}", file_name, err).unwrap();
                }),
            },
            _ => with_screen(|screen| {
                writeln!(screen, "Usage: cat <8.3-file>").unwrap();
            }),
        },
        "exec" => match (parts.next(), parts.next()) {
            (Some(file_name), None) => match process::exec_from_fat12(file_name) {
                Ok(task_id) => {
                    // Foreground exec policy:
                    // - keep REPL in this command handler until the spawned task exits,
                    // - yield cooperatively so scheduler can run the user task.
                    scheduler::wait_for_task_exit(task_id);
                }
                Err(err) => with_screen(|screen| {
                    writeln!(screen, "exec failed for '{}': {}", file_name, err).unwrap();
                }),
            },
            _ => with_screen(|screen| {
                writeln!(screen, "Usage: exec <8.3-file>").unwrap();
            }),
        },
        "fputest" => {
            run_fpu_scheduler_smoke_test();
        }
        _ => {
            with_screen(|screen| {
                writeln!(screen, "Unknown command: {}", cmd).unwrap();
            });
        }
    }
}

fn run_multitasking_vga_demo() {
    let task_ids = spawn_pattern_tasks();

    with_screen(|screen| {
        writeln!(
            screen,
            "Multitasking demo active (rows 22-24). Press q to stop."
        )
        .unwrap();
    });
    loop {
        let ch = keyboard::read_char_blocking();
        if ch == b'q' || ch == b'Q' {
            terminate_pattern_tasks(&task_ids);
            while !pattern_tasks_terminated(&task_ids) {
                scheduler::yield_now();
            }
            with_screen(|screen| {
                writeln!(screen, "\nMultitasking demo stopped.").unwrap();
            });
            return;
        }
    }
}

fn spawn_pattern_tasks() -> [usize; 3] {
    [
        scheduler::spawn_kernel_task(vga_pattern_task_a)
            .expect("failed to spawn VGA pattern task A"),
        scheduler::spawn_kernel_task(vga_pattern_task_b)
            .expect("failed to spawn VGA pattern task B"),
        scheduler::spawn_kernel_task(vga_pattern_task_c)
            .expect("failed to spawn VGA pattern task C"),
    ]
}

fn pattern_tasks_terminated(task_ids: &[usize; 3]) -> bool {
    task_ids
        .iter()
        .all(|task_id| scheduler::task_frame_ptr(*task_id).is_none())
}

fn terminate_pattern_tasks(task_ids: &[usize; 3]) {
    for task_id in task_ids {
        let _ = scheduler::terminate_task(*task_id);
    }
}

fn draw_progress_bar(screen: &mut Screen, row: usize, color: Color, label: u8, progress: usize) {
    let fill = progress.min(VGA_TEXT_COLS);
    screen.set_color(color);
    screen.set_cursor(row, 0);
    for idx in 0..VGA_TEXT_COLS {
        let ch = if idx < fill { b'#' } else { b'.' };
        if idx == 0 {
            screen.print_char(label);
        } else {
            screen.print_char(ch);
        }
    }
}

fn run_ata_interactive_roundtrip() {
    let mut existing = [0u8; ATA_SECTOR_SIZE];
    match ata::read_sectors(&mut existing, ATA_TEST_LBA, 1) {
        Ok(()) => {
            with_screen(|screen| {
                writeln!(screen, "Previous disk content:").unwrap();
            });
            print_ata_payload_from_sector(&existing, "Disk previous read-back");
        }
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "ATA initial read failed: {:?}", err).unwrap();
            });
        }
    }

    with_screen(|screen| {
        writeln!(
            screen,
            "ATA interactive test (LBA {}). Press ENTER to write your input data to disk:",
            ATA_TEST_LBA
        )
        .unwrap();
    });

    let mut input = [0u8; ATA_TEST_MAX_INPUT];
    let input_len = read_line(&mut input);

    let mut sector = [0u8; ATA_SECTOR_SIZE];
    sector[0] = (input_len & 0xFF) as u8;
    sector[1] = ((input_len >> 8) & 0xFF) as u8;
    sector[2..2 + input_len].copy_from_slice(&input[..input_len]);

    match ata::write_sectors(&sector, ATA_TEST_LBA, 1) {
        Ok(()) => {}
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "ATA write failed: {:?}", err).unwrap();
            });
            return;
        }
    }

    let mut read_back = [0u8; ATA_SECTOR_SIZE];
    match ata::read_sectors(&mut read_back, ATA_TEST_LBA, 1) {
        Ok(()) => {}
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "ATA read failed: {:?}", err).unwrap();
            });
            return;
        }
    }

    print_ata_payload_from_sector(&read_back, "Disk read-back");
}

fn print_fat12_file_content(content: &[u8]) {
    if content.is_empty() {
        with_screen(|screen| {
            writeln!(screen, "(empty file)").unwrap();
        });
        return;
    }

    if let Ok(text) = core::str::from_utf8(content) {
        with_screen(|screen| {
            writeln!(screen, "{}", text).unwrap();
        });
        return;
    }

    with_screen(|screen| {
        writeln!(screen, "(non-UTF8 content, showing hex)").unwrap();

        for (idx, byte) in content.iter().enumerate() {
            if idx > 0 && idx % 16 == 0 {
                screen.print_char(b'\n');
            }
            write!(screen, "{:02x} ", byte).unwrap();
        }

        screen.print_char(b'\n');
    });
}

fn print_ata_payload_from_sector(sector: &[u8; ATA_SECTOR_SIZE], label: &str) {
    let read_len = ((sector[1] as usize) << 8) | (sector[0] as usize);
    if read_len > ATA_TEST_MAX_INPUT {
        with_screen(|screen| {
            writeln!(
                screen,
                "{} invalid payload length: {} (max {})",
                label, read_len, ATA_TEST_MAX_INPUT
            )
            .unwrap();
        });
        return;
    }

    let payload = &sector[2..2 + read_len];
    with_screen(|screen| match core::str::from_utf8(payload) {
        Ok(text) => {
            writeln!(screen, "{}: {}", label, text).unwrap();
        }
        Err(_) => {
            writeln!(screen, "{} contains non-UTF8 bytes.", label).unwrap();
        }
    });
}

/// Runs a foreground scheduler smoke test with two FPU-heavy kernel tasks.
///
/// This is intended for real hardware validation from the REPL:
/// - each task executes explicit SSE instructions in a loop,
/// - tasks yield repeatedly to force context switches,
/// - both tasks exit, then the REPL validates deterministic results.
fn run_fpu_scheduler_smoke_test() {
    // Step 1: Reset shared completion/result slots before spawning tasks.
    FPU_SMOKE_DONE_A.store(false, Ordering::Release);
    FPU_SMOKE_DONE_B.store(false, Ordering::Release);
    FPU_SMOKE_RESULT_A.store(0, Ordering::Release);
    FPU_SMOKE_RESULT_B.store(0, Ordering::Release);

    // Step 2: Spawn both worker tasks and keep handles for foreground waiting.
    let task_a = match scheduler::spawn_kernel_task(fpu_smoke_task_a) {
        Ok(id) => id,
        Err(err) => {
            with_screen(|screen| {
                writeln!(screen, "fputest spawn A failed: {:?}", err).unwrap();
            });
            return;
        }
    };

    let task_b = match scheduler::spawn_kernel_task(fpu_smoke_task_b) {
        Ok(id) => id,
        Err(err) => {
            let _ = scheduler::terminate_task(task_a);
            with_screen(|screen| {
                writeln!(screen, "fputest spawn B failed: {:?}", err).unwrap();
            });
            return;
        }
    };

    // Step 3: Wait in foreground until both tasks exit.
    scheduler::wait_for_task_exit(task_a);
    scheduler::wait_for_task_exit(task_b);

    // Step 4: Read back completion flags and computed values.
    let done_a = FPU_SMOKE_DONE_A.load(Ordering::Acquire);
    let done_b = FPU_SMOKE_DONE_B.load(Ordering::Acquire);
    let result_a = FPU_SMOKE_RESULT_A.load(Ordering::Acquire);
    let result_b = FPU_SMOKE_RESULT_B.load(Ordering::Acquire);
    let pass = done_a
        && done_b
        && result_a == FPU_SMOKE_EXPECTED_A_BITS
        && result_b == FPU_SMOKE_EXPECTED_B_BITS;

    with_screen(|screen| {
        if pass {
            writeln!(screen, "fputest passed (task A=5.0, task B=13.0)").unwrap();
        } else {
            writeln!(screen, "fputest FAILED").unwrap();
            writeln!(
                screen,
                "  done_a={} done_b={} result_a=0x{:016x} result_b=0x{:016x}",
                done_a, done_b, result_a, result_b
            )
            .unwrap();
        }
    });
}

fn run_sse_pythagoras(a: f64, b: f64) -> f64 {
    let mut c = 0.0f64;

    // Decision note:
    // This could be written as pure Rust (`(a * a + b * b).sqrt()`), and that
    // would usually compile to SSE2 on x86_64.  We intentionally keep explicit
    // asm here so the REPL hardware smoke test executes a predictable SSE
    // instruction sequence regardless of optimization/codegen differences.
    // That makes #NM/lazy-FPU behavior easier to validate on real machines.
    //
    // Execute explicit SSE arithmetic:
    // c = sqrt((a*a) + (b*b))
    // SAFETY:
    // - This requires `unsafe` because inline assembly is outside Rust's
    //   static safety model.
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

extern "C" fn fpu_smoke_task_a() -> ! {
    let mut result = 0.0f64;

    // Run multiple iterations with cooperative yields so the scheduler must
    // preserve FPU/SSE state across frequent task switches.
    for _ in 0..FPU_SMOKE_ITERATIONS {
        result = run_sse_pythagoras(3.0, 4.0);
        scheduler::yield_now();
    }

    FPU_SMOKE_RESULT_A.store(result.to_bits(), Ordering::Release);
    FPU_SMOKE_DONE_A.store(true, Ordering::Release);
    scheduler::exit_current_task();
}

extern "C" fn fpu_smoke_task_b() -> ! {
    let mut result = 0.0f64;

    // Use different operands than task A to detect accidental state leakage
    // between two FPU-using tasks.
    for _ in 0..FPU_SMOKE_ITERATIONS {
        result = run_sse_pythagoras(5.0, 12.0);
        scheduler::yield_now();
    }

    FPU_SMOKE_RESULT_B.store(result.to_bits(), Ordering::Release);
    FPU_SMOKE_DONE_B.store(true, Ordering::Release);
    scheduler::exit_current_task();
}

/// Generic VGA progress-bar task used to visualize task switching on live screen output.
fn vga_pattern_task(row: usize, label: u8, color: Color, step: usize) -> ! {
    let mut screen = Screen::new();
    let mut progress = 0usize;

    loop {
        draw_progress_bar(&mut screen, row, color, label, progress);
        progress = (progress + step) % (VGA_TEXT_COLS + 1);

        for _ in 0..PATTERN_DELAY_SPINS {
            core::hint::spin_loop();
        }

        scheduler::yield_now();
    }
}

extern "C" fn vga_pattern_task_a() -> ! {
    vga_pattern_task(22, b'A', Color::LightCyan, 1)
}

extern "C" fn vga_pattern_task_b() -> ! {
    vga_pattern_task(23, b'B', Color::Yellow, 2)
}

extern "C" fn vga_pattern_task_c() -> ! {
    vga_pattern_task(24, b'C', Color::Pink, 3)
}
