#!/bin/bash
# Build script for user-mode programs stored in main64/user_programs.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROFILE="${1:-debug}"

if [ "$PROFILE" != "debug" ] && [ "$PROFILE" != "release" ]; then
    echo "Usage: $0 [debug|release]"
    exit 1
fi

HELLO_DIR="$SCRIPT_DIR/user_programs/hello"
cd "$HELLO_DIR"

echo "========================================"
echo "  Building user programs ($PROFILE)"
echo "========================================"
echo ""
echo "-> Building hello user program..."

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core
    INPUT_ELF="target/x86_64-unknown-none/release/hello"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="target/x86_64-unknown-none/debug/hello"
fi

llvm-objcopy -O binary "$INPUT_ELF" hello.bin 2>/dev/null || \
    rust-objcopy -O binary "$INPUT_ELF" hello.bin 2>/dev/null || \
    objcopy -O binary "$INPUT_ELF" hello.bin

echo "-> Built: $HELLO_DIR/hello.bin"
ls -la hello.bin
