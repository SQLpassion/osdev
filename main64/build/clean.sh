#!/bin/bash
# clean.sh - Clean up all build targets and generated artifacts in the workspace.
#
# This script runs cargo clean on the workspace (safely handling cases where target/ is mounted),
# removes legacy per-crate target directories, and deletes generated binary loaders, flat binaries,
# Map files, QCOW2 files, and disk images (.img) produced by BIOS and UEFI build scripts.
#
# Required tools: cargo.

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "========================================"
echo "  KAOS Clean-up Script"
echo "========================================"
echo ""

# Helper to clean cargo projects safely, even if target is a mount point
safe_cargo_clean() {
    if ! cargo clean 2>/dev/null; then
        if [ -d target ]; then
            # target is likely a mount point; delete its contents instead of the directory itself
            find target -mindepth 1 -delete 2>/dev/null || rm -rf target/* target/.[!.]* 2>/dev/null || true
        fi
    fi
}

# Step 1: Clean the Rust build output. With the Cargo workspace, every crate shares the single
# `target/` at the workspace root, so one `cargo clean` here removes all build artifacts at once.
echo "[1/3] Cleaning Rust workspace target..."
echo "----------------------------------------"
echo "  -> Running cargo clean on the workspace..."
safe_cargo_clean

# Step 2: Remove any stale per-crate `target/` directories left over from before the workspace
# migration. Cargo no longer writes here, so these would only mislead the build scripts.
echo "[2/3] Removing stale per-crate target directories..."
echo "---------------------------------------------"
rm -rf kernel/target kaosldr_64/target kaosldr_uefi/target
for dir in user_programs/*/ ; do
    rm -rf "${dir}target"
done

echo "[3/3] Cleaning build artifacts..."
echo "---------------------------------"
rm -f boot/bootsector.bin
rm -f kaosldr_16/kldr16.bin
rm -f kaosldr_16/*.o
rm -f kaosldr_64/kldr64.bin
rm -f kaosldr_64/*.o
rm -f kaos64.img
rm -f kaos64-uefi.img
rm -f kaos64.qcow2
rm -f *.map