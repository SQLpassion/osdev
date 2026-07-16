#!/bin/bash
# helper_make_fat32_bios_image.sh - Build the bootable legacy-BIOS FAT32 superfloppy (kaos64.img).
#
# This script formats a raw 64 MiB file as a FAT32 superfloppy with an enlarged reserved region,
# copies the kernel and user programs as standard files, writes the early loaders at fixed LBAs
# in the reserved area, and overlays our custom boot sector onto the VBR while preserving the BPB.
#
# Usage: helper_make_fat32_bios_image.sh <profile-target-dir>
#   <profile-target-dir>  e.g. "target/x86_64-unknown-none/debug" or "release"
#
# Required tools: mtools (mformat, mcopy), dd.
# See docs/todo_fat32_unification.md for architecture details.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"
cd "$PROJECT_ROOT"

PROFILE_DIR="${1:?usage: helper_make_fat32_bios_image.sh <profile-target-dir>}"
IMG=kaos64.img

# Disk layout constants - MUST match boot/bootsector.asm (KLDR*_LBA / *_MAX_SECTORS).
RESERVED_SECTORS=64
# NOTE: KLDR64_MAX_SECTORS must keep 0x3000 + n*512 <= 0x7C00 (the boot sector's own
# load address), i.e. n <= 38. See boot/bootsector.asm for the rationale.
KLDR16_LBA=8
KLDR16_MAX_SECTORS=8
KLDR64_LBA=16
KLDR64_MAX_SECTORS=32

KLDR16_BIN=kaosldr_16/kldr16.bin
KLDR64_BIN="$PROFILE_DIR/kldr64.bin"
KERNEL_BIN="$PROFILE_DIR/kernel.bin"
BOOTSECTOR_BIN=boot/bootsector.bin

# Guard: each early loader must fit inside its fixed reserved-sector slot, otherwise it
# would silently spill into the following loader / the FAT.
check_fits() {
  sz=$(wc -c < "$1")
  secs=$(( (sz + 511) / 512 ))
  if [ "$secs" -gt "$2" ]; then
    echo "ERROR: $1 is $secs sectors but its reserved slot holds only $2 sectors" >&2
    exit 1
  fi
}
check_fits "$KLDR16_BIN" "$KLDR16_MAX_SECTORS"
check_fits "$KLDR64_BIN" "$KLDR64_MAX_SECTORS"

rm -f "$IMG"

# 1) 64 MiB backing file so the volume is unambiguously FAT32 (>= 65525 data clusters).
dd if=/dev/zero of="$IMG" bs=1048576 count=64 2>/dev/null

# 2) FAT32 with an enlarged reserved region (holds VBR, FSInfo, backup boot + loaders).
mformat -i "$IMG" -F -R "$RESERVED_SECTORS" ::

# 3) Normal FAT32 files, stored under 8.3 uppercase names as the kernel/shell expect.
mcopy -i "$IMG" "$KERNEL_BIN"                       ::/KERNEL.BIN
mcopy -i "$IMG" user_programs/hello/hello.bin       ::/HELLO.BIN
mcopy -i "$IMG" user_programs/readline/readline.bin ::/READLINE.BIN
mcopy -i "$IMG" user_programs/filedemo/filedemo.bin ::/FILEDEMO.BIN
mcopy -i "$IMG" user_programs/exception_test/except.bin ::/EXCEPT.BIN
mcopy -i "$IMG" user_programs/shell/shell.bin       ::/SHELL.BIN
mcopy -i "$IMG" user_programs/tui_app/tui.bin       ::/TUI.BIN
mcopy -i "$IMG" user_programs/kbasic/kbasic.bin     ::/KBASIC.BIN
mcopy -i "$IMG" SFile.txt                           ::/SFILE.TXT
mcopy -i "$IMG" BigFile.txt                         ::/BIGFILE.TXT
mcopy -i "$IMG" user_programs/kbasic/src/demo.bas   ::/DEMO.BAS

# 4) Write the two early loaders to their fixed reserved LBAs (read by the boot sector).
dd if="$KLDR16_BIN" of="$IMG" bs=512 seek="$KLDR16_LBA" conv=notrunc 2>/dev/null
dd if="$KLDR64_BIN" of="$IMG" bs=512 seek="$KLDR64_LBA" conv=notrunc 2>/dev/null

# 5) Overlay our boot code onto the VBR while preserving mformat's authoritative FAT32 BPB:
#    save the BPB fields (0x0B..0x5A = 79 bytes), write our full boot sector, restore them.
#    Result: BPB = mformat (correct geometry), code + JMP@0x00 + 0x55AA signature = ours.
dd if="$IMG" of=bpb_save.bin bs=1 skip=11 count=79 2>/dev/null
dd if="$BOOTSECTOR_BIN" of="$IMG" bs=512 count=1 conv=notrunc 2>/dev/null
dd if=bpb_save.bin of="$IMG" bs=1 seek=11 count=79 conv=notrunc 2>/dev/null
rm -f bpb_save.bin
