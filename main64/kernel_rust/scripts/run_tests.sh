#!/bin/bash
# KAOS Kernel Test Suite Runner
#
# This script builds and runs all kernel integration tests.
# Each test is built as a standalone kernel binary and run in QEMU.
#
# Output files are stored in osdev/main64/:
#   - test_kernel.bin: The converted test kernel binary
#   - kaos64_test.img: The FAT12 disk image for testing
#
# Usage: ./scripts/run_tests.sh [test_name]
#
# If test_name is provided, only that test is run.
# Otherwise, all tests defined in Cargo.toml are run.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
MAIN64_DIR="$(cd "$PROJECT_DIR/.." && pwd)"
TARGET_DIR="$PROJECT_DIR/target/x86_64-unknown-none/debug"

# Colors for output
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
CYAN='\033[0;36m'
NC='\033[0m' # No Color

echo ""
echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}  KAOS Kernel Test Suite${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""
echo "Test artifacts will be stored in: $MAIN64_DIR"
echo ""

# Check for bootloader files
if [ ! -f "$MAIN64_DIR/boot/bootsector.bin" ] || \
   [ ! -f "$MAIN64_DIR/kaosldr_16/kldr16.bin" ] || \
   [ ! -f "$MAIN64_DIR/kaosldr_64/kldr64.bin" ]; then
    echo -e "${YELLOW}Warning: Bootloader files not found.${NC}"
    echo "Please run build_rust.sh first to create bootloader files."
    exit 1
fi

cd "$PROJECT_DIR"

# Get list of tests from Cargo.toml
if [ -n "$1" ]; then
    TESTS=("$1")
else
    # Extract test names from Cargo.toml [[test]] sections
    TESTS=($(grep -A1 '^\[\[test\]\]' Cargo.toml | grep 'name = ' | sed 's/.*name = "\([^"]*\)".*/\1/'))
fi

if [ ${#TESTS[@]} -eq 0 ]; then
    echo "No tests found in Cargo.toml"
    exit 0
fi

TOTAL_TESTS=${#TESTS[@]}
PASSED_TESTS=0
FAILED_TESTS=0
FAILED_NAMES=()

echo "Found $TOTAL_TESTS test(s): ${TESTS[*]}"
echo ""

for TEST_NAME in "${TESTS[@]}"; do
    echo "----------------------------------------"
    echo -e "Test: ${CYAN}$TEST_NAME${NC}"
    echo "----------------------------------------"

    # Build the test binary
    echo "  -> Building..."

    # Build just this specific test target
    if ! cargo build --target x86_64-unknown-none --test "$TEST_NAME" 2>&1; then
        echo -e "  -> ${RED}Build failed${NC}"
        FAILED_TESTS=$((FAILED_TESTS + 1))
        FAILED_NAMES+=("$TEST_NAME (build failed)")
        continue
    fi

    # Find the built test binary (it will be in deps with a hash suffix)
    TEST_BINARY=$(find "$TARGET_DIR/deps" -maxdepth 1 -name "${TEST_NAME}-*" -type f -executable 2>/dev/null | head -1)

    if [ -z "$TEST_BINARY" ]; then
        # Try without executable flag (for cross-compiled binaries)
        TEST_BINARY=$(find "$TARGET_DIR/deps" -maxdepth 1 -name "${TEST_NAME}-*" -type f ! -name "*.d" ! -name "*.rlib" ! -name "*.rmeta" 2>/dev/null | head -1)
    fi

    if [ -z "$TEST_BINARY" ] || [ ! -f "$TEST_BINARY" ]; then
        echo -e "  -> ${RED}Could not find test binary${NC}"
        FAILED_TESTS=$((FAILED_TESTS + 1))
        FAILED_NAMES+=("$TEST_NAME (binary not found)")
        continue
    fi

    echo "  -> Binary: $(basename "$TEST_BINARY")"
    echo "  -> Running in QEMU..."
    echo ""

    # Run the test using the test_runner script
    if "$SCRIPT_DIR/test_runner.sh" "$TEST_BINARY"; then
        PASSED_TESTS=$((PASSED_TESTS + 1))
    else
        FAILED_TESTS=$((FAILED_TESTS + 1))
        FAILED_NAMES+=("$TEST_NAME")
    fi

    echo ""
done

echo -e "${CYAN}========================================${NC}"
echo -e "${CYAN}  Test Results${NC}"
echo -e "${CYAN}========================================${NC}"
echo ""
echo "Total:  $TOTAL_TESTS"
echo -e "Passed: ${GREEN}$PASSED_TESTS${NC}"
echo -e "Failed: ${RED}$FAILED_TESTS${NC}"

if [ $FAILED_TESTS -gt 0 ]; then
    echo ""
    echo "Failed tests:"
    for name in "${FAILED_NAMES[@]}"; do
        echo -e "  - ${RED}$name${NC}"
    done
fi

echo ""
echo -e "${CYAN}========================================${NC}"

if [ $FAILED_TESTS -gt 0 ]; then
    exit 1
fi

exit 0
