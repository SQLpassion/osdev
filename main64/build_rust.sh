#!/bin/bash
# Build script for KAOS Rust Kernel
# This script builds the Rust kernel locally and uses Docker for bootloaders

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "========================================"
echo "  KAOS Rust Kernel Build Script"
echo "========================================"
echo ""

# Step 1: Build Rust kernel locally
echo "[1/2] Building Rust kernel locally..."
echo "--------------------------------------"
cd kernel_rust

echo "  -> Running cargo build (debug)..."
cargo build

echo "  -> Extracting flat binary with cargo objcopy..."
cargo objcopy -- -O binary target/x86_64-unknown-none/debug/kernel.bin

echo "  -> Rust kernel built: kernel_rust/target/x86_64-unknown-none/debug/kernel.bin"
ls -la target/x86_64-unknown-none/debug/kernel.bin

cd "$SCRIPT_DIR"
echo ""

# Step 2: Build bootloaders and create disk image in Docker
echo "[2/2] Building bootloaders and disk image in Docker..."
echo "-------------------------------------------------------"

docker run --rm -v "$(dirname "$SCRIPT_DIR")":/src sqlpassion/kaos-buildenv /bin/sh -c '
set -e
cd /src/main64

echo "  -> Building boot sector..."
cd kernel
nasm -fbin ../boot/bootsector.asm -o ../boot/bootsector.bin
cd ..

echo "  -> Building kldr16.bin..."
cd kaosldr_16
nasm -fbin kaosldr_entry.asm -o kldr16.bin
cd ..

echo "  -> Building kldr64.bin..."
cd kaosldr_64
make clean
make
cd ..

echo "  -> Removing old disk image if exists..."
rm -f kaos64_rust.img

echo "  -> Creating FAT12 disk image..."
fat_imgen -c -s boot/bootsector.bin -f kaos64_rust.img
fat_imgen -m -f kaos64_rust.img -i kaosldr_16/kldr16.bin
fat_imgen -m -f kaos64_rust.img -i kaosldr_64/kldr64.bin
fat_imgen -m -f kaos64_rust.img -i kernel_rust/target/x86_64-unknown-none/debug/kernel.bin

echo ""
echo "  -> Disk image created successfully!"
ls -la kaos64_rust.img
'

echo ""
echo "========================================"
echo "  Build Complete!"
echo "========================================"
echo ""
echo "Output files:"
echo "  - main64/kaos64_rust.img (bootable disk image)"
echo "  - main64/kernel_rust/target/x86_64-unknown-none/debug/kernel.bin"
echo ""
echo "To run in QEMU:"
echo "  qemu-system-x86_64 -drive format=raw,file=kaos64_rust.img"
echo ""
