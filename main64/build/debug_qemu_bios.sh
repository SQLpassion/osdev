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

BRIDGE_IFNAME="${BRIDGE_IFNAME:-en0}"
NET_MODE="${NET_MODE:-bridged}"

SUDO_PREFIX=()
case "$NET_MODE" in
    bridged)
        if [ "$OS_KIND" = "macos" ]; then
            QEMU_NET=(-netdev "vmnet-bridged,id=net0,ifname=$BRIDGE_IFNAME" -device rtl8139,netdev=net0)
            if [ "$(id -u)" -ne 0 ]; then
                SUDO_PREFIX=(sudo)
            fi
        else
            QEMU_NET=(-netdev "tap,id=net0,ifname=tap0,script=no,downscript=no" -device rtl8139,netdev=net0)
        fi
        ;;
    user|nat)
        QEMU_NET=(-netdev "user,id=net0,net=192.168.1.0/24,dhcpstart=192.168.1.200,host=192.168.1.1,dns=192.168.1.3" -device rtl8139,netdev=net0)
        ;;
    *)
        echo "ERROR: unknown NET_MODE='$NET_MODE' (expected: bridged | user)." >&2
        exit 1
        ;;
esac

"${SUDO_PREFIX[@]}" qemu-system-x86_64 \
    -drive format=raw,file=kaos64-bios.img \
    "${QEMU_NET[@]}" \
    -gdb tcp::12345 -S \
    -m 256M
