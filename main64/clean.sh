#!/bin/bash

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "========================================"
echo "  KAOS Clean-up Script"
echo "========================================"
echo ""

# Step 1: Clean the Rust kernel and loaders locally
echo "[1/3] Cleaning Rust kernel and loader..."
echo "----------------------------------------"
cd kernel
echo "  -> Running cargo clean on kernel..."
cargo clean

cd ../kaosldr_64
echo "  -> Running cargo clean on kaosldr_64..."
cargo clean

# Step 2: Clean Rust libraries and user programs
echo "[2/3] Cleaning libraries and user programs..."
echo "---------------------------------------------"
cd ../lib_kaos
if [ -f "Cargo.toml" ]; then
    echo "  -> Running cargo clean on lib_kaos..."
    cargo clean
fi

cd ../lib_tui
if [ -f "Cargo.toml" ]; then
    echo "  -> Running cargo clean on lib_tui..."
    cargo clean
fi

cd ../user_programs
for dir in */ ; do
    if [ -f "${dir}Cargo.toml" ]; then
        echo "  -> Running cargo clean on user_programs/${dir%/}"
        (cd "$dir" && cargo clean)
    fi
done

echo "[3/3] Cleaning build artifacts..."
echo "---------------------------------"
cd ..
rm -f boot/bootsector.bin
rm -f kaosldr_16/kldr16.bin
rm -f kaosldr_16/*.o
rm -f kaosldr_64/kldr64.bin
rm -f kaosldr_64/*.o
rm -f kaos64.img
rm -f kaos64.qcow2