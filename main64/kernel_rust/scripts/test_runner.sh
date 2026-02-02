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

# Extract test name from binary path (without hash suffix)
TEST_NAME_FULL=$(basename "$TEST_BINARY")
# Remove the hash suffix (e.g., basic_boot-08ffcb2841a31825 -> basic_boot)
TEST_NAME=$(echo "$TEST_NAME_FULL" | sed 's/-[a-f0-9]*$//')

echo "========================================"
echo "  Running test: $TEST_NAME"
echo "========================================"

# Convert ELF to flat binary for booting
# Store the test kernel binary in main64 directory
TEST_BIN="$MAIN64_DIR/test_kernel.bin"
echo "  -> Converting ELF to binary..."
llvm-objcopy -O binary "$TEST_BINARY" "$TEST_BIN" 2>/dev/null || \
    rust-objcopy -O binary "$TEST_BINARY" "$TEST_BIN" 2>/dev/null || \
    objcopy -O binary "$TEST_BINARY" "$TEST_BIN"

# Check if bootloader files exist
if [ ! -f "$MAIN64_DIR/boot/bootsector.bin" ] || \
   [ ! -f "$MAIN64_DIR/kaosldr_16/kldr16.bin" ] || \
   [ ! -f "$MAIN64_DIR/kaosldr_64/kldr64.bin" ]; then
    echo "  -> Bootloader files not found. Building bootloaders..."
    echo "     Please run build_rust.sh first to create bootloader files."
    exit 1
fi

# Create test disk image in main64 directory
TEST_IMG="$MAIN64_DIR/kaos64_test.img"
echo "  -> Creating test disk image: $TEST_IMG"

# Use Docker to create disk image (same as build script)
docker run --rm -v "$MAIN64_DIR":/src sqlpassion/kaos-buildenv /bin/sh -c "
    cd /src
    rm -f kaos64_test.img
    fat_imgen -c -s boot/bootsector.bin -f kaos64_test.img
    fat_imgen -m -f kaos64_test.img -i kaosldr_16/kldr16.bin
    fat_imgen -m -f kaos64_test.img -i kaosldr_64/kldr64.bin
    fat_imgen -m -f kaos64_test.img -i test_kernel.bin -n kernel.bin
" 2>/dev/null

# Run QEMU with the test kernel
echo "  -> Booting test kernel in QEMU..."
echo ""

# QEMU arguments:
# -drive: Use the test disk image
# -serial stdio: Output serial to terminal
# -device isa-debug-exit: Allow test to exit QEMU with exit code
# -display none: No graphical window
# -no-reboot: Don't reboot on triple fault

# Timeout in seconds - prevents hanging tests from blocking the suite
TIMEOUT_SECONDS=30

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
        -no-reboot
else
    echo "  -> Warning: no timeout command found, running without timeout"
    qemu-system-x86_64 \
        -drive format=raw,file="$TEST_IMG" \
        -serial stdio \
        -device isa-debug-exit,iobase=0xf4,iosize=0x04 \
        -display none \
        -no-reboot
fi

QEMU_EXIT=$?

# Re-enable set -e
set -e

echo ""

# Interpret exit code
# QEMU isa-debug-exit transforms the value: actual = (value << 1) | 1
# So 0x10 (16) becomes 33, and 0x11 (17) becomes 35
case $QEMU_EXIT in
    33)
        echo "========================================"
        echo "  TEST PASSED"
        echo "========================================"
        exit 0
        ;;
    35)
        echo "========================================"
        echo "  TEST FAILED"
        echo "========================================"
        exit 1
        ;;
    124)
        echo "========================================"
        echo "  TEST TIMED OUT (${TIMEOUT_SECONDS}s)"
        echo "========================================"
        exit 1
        ;;
    *)
        echo "========================================"
        echo "  QEMU ERROR (exit code: $QEMU_EXIT)"
        echo "========================================"
        exit 1
        ;;
esac
