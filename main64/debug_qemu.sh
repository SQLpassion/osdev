#!/bin/bash
# debug_qemu.sh - run it on the host OS
# Starts QEMU, opens the GDB server on port 12345, and stops the CPU prior the boot (-S)

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

qemu-system-x86_64 \
    -drive format=raw,file=kaos64.img \
    -gdb tcp::12345 -S \
    -m 256M
