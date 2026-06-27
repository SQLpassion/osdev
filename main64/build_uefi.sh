#!/bin/bash
# build_uefi.sh - build the KAOS UEFI loader (kaosldr_uefi), produce a bootable disk image,
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

PROFILE="debug"
TARGET="x86_64-unknown-uefi"
EFI_BIN="target/$TARGET/$PROFILE/bootx64.efi"
IMG="kaos64-uefi.img"

# 1) Build the kernel and loader (produces kernel.bin and bootx64.efi).
echo "==> Building kernel..."
( cd kernel && cargo build && cargo objcopy -- -O binary ../target/x86_64-unknown-none/debug/kernel.bin )

echo "==> Building kaosldr_uefi ($TARGET, $PROFILE)..."
( cd kaosldr_uefi && cargo build )

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
mcopy   -i "$IMG@@$PART_OFFSET" "target/x86_64-unknown-none/debug/kernel.bin" ::/KERNEL.BIN
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

# A writable copy of the variable store is required for pflash, and it must match the code file's
# flash size. First try the CODE->VARS counterpart in the same directory (correct for the _4M
# variants); then fall back to common vars names.
OVMF_VARS_SRC=""
VARS_CANDIDATE="${OVMF_CODE/CODE/VARS}"
if [ "$VARS_CANDIDATE" != "$OVMF_CODE" ] && [ -f "$VARS_CANDIDATE" ]; then
    OVMF_VARS_SRC="$VARS_CANDIDATE"
else
    for vars in OVMF_VARS_4M.fd OVMF_VARS.fd edk2-i386-vars.fd edk2-x86_64-vars.fd; do
        if [ -f "$OVMF_DIR/$vars" ]; then OVMF_VARS_SRC="$OVMF_DIR/$vars"; break; fi
    done
fi
if [ -z "$OVMF_VARS_SRC" ]; then
    echo "ERROR: Found OVMF code ($OVMF_CODE) but no matching vars file." >&2
    exit 1
fi
OVMF_VARS="kaosldr_uefi/ovmf_vars.fd"
cp "$OVMF_VARS_SRC" "$OVMF_VARS"

# 4) Choose how QEMU presents output.
#   gui    - graphical QEMU window plus serial on the terminal. The window backend is picked
#            per OS: cocoa (macOS), gtk (Linux / Windows). Override with GUI_BACKEND=sdl etc.
#   serial - headless: no window, the loader's COM1 output goes to this terminal. The right
#            choice in a headless dev container or over SSH (matches the test runner).
#   vnc    - headless but exposes the graphical framebuffer on VNC :0 (port 5900); connect a
#            VNC viewer. Use this to *see* the GOP framebuffer from inside a container.
# Override with e.g. `DISPLAY_MODE=serial ./build_uefi.sh`.
#
# The 'auto' default picks 'gui' whenever a desktop is available and 'serial' otherwise:
#   - macOS / Windows                      -> gui (always has a desktop)
#   - Linux with $DISPLAY or $WAYLAND_DISPLAY set -> gui (desktop session)
#   - Linux without a display (container, SSH)    -> serial
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
    -m 256M
