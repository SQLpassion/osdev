#!/bin/bash

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "========================================"
echo "  KAOS Clean-up Script"
echo "========================================"
echo ""

# Step 1: Clean the Rust kernel locally
echo "[1/2] Cleaning Rust kernel..."
echo "-----------------------------"
cd kernel_rust

echo "  -> Running cargo clean..."
cargo clean

echo "[2/2] Cleaning everything else..."
echo "---------------------------------"
cd ..
rm -f boot/bootsector.bin
rm -f kaosldr_16/kldr16.bin
rm -f kaosldr_16/*.o
rm -f kaosldr_64/kldr64.bin
rm -f kaosldr_64/*.o
rm -f kaos64_rust.img
rm -f kaos64_rust.qcow2