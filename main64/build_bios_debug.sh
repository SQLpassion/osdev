#!/bin/bash
# Build script for KAOS Rust Kernel
# This script builds the Rust kernel and the bootloaders locally

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "========================================"
echo "  KAOS Rust Kernel Build Script"
echo "========================================"
echo ""

# Step 1: Build Rust kernel locally
echo "[1/3] Building Rust kernel locally..."
echo "--------------------------------------"
cd kernel

echo "  -> Running cargo build (debug)..."
cargo build

echo "  -> Extracting flat binary with cargo objcopy..."
cargo objcopy -- -O binary ../target/x86_64-unknown-none/debug/kernel.bin

echo "  -> Rust kernel built: target/x86_64-unknown-none/debug/kernel.bin"
ls -la ../target/x86_64-unknown-none/debug/kernel.bin

cd "$SCRIPT_DIR"
echo ""

# Step 1b: Build Rust 64-bit kernel loader locally (debug mode)
echo "[1b/3] Building Rust 64-bit kernel loader locally (debug)..."
echo "--------------------------------------"
cd kaosldr_64

echo "  -> Running cargo build (debug)..."
cargo build

echo "  -> Extracting flat binary with cargo objcopy..."
cargo objcopy -- -O binary ../target/x86_64-unknown-none/debug/kldr64.bin

echo "  -> Rust kernel loader built: target/x86_64-unknown-none/debug/kldr64.bin"
ls -la ../target/x86_64-unknown-none/debug/kldr64.bin

cd "$SCRIPT_DIR"
echo ""

# Step 2: Build user-mode programs
echo "[2/3] Building user-mode programs..."
echo "------------------------------------"
"$SCRIPT_DIR/build_user_programs.sh" debug
echo ""

# Step 3: Build bootloaders and create disk image
echo "[3/3] Building bootloaders and disk image..."
echo "-------------------------------------------------------"

# Assemble the boot sector and Stage 2 loader locally using nasm toolchain.
echo "  -> Building boot sector..."
cd kernel
nasm -fbin ../boot/bootsector.asm -o ../boot/bootsector.bin
cd ..

echo "  -> Building kldr16.bin..."
cd kaosldr_16
nasm -fbin kaosldr_entry.asm -o kldr16.bin
cd ..

# Build the bootable FAT32 superfloppy on the host (mtools).
echo "  -> Removing old disk image if exists..."
rm -f kaos64.img

echo "  -> Creating FAT32 disk image (superfloppy)..."
"$SCRIPT_DIR/make_fat32_image.sh" "target/x86_64-unknown-none/debug"

echo ""
echo "  -> Disk image created successfully!"
ls -la kaos64.img

echo "  -> Creating qcow2 image for UTM..."
cd "$SCRIPT_DIR"
qemu-img convert -O qcow2 kaos64.img kaos64.qcow2 
cp kaos64.qcow2 "$HOME/Library/Containers/com.utmapp.UTM/Data/Documents/KAOS x64.utm/Data/kaos64.qcow2"
echo ""
echo "  -> qcow2 image created successfully!"
ls -la kaos64.qcow2

echo ""
echo "========================================"
echo "  Build Complete!"
echo "========================================"
echo ""
echo "Output files:"
echo "  - main64/kaos64.img (bootable disk image)"
echo "  - main64/target/x86_64-unknown-none/debug/kernel.bin"
echo ""
echo "To run in QEMU:"
echo "  qemu-system-x86_64 -drive format=raw,file=kaos64.img"
echo ""
