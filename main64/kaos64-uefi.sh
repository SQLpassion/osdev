set -euo pipefail

VMID=103
STORAGE=data
IMG="$(pvesm path ${STORAGE}:iso/kaos64-uefi.img)"

qm create $VMID \
  --name kaos64-uefi \
  --memory 2048 \
  --cores 1 \
  --machine q35 \
  --bios ovmf \
  --efidisk0 ${STORAGE}:0,efitype=4m,pre-enrolled-keys=0 \
  --serial0 socket \
  --vga std

qm importdisk $VMID "$IMG" ${STORAGE} --format raw

DISK="$(qm config $VMID | awk -F': ' '/^unused0:/ {print $2}')"

if [ -z "$DISK" ]; then
    echo "No imported unused disk found."
    qm config "$VMID"
    exit 1
  fi

qm set $VMID --sata0 "$DISK"
qm set $VMID --boot order=sata0