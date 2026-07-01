#!/bin/bash
# build_bios_debug_devcontainer.sh - Build the KAOS Rust Kernel and bootloaders inside the dev container.
#
# This script compiles the 16-bit real-mode entry loader, the 64-bit kernel loader, the kernel (debug),
# and user programs. It packages them into a raw bootable legacy BIOS FAT32 disk image (kaos64.img)
# using a helper script, without performing host-specific operations or deployments.
#
# Required tools: nasm, cargo (Rust nightly target x86_64-unknown-none), cargo-binutils (cargo objcopy),
# and mtools. All are preinstalled in the dev container; on macOS install them with:
#   brew install nasm mtools
#   rustup component add llvm-tools-preview
#   cargo install cargo-binutils

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

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

cd "$PROJECT_ROOT"
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

cd "$PROJECT_ROOT"
echo ""

# Step 2: Build user-mode programs
echo "[2/3] Building user-mode programs..."
echo "------------------------------------"
"$SCRIPT_DIR/helper_build_user_programs.sh" debug
echo ""

# Step 3: Build bootloaders and create disk image
echo "[3/3] Building bootloaders and disk image..."
echo "-------------------------------------------------------"

echo "  -> Building boot sector..."
cd kernel
nasm -fbin ../boot/bootsector.asm -o ../boot/bootsector.bin
cd ..

echo "  -> Building kldr16.bin..."
cd kaosldr_16
nasm -fbin kaosldr_entry.asm -o kldr16.bin
cd ..

echo "  -> Removing old disk image if exists..."
rm -f kaos64.img

echo "  -> Creating FAT32 disk image (superfloppy)..."
"$SCRIPT_DIR/helper_make_fat32_bios_image.sh" "target/x86_64-unknown-none/debug"

echo ""
echo "  -> Disk image created successfully!"
ls -la kaos64.img

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
echo "  qemu-system-x86_64 -drive format=raw,file=kaos64.img -display curses"
echo ""
