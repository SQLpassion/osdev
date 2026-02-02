# KAOS Kernel Test Framework

This document describes the custom test framework used by the KAOS kernel, how tests
are compiled and executed, and how to write new tests.

## Why a Custom Framework?

The standard Rust test harness (`#[test]`, `cargo test`) depends on the `std` library. It
uses OS facilities like threads, process exit codes, and stdout to discover, run, and
report tests. None of this exists in a bare-metal `#![no_std]` kernel — there is no OS
underneath to provide these services.

The KAOS test framework solves this by:

1. Using Rust's **`custom_test_frameworks`** nightly feature to let the compiler collect
   test functions at compile time, without depending on `std`.
2. Compiling each integration test file into a **standalone kernel binary** that boots in
   QEMU — the same way the real kernel boots.
3. Using QEMU's **`isa-debug-exit`** device to signal pass/fail back to the host via a
   process exit code.
4. Using the **serial port** (COM1) for test output, captured by QEMU on the host's
   terminal via `-serial stdio`.

Each test binary goes through the full boot sequence: BIOS, 16-bit loader, 64-bit loader,
long mode switch, page table setup, and finally the jump to `KernelMain`. This means
integration tests exercise the real boot path and run on real (emulated) hardware, not in a
mocked environment.

## How a Test Executes, Step by Step

The following sections trace the complete lifecycle of a test run, from the initial
`cargo test` command through to the final exit code on the host.

### Step 1: Cargo Invokes rustc with the `--test` Flag

When you run:

```bash
cargo test --test basic_boot
```

Cargo compiles two things:

1. **`libkaos_kernel.rlib`** — the kernel's library crate (`src/lib.rs` and all its
   modules). This is compiled as a normal library, not in test mode. It provides the
   kernel APIs that the test binary links against.

2. **The test binary** (`tests/basic_boot.rs`) — compiled as a standalone binary with the
   `--test` flag passed to `rustc`. The `--test` flag is what activates the
   `custom_test_frameworks` feature. Without it, `#[test_case]` attributes are ignored
   and the compiler does not generate the `test_main()` entry point.

Note: The `[[test]]` entries in `Cargo.toml` do **not** set `harness = false`. This is
deliberate. When `harness = false`, Cargo omits the `--test` flag from the `rustc`
invocation, which prevents `custom_test_frameworks` from activating. Leaving `harness`
at its default (`true`) ensures the `--test` flag is passed, while the
`#![feature(custom_test_frameworks)]` attribute in each test file replaces the standard
harness with our custom one.

### Step 2: The Compiler Generates `test_main()`

Each integration test file contains three crate-level attributes:

```rust
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]
```

When `rustc` processes the file with the `--test` flag:

1. **`#![feature(custom_test_frameworks)]`** enables the nightly feature that replaces the
   standard test harness.

2. **`#![test_runner(kaos_kernel::testing::test_runner)]`** tells the compiler which
   function to call with the collected tests. The compiler will generate code that calls
   `kaos_kernel::testing::test_runner` with a `&[&dyn Testable]` slice.

3. **`#![reexport_test_harness_main = "test_main"]`** controls the name of the generated
   entry point. Instead of generating a `main()` function (which conflicts with
   `#![no_main]`), the compiler generates a function called `test_main()` that we can
   call explicitly from `KernelMain`.

The compiler scans the file for every function annotated with `#[test_case]` and
synthesizes a `test_main` function equivalent to:

```rust
// Compiler-generated (you never see this code)
fn test_main() {
    kaos_kernel::testing::test_runner(&[
        &test_kernel_boots,
        &test_trivial_assertion,
        &test_vga_buffer_address,
    ]);
}
```

The type coercion from `&fn()` to `&dyn Testable` works because of the blanket
implementation in `src/testing.rs`:

```rust
impl<T: Fn()> Testable for T { ... }
```

Every `fn()` item implements the `Fn()` trait, which satisfies the `Testable` trait bound.
The compiler inserts the necessary vtable coercion when building the slice.

### Step 3: The Linker Produces an ELF Binary

The compiled test binary is linked with `rust-lld` using the kernel's linker script
(`link.ld`). The same flags from `.cargo/config.toml` apply:

- `-Tlink.ld` — use the kernel linker script
- `-z max-page-size=0x1000` — 4 KB page alignment
- `-C code-model=large` — required for higher-half kernel addresses
- `-C relocation-model=static` — no position-independent code

The linker script places the `KernelMain` function (marked with
`#[link_section = ".text.boot"]`) at the very beginning of the `.text` section so the
bootloader can jump directly to it. The virtual base address is `0xFFFF800000000000`
(higher-half kernel), with the physical load address at `0x100000` (1 MB).

The output is an ELF binary in `target/x86_64-unknown-none/debug/deps/`.

### Step 4: Cargo Invokes the Test Runner Script

The `.cargo/config.toml` file specifies a custom runner:

```toml
runner = "tests/test_runner.sh"
```

After building the test binary, Cargo invokes this script with the path to the ELF binary
as the first argument:

```
tests/test_runner.sh target/x86_64-unknown-none/debug/deps/basic_boot-08ffcb2841a31825
```

### Step 5: ELF-to-Binary Conversion

The script converts the ELF binary to a flat (raw) binary using `llvm-objcopy`:

```bash
llvm-objcopy -O binary "$TEST_BINARY" "$TEST_BIN"
```

This strips all ELF headers, section tables, and metadata, leaving only the raw machine
code and data. The bootloader expects a flat binary — it loads the file contents directly
into memory at the physical load address (`0x100000`) and jumps to the first byte.

The script tries `llvm-objcopy` first, then falls back to `rust-objcopy` or `objcopy` if
the LLVM version is not available.

### Step 6: FAT12 Disk Image Creation

The script uses Docker with the `sqlpassion/kaos-buildenv` image to create a bootable
floppy disk image:

```bash
docker run --rm -v "$MAIN64_DIR":/src sqlpassion/kaos-buildenv /bin/sh -c "
    fat_imgen -c -s boot/bootsector.bin -f kaos64_test.img
    fat_imgen -m -f kaos64_test.img -i kaosldr_16/kldr16.bin
    fat_imgen -m -f kaos64_test.img -i kaosldr_64/kldr64.bin
    fat_imgen -m -f kaos64_test.img -i test_kernel.bin -n kernel.bin
"
```

This produces a FAT12 disk image containing:

| File on disk        | Purpose                                           |
|---------------------|---------------------------------------------------|
| (boot sector)       | BIOS loads this first; it finds and loads kldr16   |
| `kldr16.bin`        | 16-bit loader: sets up environment, loads kldr64   |
| `kldr64.bin`        | 64-bit loader: enters long mode, loads kernel.bin  |
| `kernel.bin`        | The test kernel binary (our compiled test)         |

The test binary is written to the disk image as `kernel.bin` so the bootloader finds and
loads it using the same code path as the real kernel.

### Step 7: QEMU Boot

The script launches QEMU with the test disk image:

```bash
qemu-system-x86_64 \
    -drive format=raw,file="$TEST_IMG" \
    -serial stdio \
    -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
    -display none \
    -no-reboot
```

The flags:

| Flag                          | Purpose                                                    |
|-------------------------------|------------------------------------------------------------|
| `-drive format=raw,file=...`  | Attach the test disk image as the boot disk                |
| `-serial stdio`               | Route the emulated COM1 serial port to the host's terminal |
| `-device isa-debug-exit,...`  | Add a debug exit device at I/O port 0xF4                   |
| `-display none`               | No GUI window — the test runs headless                     |
| `-no-reboot`                  | On triple fault, stop instead of rebooting (aids debugging)|

If a `timeout` command is available (GNU coreutils `timeout` or macOS `gtimeout`), QEMU
is wrapped with a 30-second timeout to prevent hanging tests from blocking the suite
indefinitely. If neither is available, QEMU runs without a timeout and a warning is
printed.

### Step 8: The Boot Sequence

QEMU starts executing the BIOS, which loads the boot sector from the disk image. The boot
sequence proceeds identically to a normal kernel boot:

1. **Boot sector** — loaded by BIOS at `0x7C00`, finds and loads `kldr16.bin`
2. **16-bit loader** (`kldr16.bin`) — performs real-mode initialization, reads the BIOS
   memory map via INT 15h (E820), loads `kldr64.bin`
3. **64-bit loader** (`kldr64.bin`) — sets up the GDT, enables paging with an initial page
   table mapping the higher half, switches to long mode, loads `kernel.bin` at physical
   address `0x100000`, and jumps to the entry point

The entry point is the `KernelMain` function in the test file, placed at the beginning of
the binary by the `#[link_section = ".text.boot"]` attribute.

### Step 9: Test Kernel Initialization

Each test file provides its own `KernelMain` that performs whatever initialization the
tests require. For example, `tests/pmm_test.rs`:

```rust
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();   // required: test output goes via serial
    pmm::init();                             // required: PMM tests need the allocator
    test_main();                             // compiler-generated entry point
    loop {}
}
```

At minimum, every test must call `kaos_kernel::drivers::serial::init()` because the test
framework outputs results via the serial port (`debug!`/`debugln!` macros). Beyond that,
each test file initializes only what its tests need.

The call to `test_main()` transfers control to the compiler-generated function, which
calls `kaos_kernel::testing::test_runner()` with the collected test functions.

### Step 10: The Test Runner Executes Tests

The `test_runner` function in `src/testing.rs` receives a `&[&dyn Testable]` slice and
iterates through each test:

```rust
pub fn test_runner(tests: &[&dyn Testable]) {
    debugln!("Running {} tests:", tests.len());

    for test in tests {
        test.run();
    }

    debugln!("All {} tests passed!", tests.len());
    exit_qemu(QemuExitCode::Success);
}
```

For each test, the `Testable::run()` implementation:

1. Prints the fully-qualified function name via `core::any::type_name::<T>()` to the
   serial port (e.g., `kaos_kernel::tests::basic_boot::test_kernel_boots...`)
2. Calls the test function (`self()`)
3. If the function returns normally, prints `[ok]`

If a test function panics (via `assert!`, `assert_eq!`, `panic!`, or any other panic
path), the test file's `#[panic_handler]` is invoked.

### Step 11: Panic Handling (Test Failure)

Each test file defines a panic handler that delegates to `test_panic_handler` in
`src/testing.rs`:

```rust
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}
```

The `test_panic_handler` function:

1. Prints `[FAILED]` to the serial port
2. Prints the file and line number from the `PanicInfo` location
3. Prints the panic message (if available as a static string)
4. Calls `exit_qemu(QemuExitCode::Failed)` to terminate QEMU immediately

Because the panic handler calls `exit_qemu`, a single test failure stops the entire test
binary. Subsequent tests in the same file are not executed after a failure.

### Step 12: QEMU Exit via isa-debug-exit

The `exit_qemu` function in `src/arch/qemu.rs` writes a byte to I/O port `0xF4`:

```rust
pub fn exit_qemu(exit_code: QemuExitCode) -> ! {
    unsafe {
        let port = PortByte::new(0xF4);
        port.write(exit_code as u8);
    }
    loop { unsafe { core::arch::asm!("hlt"); } }
}
```

The `isa-debug-exit` device intercepts writes to this port and terminates the QEMU process.
It transforms the written value using the formula:

```
actual_exit_code = (value << 1) | 1
```

| Written value (`QemuExitCode`) | Raw value | QEMU exit code | Meaning |
|--------------------------------|-----------|----------------|---------|
| `Success`                      | `0x10`    | 33             | All tests passed |
| `Failed`                       | `0x11`    | 35             | A test panicked  |

The transformation exists because QEMU reserves exit code 0 for "VM shut down normally"
and exit code 1 for "VM error." By shifting and OR-ing, the debug exit device avoids
collisions with these built-in codes.

### Step 13: Exit Code Interpretation

Back on the host, `test_runner.sh` captures QEMU's exit code and maps it:

```bash
case $QEMU_EXIT in
    33) echo "TEST PASSED"; exit 0 ;;
    35) echo "TEST FAILED"; exit 1 ;;
    124) echo "TEST TIMED OUT"; exit 1 ;;
    *)  echo "QEMU ERROR (exit code: $QEMU_EXIT)"; exit 1 ;;
esac
```

Exit code 124 is produced by the `timeout` command when QEMU exceeds the 30-second limit.

Cargo interprets the script's exit code: 0 means the test passed, non-zero means it
failed. `cargo test` reports the result accordingly.

## File Overview

```
kernel_rust/
├── .cargo/config.toml         ← Target, linker flags, runner = "tests/test_runner.sh"
├── Cargo.toml                 ← [[test]] entries for each integration test
├── link.ld                    ← Linker script (shared by kernel and test binaries)
├── src/
│   ├── lib.rs                 ← Exposes kernel modules for test imports
│   ├── testing.rs             ← Testable trait, test_runner, test_panic_handler, macros
│   └── arch/
│       └── qemu.rs            ← QemuExitCode, exit_qemu (isa-debug-exit driver)
├── tests/
│   ├── basic_boot.rs          ← Boot verification tests (3 tests)
│   ├── pmm_test.rs            ← Physical memory manager tests (5 tests)
│   └── test_runner.sh          ← Per-test: ELF conversion, disk image, QEMU execution
```

## How to Add a New Test

### Adding a test to an existing file

Add a function with the `#[test_case]` attribute. No other changes are needed — the
compiler discovers it automatically:

```rust
// In tests/pmm_test.rs
#[test_case]
fn test_pmm_exhaustion() {
    pmm::with_pmm(|pmm| {
        let mut count = 0u64;
        while let Some(_frame) = pmm.alloc_frame() {
            count += 1;
        }
        assert!(count > 0, "Should have allocated at least one frame");
    });
}
```

### Creating a new integration test file

Each integration test file compiles to a separate kernel binary that boots independently
in QEMU. Create a new file when you need different initialization (e.g., testing
interrupts requires IDT setup, while PMM tests only need `pmm::init()`).

**Step 1** — Create `tests/my_new_test.rs`:

```rust
//! Description of what this test file covers

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use core::panic::PanicInfo;

/// Entry point — performs initialization required by the tests in this file
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    kaos_kernel::drivers::serial::init();
    // Add any other initialization your tests require here.
    // For example: kaos_kernel::arch::interrupts::init();
    test_main();
    loop {}
}

/// Panic handler — delegates to the test framework's failure handler
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

#[test_case]
fn test_something() {
    assert_eq!(2 + 2, 4);
}
```

**Step 2** — Register the test in `Cargo.toml`:

```toml
[[test]]
name = "my_new_test"
```

Do **not** add `harness = false` — the default (`true`) is required for
`custom_test_frameworks` to work.

**Step 3** — Run it:

```bash
cargo test --test my_new_test
```

## Available Assertion Macros

Tests can use the standard `core` assertion macros as well as two custom macros defined
in `src/testing.rs`:

| Macro                         | Source    | Notes                                       |
|-------------------------------|-----------|---------------------------------------------|
| `assert!(condition)`          | `core`    | Panics if condition is false                |
| `assert_eq!(left, right)`     | `core`    | Panics if left != right, shows both values  |
| `assert_ne!(left, right)`     | `core`    | Panics if left == right                     |
| `test_assert!(condition)`     | `testing` | Same as `assert!`, stringifies the condition|
| `test_assert_eq!(left, right)`| `testing` | Same as `assert_eq!` with custom formatting |

The custom macros (`test_assert!`, `test_assert_eq!`) are exported from the
`kaos_kernel` crate via `#[macro_export]`. They provide slightly different formatting but
are functionally equivalent to the `core` versions. Either set can be used in tests.

## Running Tests

### Run all tests

```bash
# Via the test suite script (builds + runs each test in QEMU)
./scripts/run_tests.sh

# Via Cargo (same effect — Cargo invokes test_runner.sh for each test binary)
cargo test
```

### Run a specific test file

```bash
cargo test --test basic_boot
cargo test --test pmm_test
```

### Build without running

```bash
cargo test --test basic_boot --no-run
```

### Run via the suite script with a filter

```bash
./scripts/run_tests.sh pmm_test
```

## Prerequisites

- **Rust nightly toolchain** — configured in `rust-toolchain.toml`; must include
  `rust-src` and `llvm-tools-preview` components
- **Docker** — required by `test_runner.sh` to build FAT12 disk images using the
  `sqlpassion/kaos-buildenv` container
- **QEMU** — `qemu-system-x86_64` must be in `$PATH`
- **Bootloader binaries** — `boot/bootsector.bin`, `kaosldr_16/kldr16.bin`, and
  `kaosldr_64/kldr64.bin` must exist in the `main64/` directory. Build them first with
  `build_rust.sh`
- **`timeout` command** (optional) — GNU coreutils `timeout` (Linux) or `gtimeout`
  (macOS, via `brew install coreutils`). If not available, tests run without a timeout
  guard

## Design Notes

### Why each test file is a separate kernel

In a normal Rust project, integration tests in `tests/` are separate binaries that link
against the library crate. In our case, each binary is a bootable kernel. This means:

- Each test file controls its own `KernelMain` and initialization sequence.
- Tests that need the PMM call `pmm::init()`; tests that only verify boot do not.
- A test file that crashes or triple-faults does not affect other test files.
- Each file boots from scratch in a fresh QEMU instance with clean hardware state.

The trade-off is that each test file has ~15 lines of boilerplate (`KernelMain`,
`#[panic_handler]`, feature attributes). This is inherent to the "each test is a kernel"
design.

### Why test failure stops the binary

When a test panics, the `#[panic_handler]` calls `exit_qemu(Failed)` immediately. There
is no mechanism to catch panics in `#![no_std]` without an allocator (no `catch_unwind`).
This means a failure in the first test prevents subsequent tests in the same file from
running. If you need independent test isolation, put tests in separate files.

### Serial port as the output channel

All test output (`[ok]`, `[FAILED]`, test names, panic messages) goes through the serial
port driver (`src/drivers/serial.rs`) via the `debug!`/`debugln!` macros. QEMU captures
this via `-serial stdio` and prints it to the host terminal. The VGA text mode driver is
not used during tests because `-display none` disables the graphical output.
