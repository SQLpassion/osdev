#!/bin/bash
# build_uefi_release.sh - build the KAOS UEFI loader (kaosldr_uefi) in Release mode, produce a bootable disk image,
# and boot it in QEMU under OVMF.
#
# This builds a real GPT disk image with a FAT32 EFI System Partition, kaos64-uefi.img, holding
# /EFI/BOOT/BOOTX64.EFI. The same image boots in QEMU here AND can be written 1:1 to a USB stick
# for real hardware (see docs/uefi.md).
#
# Required host tools: a Rust nightly with the x86_64-unknown-uefi target, QEMU + OVMF, and
# gptfdisk (sgdisk) + mtools. All are preinstalled in the dev container; on macOS install them
# with `brew install qemu gptfdisk mtools`.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

PROFILE="release"
TARGET="x86_64-unknown-uefi"
EFI_BIN="target/$TARGET/$PROFILE/bootx64.efi"
IMG="kaos64-uefi.img"

# 1) Build the kernel and loader (produces kernel.bin and bootx64.efi).
echo "==> Building kernel (release)..."
( cd kernel && cargo build --release && cargo objcopy --release -- -O binary ../target/x86_64-unknown-none/release/kernel.bin )

echo "==> Building kaosldr_uefi ($TARGET, $PROFILE)..."
( cd kaosldr_uefi && cargo build --release )

echo "==> Building user-mode programs (release)..."
"$SCRIPT_DIR/helper_build_user_programs.sh" "$PROFILE"

# 2) Build the bootable GPT/ESP disk image (kaos64-uefi.img): a GPT disk with one FAT32 EFI
# System Partition holding /EFI/BOOT/BOOTX64.EFI and /KERNEL.BIN.
echo "==> Creating bootable GPT/ESP image $IMG ..."
IMG_SIZE_MB=128
PART_OFFSET=1M                         # the ESP starts at sector 2048 (= 1 MiB)
rm -f "$IMG"
# Create the backing file. dd is portable across Linux and macOS (avoids GNU `truncate`).
dd if=/dev/zero of="$IMG" bs=1048576 count="$IMG_SIZE_MB" 2>/dev/null
# GPT with a single EFI System Partition (type ef00) spanning the rest of the disk.
sgdisk --clear \
       --new=1:2048:0 --typecode=1:ef00 --change-name=1:"EFI System Partition" \
       "$IMG" >/dev/null
# Format that partition as FAT32 and populate it (mtools' image@@offset syntax; no root needed).
mformat -i "$IMG@@$PART_OFFSET" -F ::
mmd     -i "$IMG@@$PART_OFFSET" ::/EFI ::/EFI/BOOT
mcopy   -i "$IMG@@$PART_OFFSET" "$EFI_BIN" ::/EFI/BOOT/BOOTX64.EFI
mcopy   -i "$IMG@@$PART_OFFSET" "target/x86_64-unknown-none/release/kernel.bin" ::/KERNEL.BIN
# User-mode programs. SHELL.BIN is the root shell the kernel runs on the UEFI
# path; the others are launched from within the shell (matching the legacy
# FAT32 image populated by build_bios_debug.sh / build_bios_debug_devcontainer.sh). 8.3 uppercase names, as stored by mcopy.
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/shell/shell.bin"       ::/SHELL.BIN
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/hello/hello.bin"       ::/HELLO.BIN
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/readline/readline.bin" ::/READLINE.BIN
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/filedemo/filedemo.bin" ::/FILEDEMO.BIN
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/tui_app/tui.bin"       ::/TUI.BIN
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/kbasic/kbasic.bin"     ::/KBASIC.BIN
mcopy   -i "$IMG@@$PART_OFFSET" "user_programs/kbasic/src/demo.bas"   ::/DEMO.BAS
echo "==> $IMG ready. Flash to a USB stick with (DESTRUCTIVE - pick the right device!):"
echo "        sudo dd if=$IMG of=/dev/<your-usb> bs=4M conv=fsync"

# 3) Locate the OVMF firmware (UEFI for QEMU). Honor a manually provided OVMF_CODE first;
# otherwise search the usual locations on macOS, Linux and Windows. Firmware file names vary:
# edk2-x86_64-code.fd (Homebrew / Windows QEMU), OVMF_CODE_4M.fd (Ubuntu 24.04 `ovmf` package),
# OVMF_CODE.fd (older distros).
if [ -z "${OVMF_CODE:-}" ]; then
    SEARCH_DIRS=()
    # macOS Homebrew.
    if command -v brew >/dev/null 2>&1; then
        SEARCH_DIRS+=("$(brew --prefix qemu 2>/dev/null)/share/qemu")
    fi
    # Relative to the QEMU binary (covers Windows installs and non-standard prefixes).
    QEMU_PATH="$(command -v qemu-system-x86_64 || true)"
    if [ -n "$QEMU_PATH" ]; then
        QEMU_BIN_DIR="$(cd "$(dirname "$QEMU_PATH")" && pwd)"
        SEARCH_DIRS+=("$QEMU_BIN_DIR" "$QEMU_BIN_DIR/share" "$QEMU_BIN_DIR/../share/qemu")
    fi
    # Common Linux locations.
    SEARCH_DIRS+=(/usr/share/qemu /usr/share/OVMF /usr/share/edk2/x64 /usr/share/edk2-ovmf/x64)

    for dir in "${SEARCH_DIRS[@]}"; do
        [ -z "$dir" ] && continue
        for code in edk2-x86_64-code.fd OVMF_CODE_4M.fd OVMF_CODE.fd; do
            if [ -f "$dir/$code" ]; then OVMF_CODE="$dir/$code"; break 2; fi
        done
    done
fi

if [ -z "${OVMF_CODE:-}" ] || [ ! -f "$OVMF_CODE" ]; then
    echo "ERROR: Could not find OVMF firmware (edk2-x86_64-code.fd / OVMF_CODE_4M.fd / OVMF_CODE.fd)." >&2
    echo "       Install it: 'apt-get install ovmf' (Linux), 'brew install qemu' (macOS)," >&2
    echo "       or set OVMF_CODE=/path/to/OVMF_CODE.fd manually (e.g. on Windows)." >&2
    exit 1
fi
OVMF_DIR="$(cd "$(dirname "$OVMF_CODE")" && pwd)"
OVMF_VARS_SRC=""
VARS_CANDIDATE="${OVMF_CODE/CODE/VARS}"
if [ "$VARS_CANDIDATE" != "$OVMF_CODE" ] && [ -f "$VARS_CANDIDATE" ]; then
    OVMF_VARS_SRC="$VARS_CANDIDATE"
else
    for vars in OVMF_VARS_4M.fd OVMF_VARS.fd edk2-i386-vars.fd edk2-x86_64-vars.fd; do
        if [ -f "$OVMF_DIR/$vars" ]; then OVMF_VARS_SRC="$OVMF_DIR/$vars"; break; fi
    done
fi
OVMF_CODE_DIR="$(cd "$(dirname "$OVMF_CODE")" && pwd)"
if [ -z "$OVMF_VARS_SRC" ]; then
    for vars in OVMF_VARS_4M.fd OVMF_VARS.fd edk2-i386-vars.fd edk2-x86_64-vars.fd; do
        if [ -f "$OVMF_CODE_DIR/$vars" ]; then OVMF_VARS_SRC="$OVMF_CODE_DIR/$vars"; break; fi
    done
fi
if [ -z "$OVMF_VARS_SRC" ]; then
    echo "ERROR: Found OVMF code ($OVMF_CODE) but no matching vars file." >&2
    exit 1
fi
OVMF_VARS="kaosldr_uefi/ovmf_vars.fd"
cp "$OVMF_VARS_SRC" "$OVMF_VARS"

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
echo "==> OVMF code: $OVMF_CODE"
echo "==> Launching QEMU [$DISPLAY_MODE: $DISPLAY_HINT]..."
qemu-system-x86_64 \
    -machine q35 \
    -drive if=pflash,format=raw,readonly=on,file="$OVMF_CODE" \
    -drive if=pflash,format=raw,file="$OVMF_VARS" \
    -drive format=raw,file="$IMG" \
    -vga virtio \
    "${QEMU_DISPLAY[@]}" \
    -net none \
    -m 256M \
    "$@"
