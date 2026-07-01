#!/bin/bash
# debug_qemu_bios.sh - Run the legacy BIOS disk image in QEMU under GDB debugging control.
#
# This script starts QEMU with the raw BIOS disk image (kaos64.img), exposes a GDB remote
# debugging server on TCP port 12345, and pauses execution at boot (-S) waiting for GDB to connect.
#
# Required tools: qemu-system-x86_64.

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

qemu-system-x86_64 \
    -drive format=raw,file=kaos64.img \
    -gdb tcp::12345 -S \
    -m 256M
