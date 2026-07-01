#!/bin/bash
# build_bios_debug.sh - Build the KAOS Rust Kernel and bootloaders (debug) and deploy to UTM.
#
# This script compiles the 16-bit entry loader, 64-bit loader, kernel, and user programs in debug mode.
# It creates a raw FAT32 superfloppy disk image (kaos64.img), converts it to a QCOW2 image (kaos64.qcow2),
# and copies/deploys the resulting QCOW2 image directly into the local macOS UTM application's VM directory.
#
# Required tools: nasm, cargo (Rust nightly target x86_64-unknown-none), cargo-binutils (cargo objcopy),
# mtools, and qemu (for qemu-img). All are preinstalled in the dev container; on macOS install them with:
#   brew install nasm mtools qemu
#   rustup component add llvm-tools-preview
#   cargo install cargo-binutils

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
"$SCRIPT_DIR/helper_build_user_programs.sh" debug
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
"$SCRIPT_DIR/helper_make_fat32_bios_image.sh" "target/x86_64-unknown-none/debug"

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
# 4) Choose how QEMU presents output.
case "$(uname -s)" in
    Darwin)               OS_KIND="macos";   GUI_BACKEND_DEFAULT="cocoa" ;;
    MINGW*|MSYS*|CYGWIN*) OS_KIND="windows"; GUI_BACKEND_DEFAULT="gtk"   ;;
    *)                    OS_KIND="linux";   GUI_BACKEND_DEFAULT="gtk"   ;;
esac
GUI_BACKEND="${GUI_BACKEND:-$GUI_BACKEND_DEFAULT}"

DISPLAY_MODE="${DISPLAY_MODE:-auto}"
if [ "$DISPLAY_MODE" = "auto" ]; then
    if [ "$OS_KIND" = "macos" ] || [ "$OS_KIND" = "windows" ]; then
        DISPLAY_MODE="gui"
    elif [ -n "${DISPLAY:-}" ] || [ -n "${WAYLAND_DISPLAY:-}" ]; then
        DISPLAY_MODE="gui"   # Linux desktop session
    else
        DISPLAY_MODE="serial"  # headless Linux (dev container, SSH)
    fi
fi

case "$DISPLAY_MODE" in
    gui)
        QEMU_DISPLAY=(-display "$GUI_BACKEND" -serial stdio)
        DISPLAY_HINT="$GUI_BACKEND window + serial on this terminal"
        ;;
    serial)
        QEMU_DISPLAY=(-serial stdio -display none)
        DISPLAY_HINT="serial on this terminal (headless)"
        ;;
    vnc)
        QEMU_DISPLAY=(-display none -vnc :0 -serial stdio)
        DISPLAY_HINT="VNC on :0 (port 5900) + serial on this terminal"
        ;;
    *)
        echo "ERROR: unknown DISPLAY_MODE='$DISPLAY_MODE' (expected: gui | serial | vnc)." >&2
        exit 1
        ;;
esac

# 5) Boot it. (Ctrl-A X quits QEMU when serial is attached to the terminal.)
echo "==> Launching QEMU [$DISPLAY_MODE: $DISPLAY_HINT]..."
qemu-system-x86_64 \
    -drive format=raw,file="kaos64.img" \
    "${QEMU_DISPLAY[@]}" \
    -m 256M \
    "$@"
