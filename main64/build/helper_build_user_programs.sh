#!/bin/bash
# helper_build_user_programs.sh - Build KAOS user-mode programs (hello, readline, filedemo, exception test, shell, tui, kbasic).
#
# This script compiles all user-mode applications located in the user_programs/ subdirectories
# for the x86_64-unknown-none target (using debug or release profiles) and copies the resulting
# ELF64 static executable directly for filesystem inclusion -- the kernel's loader reads ELF
# program headers directly, so the former objcopy-to-flat-binary step is gone.
#
# Required tools: cargo (Rust nightly target x86_64-unknown-none).

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
PROFILE="${1:-debug}"

if [ "$PROFILE" != "debug" ] && [ "$PROFILE" != "release" ]; then
    echo "Usage: $0 [debug|release]"
    exit 1
fi

# Strips DWARF debug info from a copied ELF binary in place. Debug builds can
# otherwise easily approach the 2 MiB USER_CODE window (process::
# USER_PROGRAM_MAX_IMAGE_SIZE) once every program ships its full ELF instead
# of an objcopy'd flat blob; PT_LOAD segments (the only thing the kernel's
# loader reads) are untouched by --strip-debug, only the non-allocated debug
# sections are removed (debug info, symbol tables -- neither is read by the loader).
strip_debug_info() {
    llvm-strip --strip-debug "$1" 2>/dev/null || \
        rust-strip --strip-debug "$1" 2>/dev/null || \
        strip --strip-debug "$1"
}

HELLO_DIR="$PROJECT_ROOT/user_programs/hello"
READLINE_DIR="$PROJECT_ROOT/user_programs/readline"

echo "========================================"
echo "  Building user programs ($PROFILE)"
echo "========================================"
echo ""
echo "-> Building hello user program..."

cd "$HELLO_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/hello"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/hello"
fi

cp "$INPUT_ELF" hello.bin
strip_debug_info hello.bin

echo "-> Built: $HELLO_DIR/hello.bin"
ls -la hello.bin

echo ""
echo "-> Building readline user program..."

cd "$READLINE_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/readline"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/readline"
fi

cp "$INPUT_ELF" readline.bin
strip_debug_info readline.bin

echo "-> Built: $READLINE_DIR/readline.bin"
ls -la readline.bin

FILEDEMO_DIR="$PROJECT_ROOT/user_programs/filedemo"
echo ""
echo "-> Building filedemo user program..."

cd "$FILEDEMO_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/filedemo"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/filedemo"
fi

cp "$INPUT_ELF" filedemo.bin
strip_debug_info filedemo.bin

echo "-> Built: $FILEDEMO_DIR/filedemo.bin"
ls -la filedemo.bin

EXCEPTION_TEST_DIR="$PROJECT_ROOT/user_programs/exception_test"
echo ""
echo "-> Building exception exerciser user program..."

cd "$EXCEPTION_TEST_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/except"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/except"
fi

cp "$INPUT_ELF" except.bin
strip_debug_info except.bin

echo "-> Built: $EXCEPTION_TEST_DIR/except.bin"
ls -la except.bin

SHELL_DIR="$PROJECT_ROOT/user_programs/shell"
echo ""
echo "-> Building shell user program..."

cd "$SHELL_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/shell"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/shell"
fi

cp "$INPUT_ELF" shell.bin
strip_debug_info shell.bin

echo "-> Built: $SHELL_DIR/shell.bin"
ls -la shell.bin

TUI_DIR="$PROJECT_ROOT/user_programs/tui_app"
echo ""
echo "-> Building tui user program..."

cd "$TUI_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/tui"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/tui"
fi

cp "$INPUT_ELF" tui.bin
strip_debug_info tui.bin

echo "-> Built: $TUI_DIR/tui.bin"
ls -la tui.bin


KBASIC_DIR="$PROJECT_ROOT/user_programs/kbasic"
echo ""
echo "-> Building kbasic user program..."

cd "$KBASIC_DIR"

if [ "$PROFILE" = "release" ]; then
    cargo +nightly build --release --target x86_64-unknown-none -Z build-std=core,alloc
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/release/kbasic"
else
    cargo +nightly build --target x86_64-unknown-none
    INPUT_ELF="$PROJECT_ROOT/target/x86_64-unknown-none/debug/kbasic"
fi

cp "$INPUT_ELF" kbasic.bin
strip_debug_info kbasic.bin

echo "-> Built: $KBASIC_DIR/kbasic.bin"
ls -la kbasic.bin

