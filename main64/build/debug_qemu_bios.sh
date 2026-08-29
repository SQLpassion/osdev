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

qemu-system-x86_64 \
    -drive format=raw,file=kaos64-bios.img \
    -netdev user,id=net0,net=192.168.1.0/24,dhcpstart=192.168.1.200,host=192.168.1.1,dns=192.168.1.3 -device rtl8139,netdev=net0 \
    -gdb tcp::12345 -S \
    -m 256M
