#!/bin/bash
# test_all.sh - Run all KAOS test suites across the workspace
#
# Runs:
# 1. User program unit tests (e.g. rtl8139_user_program, kbasic_user_program)
# 2. Kernel integration tests in QEMU (kernel/tests/*)
# 3. Workspace code formatting (cargo fmt --check)
# 4. Kernel & driver clippy linter (cargo clippy --target x86_64-unknown-none)

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "=================================================="
echo "  KAOS Test Suite Runner"
echo "=================================================="
echo ""

# -----------------------------------------------------------------------------
# Phase 1: User-Space Program Unit Tests
# -----------------------------------------------------------------------------
echo "[1/4] Running User-Space Program Unit Tests..."
echo "--------------------------------------------------"
echo "  -> Testing rtl8139_user_program (Ethernet, ARP, IPv4, ICMP)..."
cargo test -p rtl8139_user_program

echo ""
echo "  -> Testing kbasic_user_program (BASIC interpreter & tokenizer)..."
cargo test -p kbasic_user_program

echo ""
echo "  -> [PASSED] User-space unit tests completed successfully."
echo ""

# -----------------------------------------------------------------------------
# Phase 2: Kernel Integration Tests (in QEMU)
# -----------------------------------------------------------------------------
echo "[2/4] Running Kernel Integration Tests in QEMU..."
echo "--------------------------------------------------"
(
    cd kernel
    cargo test
)
echo ""
echo "  -> [PASSED] Kernel integration tests completed successfully."
echo ""

# -----------------------------------------------------------------------------
# Phase 3: Code Formatting Check
# -----------------------------------------------------------------------------
echo "[3/4] Checking Code Formatting (cargo fmt)..."
echo "--------------------------------------------------"
cargo fmt --check
echo "  -> [PASSED] Code formatting check passed."
echo ""

# -----------------------------------------------------------------------------
# Phase 4: Clippy Static Analysis
# -----------------------------------------------------------------------------
echo "[4/4] Running Clippy Static Analysis..."
echo "--------------------------------------------------"
cargo clippy --target x86_64-unknown-none
echo "  -> [PASSED] Clippy check passed with 0 warnings."
echo ""

echo "=================================================="
echo "  ALL TESTS AND QUALITY CHECKS PASSED!"
echo "=================================================="
