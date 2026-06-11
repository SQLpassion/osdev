#!/bin/bash
# Build script for KAOS Rust Kernel (Release Build)
# This script builds the Rust kernel in release mode locally and uses Docker for bootloaders

set -e  # Exit on error

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

echo "========================================"
echo "  KAOS Rust Kernel Build Script"
echo "  (Release Build)"
echo "========================================"
echo ""

# Step 1: Build Rust kernel locally (release mode)
echo "[1/3] Building Rust kernel locally (release)..."
echo "--------------------------------------"
cd kernel

echo "  -> Running cargo build (release, rebuilding core/alloc with -Z build-std)..."
cargo build --release -Z build-std=core,alloc

echo "  -> Extracting flat binary with rust-objcopy..."
rust-objcopy -O binary target/x86_64-unknown-none/release/kaos_kernel target/x86_64-unknown-none/release/kernel.bin

echo "  -> Rust kernel built: kernel/target/x86_64-unknown-none/release/kernel.bin"
ls -la target/x86_64-unknown-none/release/kernel.bin

cd "$SCRIPT_DIR"
echo ""

# Step 1b: Build Rust 64-bit kernel loader locally (release mode)
echo "[1b/3] Building Rust 64-bit kernel loader locally (release)..."
echo "--------------------------------------"
cd kaosldr_64

echo "  -> Running cargo build (release, rebuilding core with -Z build-std)..."
cargo build --release -Z build-std=core

echo "  -> Extracting flat binary with rust-objcopy..."
rust-objcopy -O binary target/x86_64-unknown-none/release/kldr64 target/x86_64-unknown-none/release/kldr64.bin

echo "  -> Rust kernel loader built: kaosldr_64/target/x86_64-unknown-none/release/kldr64.bin"
ls -la target/x86_64-unknown-none/release/kldr64.bin

cd "$SCRIPT_DIR"
echo ""

# Step 2: Build user-mode programs
echo "[2/3] Building user-mode programs..."
echo "------------------------------------"
"$SCRIPT_DIR/build_user_programs.sh" release
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

echo "  -> Removing old disk image if exists..."
rm -f kaos64.img

echo "  -> Creating FAT12 disk image..."
fat_imgen -c -s boot/bootsector.bin -f kaos64.img
fat_imgen -m -f kaos64.img -i kaosldr_16/kldr16.bin
fat_imgen -m -f kaos64.img -i kaosldr_64/target/x86_64-unknown-none/release/kldr64.bin
fat_imgen -m -f kaos64.img -i kernel/target/x86_64-unknown-none/release/kernel.bin
fat_imgen -m -f kaos64.img -i user_programs/hello/hello.bin -n HELLO.BIN
fat_imgen -m -f kaos64.img -i user_programs/readline/readline.bin -n READLINE.BIN
fat_imgen -m -f kaos64.img -i user_programs/filedemo/filedemo.bin -n FILEDEMO.BIN
fat_imgen -m -f kaos64.img -i user_programs/shell/shell.bin -n SHELL.BIN
fat_imgen -m -f kaos64.img -i user_programs/tui_app/tui.bin -n TUI.BIN
fat_imgen -m -f kaos64.img -i user_programs/kbasic/kbasic.bin -n KBASIC.BIN
fat_imgen -m -f kaos64.img -i SFile.txt
fat_imgen -m -f kaos64.img -i BigFile.txt
fat_imgen -m -f kaos64.img -i user_programs/kbasic/src/demo.bas



echo ""
echo "  -> Disk image created successfully!"
ls -la kaos64.img
'

echo "  -> Creating qcow2 image for UTM..."
cd "$SCRIPT_DIR"
qemu-img convert -O qcow2 kaos64.img kaos64.qcow2 
cp kaos64.qcow2 "$HOME/Library/Containers/com.utmapp.UTM/Data/Documents/KAOS x64.utm/Data/kaos64.qcow2"
echo ""
echo "  -> qcow2 image created successfully!"
ls -la kaos64.qcow2

echo ""
echo "========================================"
echo "  Release Build Complete!"
echo "========================================"
echo ""
echo "Output files:"
echo "  - main64/kaos64.img (bootable disk image)"
echo "  - main64/kernel/target/x86_64-unknown-none/release/kernel.bin"
echo ""
echo "To run in QEMU:"
echo "  qemu-system-x86_64 -drive format=raw,file=kaos64.img"
echo ""
