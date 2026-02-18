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
echo "[1/3] Building Rust kernel locally..."
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

# Step 2: Build user-mode programs
echo "[2/3] Building user-mode programs..."
echo "------------------------------------"
"$SCRIPT_DIR/build_user_programs.sh" debug
echo ""

# Step 3: Build bootloaders and create disk image in Docker
echo "[3/3] Building bootloaders and disk image in Docker..."
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
fat_imgen -m -f kaos64_rust.img -i user_programs/hello/hello.bin -n HELLO.BIN
fat_imgen -m -f kaos64_rust.img -i SFile.txt
fat_imgen -m -f kaos64_rust.img -i BigFile.txt

echo ""
echo "  -> Disk image created successfully!"
ls -la kaos64_rust.img
'

# echo "  -> Creating qcow2 image for UTM..."
# cd "$SCRIPT_DIR"
# qemu-img convert -O qcow2 kaos64_rust.img kaos64_rust.qcow2 
# cp kaos64_rust.qcow2 "$HOME/Library/Containers/com.utmapp.UTM/Data/Documents/KAOS x64.utm/Data/kaos64.qcow2"
# echo ""
# echo "  -> qcow2 image created successfully!"
# ls -la kaos64_rust.qcow2

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
