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
READLINE_DIR="$SCRIPT_DIR/user_programs/readline"

echo "========================================"
echo "  Building user programs ($PROFILE)"
echo "========================================"
echo ""
echo "-> Building hello user program..."

cd "$HELLO_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
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

echo ""
echo "-> Building readline user program..."

cd "$READLINE_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="target/x86_64-unknown-none/release/readline"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="target/x86_64-unknown-none/debug/readline"
fi

llvm-objcopy -O binary "$INPUT_ELF" readline.bin 2>/dev/null || \
    rust-objcopy -O binary "$INPUT_ELF" readline.bin 2>/dev/null || \
    objcopy -O binary "$INPUT_ELF" readline.bin

echo "-> Built: $READLINE_DIR/readline.bin"
ls -la readline.bin

FILEDEMO_DIR="$SCRIPT_DIR/user_programs/filedemo"
echo ""
echo "-> Building filedemo user program..."

cd "$FILEDEMO_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="target/x86_64-unknown-none/release/filedemo"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="target/x86_64-unknown-none/debug/filedemo"
fi

llvm-objcopy -O binary "$INPUT_ELF" filedemo.bin 2>/dev/null || \
    rust-objcopy -O binary "$INPUT_ELF" filedemo.bin 2>/dev/null || \
    objcopy -O binary "$INPUT_ELF" filedemo.bin

echo "-> Built: $FILEDEMO_DIR/filedemo.bin"
ls -la filedemo.bin

SHELL_DIR="$SCRIPT_DIR/user_programs/shell"
echo ""
echo "-> Building shell user program..."

cd "$SHELL_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="target/x86_64-unknown-none/release/shell"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="target/x86_64-unknown-none/debug/shell"
fi

llvm-objcopy -O binary "$INPUT_ELF" shell.bin 2>/dev/null || \
    rust-objcopy -O binary "$INPUT_ELF" shell.bin 2>/dev/null || \
    objcopy -O binary "$INPUT_ELF" shell.bin

echo "-> Built: $SHELL_DIR/shell.bin"
ls -la shell.bin

TUI_DIR="$SCRIPT_DIR/user_programs/tui_app"
echo ""
echo "-> Building tui user program..."

cd "$TUI_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="target/x86_64-unknown-none/release/tui"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="target/x86_64-unknown-none/debug/tui"
fi

llvm-objcopy -O binary "$INPUT_ELF" tui.bin 2>/dev/null || \
    rust-objcopy -O binary "$INPUT_ELF" tui.bin 2>/dev/null || \
    objcopy -O binary "$INPUT_ELF" tui.bin

echo "-> Built: $TUI_DIR/tui.bin"
ls -la tui.bin


KBASIC_DIR="$SCRIPT_DIR/user_programs/kbasic"
echo ""
echo "-> Building kbasic user program..."

cd "$KBASIC_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="target/x86_64-unknown-none/release/kbasic"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="target/x86_64-unknown-none/debug/kbasic"
fi

llvm-objcopy -O binary "$INPUT_ELF" kbasic.bin 2>/dev/null || \
    rust-objcopy -O binary "$INPUT_ELF" kbasic.bin 2>/dev/null || \
    objcopy -O binary "$INPUT_ELF" kbasic.bin

echo "-> Built: $KBASIC_DIR/kbasic.bin"
ls -la kbasic.bin


