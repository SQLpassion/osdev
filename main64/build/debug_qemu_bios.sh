#!/bin/bash
# debug_qemu_bios.sh - Run the legacy BIOS disk image in QEMU under GDB debugging control.
#
# This script starts QEMU with the raw BIOS disk image (kaos64-bios.img), exposes a GDB remote
# debugging server on TCP port 12345, and pauses execution at boot (-S) waiting for GDB to connect.
#
# Required tools: qemu-system-x86_64.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

case "$(uname -s)" in
    Darwin) OS_KIND="macos" ;;
    *)      OS_KIND="linux" ;;
esac

# Network backend configuration (always bridged network)
SUDO_PREFIX=()
if [ "$OS_KIND" = "macos" ]; then
    QEMU_NET=(-netdev vmnet-bridged,id=net0,ifname=en0 -device rtl8139,netdev=net0)
    if [ "$(id -u)" -ne 0 ]; then
        SUDO_PREFIX=(sudo)
    fi
else
    QEMU_NET=(-netdev tap,id=net0,ifname=tap0,script=no,downscript=no -device rtl8139,netdev=net0)
fi

"${SUDO_PREFIX[@]}" qemu-system-x86_64 \
    -drive format=raw,file=kaos64-bios.img \
    "${QEMU_NET[@]}" \
    -gdb tcp::12345 -S \
    -m 256M
