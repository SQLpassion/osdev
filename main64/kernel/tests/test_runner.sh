#!/bin/bash
# KAOS Kernel Test Runner
#
# This script is called by `cargo test` for each test binary.
# It boots the test kernel in QEMU and checks the exit code.
#
# Usage: test_runner.sh <path-to-test-binary>
#
# QEMU Exit Codes:
#   33 = Success (test passed)
#   35 = Failure (test failed)
#   Other = QEMU error or timeout

set -e

# The test binary path is passed as the first argument
TEST_BINARY="$1"

if [ -z "$TEST_BINARY" ]; then
    echo "Error: No test binary specified"
    exit 1
fi

if [ ! -f "$TEST_BINARY" ]; then
    echo "Error: Test binary not found: $TEST_BINARY"
    exit 1
fi

# Get the script directory and project root
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MAIN64_DIR="$(cd "$PROJECT_DIR/.." && pwd)"

# Initialize test results tracking
RESULTS_DIR="$MAIN64_DIR/target/test-results"
PID_FILE="$MAIN64_DIR/target/test_runner_parent.pid"
mkdir -p "$RESULTS_DIR"

CURRENT_PARENT_PID=$PPID
if [ ! -f "$PID_FILE" ] || [ "$(cat "$PID_FILE" 2>/dev/null)" != "$CURRENT_PARENT_PID" ]; then
    echo "$CURRENT_PARENT_PID" > "$PID_FILE"
    rm -rf "$RESULTS_DIR"/* 2>/dev/null
fi


# Extract test name from binary path (without hash suffix)
TEST_NAME_FULL=$(basename "$TEST_BINARY")
# Remove the hash suffix (e.g., basic_boot-08ffcb2841a31825 -> basic_boot)
TEST_NAME=$(echo "$TEST_NAME_FULL" | sed 's/-[a-f0-9]*$//')

echo ""
echo "========================================"
echo "  Running test: $TEST_NAME"
echo "========================================"

# Convert ELF to flat binary for booting
# Store the test kernel binary in main64 directory
TEST_BIN="$MAIN64_DIR/test_kernel.bin"
llvm-objcopy -O binary "$TEST_BINARY" "$TEST_BIN" 2>/dev/null || \
    rust-objcopy -O binary "$TEST_BINARY" "$TEST_BIN" 2>/dev/null || \
    objcopy -O binary "$TEST_BINARY" "$TEST_BIN"

# Check if bootloader files exist
if [ ! -f "$MAIN64_DIR/boot/bootsector.bin" ] || \
   [ ! -f "$MAIN64_DIR/kaosldr_16/kldr16.bin" ]; then
    echo "  -> Bootloader files not found. Building bootloaders..."
    echo "     Please run build/build_bios_debug.sh or build/build_bios_debug_devcontainer.sh first to create bootloader files."
    exit 1
fi

# Ensure user-mode binaries exist for filesystem integration tests.
USER_PROGRAM_HELLO_BIN="$MAIN64_DIR/user_programs/hello/hello.bin"
USER_PROGRAM_READLINE_BIN="$MAIN64_DIR/user_programs/readline/readline.bin"
USER_PROGRAM_FILEDEMO_BIN="$MAIN64_DIR/user_programs/filedemo/filedemo.bin"
if [ ! -f "$USER_PROGRAM_HELLO_BIN" ] || [ ! -f "$USER_PROGRAM_READLINE_BIN" ] || [ ! -f "$USER_PROGRAM_FILEDEMO_BIN" ]; then
    echo "  -> User program binary missing. Building user-mode programs..."
    "$MAIN64_DIR/build/helper_build_user_programs.sh" debug
fi

if [ ! -f "$USER_PROGRAM_HELLO_BIN" ]; then
    echo "Error: User program binary not found after build: $USER_PROGRAM_HELLO_BIN"
    exit 1
fi

if [ ! -f "$USER_PROGRAM_READLINE_BIN" ]; then
    echo "Error: User program binary not found after build: $USER_PROGRAM_READLINE_BIN"
    exit 1
fi

if [ ! -f "$USER_PROGRAM_FILEDEMO_BIN" ]; then
    echo "Error: User program binary not found after build: $USER_PROGRAM_FILEDEMO_BIN"
    exit 1
fi

# Create test disk image in main64 directory (FAT32 superfloppy, matching the boot path).
TEST_IMG="$MAIN64_DIR/kaos64_test.img"

if ! command -v mformat &>/dev/null || ! command -v mcopy &>/dev/null; then
    echo "Error: mtools (mformat/mcopy) is required to build the FAT32 test image." >&2
    echo "       Install with 'brew install mtools' (macOS) or 'apt-get install mtools' (Linux)." >&2
    exit 1
fi

echo "  -> Creating FAT32 test disk image..."
(
    cd "$MAIN64_DIR"
    # Disk layout constants - MUST match boot/bootsector.asm.
    RESERVED_SECTORS=64
    KLDR16_LBA=8
    KLDR64_LBA=16

    rm -f kaos64_test.img
    dd if=/dev/zero of=kaos64_test.img bs=1048576 count=64 2>/dev/null
    mformat -i kaos64_test.img -F -R "$RESERVED_SECTORS" ::

    # The test kernel is the boot kernel for this run (loaded as KERNEL.BIN by kaosldr_64).
    mcopy -i kaos64_test.img test_kernel.bin                     ::/KERNEL.BIN
    mcopy -i kaos64_test.img SFile.txt                           ::/SFILE.TXT
    mcopy -i kaos64_test.img BigFile.txt                         ::/BIGFILE.TXT
    mcopy -i kaos64_test.img user_programs/hello/hello.bin       ::/HELLO.BIN
    mcopy -i kaos64_test.img user_programs/readline/readline.bin ::/READLINE.BIN
    mcopy -i kaos64_test.img user_programs/filedemo/filedemo.bin ::/FILEDEMO.BIN

    # Early loaders at their fixed reserved LBAs.
    dd if=kaosldr_16/kldr16.bin of=kaos64_test.img bs=512 seek="$KLDR16_LBA" conv=notrunc 2>/dev/null
    dd if=target/x86_64-unknown-none/debug/kldr64.bin of=kaos64_test.img bs=512 seek="$KLDR64_LBA" conv=notrunc 2>/dev/null

    # Overlay our boot code onto the VBR while preserving mformat's FAT32 BPB.
    dd if=kaos64_test.img of=bpb_save.bin bs=1 skip=11 count=79 2>/dev/null
    dd if=boot/bootsector.bin of=kaos64_test.img bs=512 count=1 conv=notrunc 2>/dev/null
    dd if=bpb_save.bin of=kaos64_test.img bs=1 seek=11 count=79 conv=notrunc 2>/dev/null
    rm -f bpb_save.bin
)

# Run QEMU with the test kernel
echo ""

# QEMU arguments:
# -drive: Use the test disk image
# -serial stdio: Output serial to terminal
# -device isa-debug-exit: Allow test to exit QEMU with exit code
# -display none: No graphical window
# -no-reboot: Don't reboot on triple fault

# Timeout in seconds - prevents hanging tests from blocking the suite
TIMEOUT_SECONDS=90

# Detect a usable timeout command (GNU coreutils `timeout` or macOS `gtimeout`)
TIMEOUT_CMD=""
if command -v timeout &>/dev/null; then
    TIMEOUT_CMD="timeout"
elif command -v gtimeout &>/dev/null; then
    TIMEOUT_CMD="gtimeout"
fi

# Disable set -e temporarily to capture QEMU exit code
set +e

QEMU_LOG="$RESULTS_DIR/$TEST_NAME.log"

if [ -n "$TIMEOUT_CMD" ]; then
    echo "1" | $TIMEOUT_CMD $TIMEOUT_SECONDS qemu-system-x86_64 \
        -drive format=raw,file="$TEST_IMG" \
        -serial stdio \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -display none \
        -no-reboot > "$QEMU_LOG" 2>&1
else
    echo "1" | qemu-system-x86_64 \
        -drive format=raw,file="$TEST_IMG" \
        -serial stdio \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -display none \
        -no-reboot > "$QEMU_LOG" 2>&1
fi

QEMU_EXIT=$?

# Re-enable set -e
set -e

# Output the logs so the user sees the output in the console
cat "$QEMU_LOG"

# Extract test case counts
total_cases=$(grep -a -oE "Total:[[:space:]]+[0-9]+" "$QEMU_LOG" | tail -n 1 | grep -oE "[0-9]+" || true)
passed_cases=$(grep -a -oE "Passed:[[:space:]]+[0-9]+" "$QEMU_LOG" | tail -n 1 | grep -oE "[0-9]+" || true)
if [ -z "$total_cases" ] || [ "$total_cases" -eq 0 ]; then
    total_cases=1
    if [ $QEMU_EXIT -eq 33 ]; then
        passed_cases=1
    else
        passed_cases=0
    fi
fi
[ -z "$passed_cases" ] && passed_cases=0

# Record the test result with counts
if [ $QEMU_EXIT -eq 33 ]; then
    echo "OK:$total_cases:$passed_cases" > "$RESULTS_DIR/$TEST_NAME"
else
    echo "FAIL:$total_cases:$passed_cases" > "$RESULTS_DIR/$TEST_NAME"
fi

# Count how many tests have finished in this run
TOTAL_TESTS=$(ls -1 "$SCRIPT_DIR"/*.rs 2>/dev/null | wc -l)
FINISHED_TESTS=$(find "$RESULTS_DIR" -maxdepth 1 -type f ! -name "*.log" | wc -l)

if [ "$FINISHED_TESTS" -eq "$TOTAL_TESTS" ]; then
    if mkdir "$RESULTS_DIR/summary.lock" 2>/dev/null; then
        echo ""
        echo "=================================================="
        echo "          GLOBAL TEST RUN SUMMARY"
        echo "=================================================="
        passed_files=0
        failed_files=0
        total_cases_sum=0
        passed_cases_sum=0
        
        for f in "$RESULTS_DIR"/*; do
            [ -f "$f" ] || continue
            case "$f" in
                *.log) continue ;;
            esac
            tname=$(basename "$f")
            
            content=$(cat "$f")
            status=$(echo "$content" | cut -d: -f1)
            total=$(echo "$content" | cut -d: -f2)
            passed=$(echo "$content" | cut -d: -f3)
            
            [ -z "$total" ] && total=0
            [ -z "$passed" ] && passed=0
            
            total_cases_sum=$((total_cases_sum + total))
            passed_cases_sum=$((passed_cases_sum + passed))
            
            if [ "$status" = "OK" ]; then
                printf "  %-40s [\033[0;32mPASSED\033[0m] (%d/%d cases)\n" "$tname" "$passed" "$total"
                passed_files=$((passed_files + 1))
            else
                printf "  %-40s [\033[0;31mFAILED\033[0m] (%d/%d cases)\n" "$tname" "$passed" "$total"
                failed_files=$((failed_files + 1))
            fi
        done
        echo "--------------------------------------------------"
        if [ $failed_files -eq 0 ]; then
            echo -e "  \033[1;32mALL TESTS PASSED ($passed_cases_sum/$total_cases_sum test cases across $passed_files test files)\033[0m"
        else
            echo -e "  \033[1;31mSOME TESTS FAILED ($passed_cases_sum/$total_cases_sum test cases passed)\033[0m"
        fi
        echo "=================================================="
        echo ""
        rm -f "$TEST_BIN" "$TEST_IMG" "$PID_FILE"
        rmdir "$RESULTS_DIR/summary.lock" 2>/dev/null
        rm -rf "$RESULTS_DIR"
    fi
fi

if [ $QEMU_EXIT -eq 33 ]; then
    exit 0
else
    exit 1
fi
