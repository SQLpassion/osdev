#!/usr/bin/env bash
  set -euo pipefail

  VMID=103
  STORAGE=data
  IMG="$(pvesm path ${STORAGE}:iso/kaos64.img)"

  qm create "$VMID" \
    --name kaos64-bios \
    --memory 2048 \
    --cores 1 \
    --machine pc \
    --bios seabios \
    --serial0 socket \
    --vga std

  qm importdisk "$VMID" "$IMG" "$STORAGE" --format raw

  DISK="$(qm config "$VMID" | awk -F': ' '/^unused[0-9]+:/ {print $2; exit}')"

  if [ -z "$DISK" ]; then
    echo "No imported unused disk found."
    qm config "$VMID"
    exit 1
  fi

  qm set "$VMID" --ide0 "$DISK"
  qm set "$VMID" --boot order=ide0