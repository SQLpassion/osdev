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
   [ ! -f "$MAIN64_DIR/kaosldr_16/kldr16.bin" ] || \
   [ ! -f "$MAIN64_DIR/kaosldr_64/target/x86_64-unknown-none/debug/kldr64.bin" ]; then
    echo "  -> Bootloader files not found. Building bootloaders..."
    echo "     Please run build_kernel_debug.sh first to create bootloader files."
    exit 1
fi

# Ensure user-mode binaries exist for FAT12 integration tests.
USER_PROGRAM_HELLO_BIN="$MAIN64_DIR/user_programs/hello/hello.bin"
USER_PROGRAM_READLINE_BIN="$MAIN64_DIR/user_programs/readline/readline.bin"
USER_PROGRAM_FILEDEMO_BIN="$MAIN64_DIR/user_programs/filedemo/filedemo.bin"
if [ ! -f "$USER_PROGRAM_HELLO_BIN" ] || [ ! -f "$USER_PROGRAM_READLINE_BIN" ] || [ ! -f "$USER_PROGRAM_FILEDEMO_BIN" ]; then
    echo "  -> User program binary missing. Building user-mode programs..."
    "$MAIN64_DIR/build_user_programs.sh" debug
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

# Create test disk image in main64 directory
TEST_IMG="$MAIN64_DIR/kaos64_test.img"

# Create test disk image
if command -v fat_imgen &>/dev/null; then
    echo "  -> Creating test disk image natively..."
    (
        cd "$MAIN64_DIR"
        rm -f kaos64_test.img
        fat_imgen -c -s boot/bootsector.bin -f kaos64_test.img
        fat_imgen -m -f kaos64_test.img -i kaosldr_16/kldr16.bin
        fat_imgen -m -f kaos64_test.img -i kaosldr_64/target/x86_64-unknown-none/debug/kldr64.bin
        fat_imgen -m -f kaos64_test.img -i SFile.txt
        fat_imgen -m -f kaos64_test.img -i BigFile.txt
        fat_imgen -m -f kaos64_test.img -i user_programs/hello/hello.bin -n HELLO.BIN
        fat_imgen -m -f kaos64_test.img -i user_programs/readline/readline.bin -n READLINE.BIN
        fat_imgen -m -f kaos64_test.img -i user_programs/filedemo/filedemo.bin -n FILEDEMO.BIN
        fat_imgen -m -f kaos64_test.img -i test_kernel.bin -n kernel.bin
    )
else
    # Use Docker to create disk image since we are running locally on macOS
    echo "  -> Creating test disk image via Docker..."
    docker run --rm -v "$MAIN64_DIR":/src sqlpassion/kaos-buildenv /bin/sh -c "
        cd /src
        rm -f kaos64_test.img
        fat_imgen -c -s boot/bootsector.bin -f kaos64_test.img
        fat_imgen -m -f kaos64_test.img -i kaosldr_16/kldr16.bin
        fat_imgen -m -f kaos64_test.img -i kaosldr_64/target/x86_64-unknown-none/debug/kldr64.bin
        fat_imgen -m -f kaos64_test.img -i SFile.txt
        fat_imgen -m -f kaos64_test.img -i BigFile.txt
        fat_imgen -m -f kaos64_test.img -i user_programs/hello/hello.bin -n HELLO.BIN
        fat_imgen -m -f kaos64_test.img -i user_programs/readline/readline.bin -n READLINE.BIN
        fat_imgen -m -f kaos64_test.img -i user_programs/filedemo/filedemo.bin -n FILEDEMO.BIN
        fat_imgen -m -f kaos64_test.img -i test_kernel.bin -n kernel.bin
    " 2>/dev/null
fi

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

if [ -n "$TIMEOUT_CMD" ]; then
    $TIMEOUT_CMD $TIMEOUT_SECONDS qemu-system-x86_64 \
        -drive format=raw,file="$TEST_IMG" \
        -serial stdio \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -display none \
        -no-reboot < /dev/null
else
    qemu-system-x86_64 \
        -drive format=raw,file="$TEST_IMG" \
        -serial stdio \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -display none \
        -no-reboot < /dev/null
fi

QEMU_EXIT=$?

# Re-enable set -e
set -e

# Record the test result
if [ $QEMU_EXIT -eq 33 ]; then
    echo "OK" > "$RESULTS_DIR/$TEST_NAME"
else
    echo "FAIL" > "$RESULTS_DIR/$TEST_NAME"
fi

# Count how many tests have finished in this run
TOTAL_TESTS=$(ls -1 "$SCRIPT_DIR"/*.rs 2>/dev/null | wc -l)
FINISHED_TESTS=$(find "$RESULTS_DIR" -maxdepth 1 -type f | wc -l)

if [ "$FINISHED_TESTS" -eq "$TOTAL_TESTS" ]; then
    if mkdir "$RESULTS_DIR/summary.lock" 2>/dev/null; then
        echo ""
        echo "=================================================="
        echo "          GLOBAL TEST RUN SUMMARY"
        echo "=================================================="
        passed=0
        failed=0
        for f in "$RESULTS_DIR"/*; do
            [ -f "$f" ] || continue
            tname=$(basename "$f")
            status=$(cat "$f")
            if [ "$status" = "OK" ]; then
                printf "  %-40s [\033[0;32mPASSED\033[0m]\n" "$tname"
                passed=$((passed + 1))
            else
                printf "  %-40s [\033[0;31mFAILED\033[0m]\n" "$tname"
                failed=$((failed + 1))
            fi
        done
        echo "--------------------------------------------------"
        if [ $failed -eq 0 ]; then
            echo -e "  \033[1;32mALL TESTS PASSED ($passed/$TOTAL_TESTS test files)\033[0m"
        else
            echo -e "  \033[1;31mSOME TESTS FAILED ($failed/$TOTAL_TESTS test files failed)\033[0m"
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
