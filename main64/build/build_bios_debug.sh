#!/bin/bash
# build_bios_debug.sh - Build the KAOS Rust Kernel and bootloaders (debug) and deploy to UTM.
#
# This script compiles the 16-bit entry loader, 64-bit loader, kernel, and user programs in debug mode.
# It creates a raw FAT32 superfloppy disk image (kaos64-bios.img), converts it to a QCOW2 image (kaos64.qcow2),
# copies it to UTM (if on macOS), and launches QEMU.
#
# Required tools: nasm, cargo (Rust nightly target x86_64-unknown-none), cargo-binutils (cargo objcopy),
# qemu-img, and mtools.
#
# Arguments:
#   Any arguments passed to this script will be forwarded directly to qemu-system-x86_64.
#   Environment variable DISPLAY_MODE:
#     - gui    : show graphic window + serial console in terminal (default on macOS)
#     - serial : text serial console only (default headless / SSH / dev container)
#     - vnc    : VNC server on :0 (port 5900) + serial in terminal
#
# Example:
#   ./build/build_bios_debug.sh -s -S   # start QEMU suspended, listening for GDB on port 1234
#   DISPLAY_MODE=serial ./build/build_bios_debug.sh

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
rm -f kaos64-bios.img

echo "  -> Creating FAT32 disk image (superfloppy)..."
"$SCRIPT_DIR/helper_make_fat32_bios_image.sh" "target/x86_64-unknown-none/debug"

echo ""
echo "  -> Disk image created successfully!"
ls -la kaos64-bios.img

echo "  -> Creating qcow2 image for UTM..."
cd "$PROJECT_ROOT"
qemu-img convert -O qcow2 kaos64-bios.img kaos64.qcow2 
cp kaos64.qcow2 "$HOME/Library/Containers/com.utmapp.UTM/Data/Documents/KAOS x64 BIOS.utm/Data/kaos64.qcow2"
rm -f kaos64.qcow2
echo ""
echo "  -> qcow2 image created and deployed to UTM successfully!"

echo ""
echo "========================================"
echo "  Build Complete!"
echo "========================================"
echo ""
echo "Output files:"
echo "  - main64/kaos64-bios.img (bootable disk image)"
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
    -drive format=raw,file="kaos64-bios.img" \
    -netdev user,id=net0,net=192.168.1.0/24,dhcpstart=192.168.1.200,host=192.168.1.1,dns=192.168.1.3 -device rtl8139,netdev=net0 \
    "${QEMU_DISPLAY[@]}" \
    -m 256M \
    "$@"
