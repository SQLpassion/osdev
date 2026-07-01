#!/bin/bash
# make_fat32_image.sh - build the bootable legacy-BIOS FAT32 superfloppy (kaos64.img).
#
# Usage: make_fat32_image.sh <profile-target-dir>
#   <profile-target-dir>  e.g. "target/x86_64-unknown-none/debug"
#                         or   "target/x86_64-unknown-none/release"
#
# Prerequisites (already produced by the calling build script):
#   - boot/bootsector.bin           (assembled FAT32 boot sector)
#   - kaosldr_16/kldr16.bin         (Stage 2 loader)
#   - <profile-target-dir>/kldr64.bin and kernel.bin
#   - the user programs under user_programs/*/
#
# Requires: mtools (mformat, mcopy). Install with `brew install mtools` (macOS) or
# `apt-get install mtools` (Linux); already present in the dev container.
#
# The image is a FAT32 *superfloppy* (FAT32 VBR at LBA 0, no partition table). The two
# early loaders live at fixed LBAs in the enlarged reserved-sector region so the 512-byte
# boot sector needs no filesystem parsing; KERNEL.BIN and the user programs are normal
# FAT32 files read later by kaosldr_64 and the kernel. See docs/todo_fat32_unification.md.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

PROFILE_DIR="${1:?usage: make_fat32_image.sh <profile-target-dir>}"
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
