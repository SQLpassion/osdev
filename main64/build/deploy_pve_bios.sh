#!/usr/bin/env bash
# deploy_pve_bios.sh - Deploy the legacy BIOS disk image (kaos64.img) to a Proxmox VE (PVE) host.
#
# This script uploads the local BIOS image via SCP to the remote Proxmox server, deletes any existing
# VM with the specified VMID (default 601), creates a new Seabios-based legacy BIOS VM with std VGA
# and socket serial console, imports the disk image to the Proxmox storage, and starts the VM.
#
# Required tools: ssh, scp.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "$SCRIPT_DIR/.." && pwd)"

PVE_HOST="192.168.1.99"
PVE_USER="${PVE_USER:-root}"
PVE_SSH_PORT="${PVE_SSH_PORT:-22}"
VMID="${VMID:-601}"
STORAGE="${STORAGE:-data}"
LOCAL_IMG="${LOCAL_IMG:-${PROJECT_ROOT}/kaos64.img}"
REMOTE_IMG="${REMOTE_IMG:-/tmp/kaos64-bios-${VMID}.img}"

if ! [[ "$VMID" =~ ^[0-9]+$ ]]; then
    echo "VMID must be numeric: $VMID" >&2
    exit 1
fi

if ! [[ "$PVE_SSH_PORT" =~ ^[0-9]+$ ]]; then
    echo "PVE_SSH_PORT must be numeric: $PVE_SSH_PORT" >&2
    exit 1
fi

if ! [[ "$STORAGE" =~ ^[A-Za-z0-9_.:-]+$ ]]; then
    echo "STORAGE contains unsupported characters: $STORAGE" >&2
    exit 1
fi

if ! [[ "$REMOTE_IMG" =~ ^/[A-Za-z0-9_./:-]+$ ]]; then
    echo "REMOTE_IMG must be an absolute path without shell metacharacters: $REMOTE_IMG" >&2
    exit 1
fi

if [ ! -f "$LOCAL_IMG" ]; then
    echo "Local image not found: $LOCAL_IMG" >&2
    exit 1
fi

SSH_TARGET="${PVE_USER}@${PVE_HOST}"
SSH_CONTROL_PATH="/tmp/kpve-%C"
SSH_COMMON_OPTS=(
    -o ControlMaster=auto
    -o ControlPersist=60
    -o "ControlPath=${SSH_CONTROL_PATH}"
)
SSH_OPTS=(-p "$PVE_SSH_PORT" "${SSH_COMMON_OPTS[@]}")
SCP_OPTS=(-P "$PVE_SSH_PORT" "${SSH_COMMON_OPTS[@]}")

if [ -n "${PVE_SSH_KEY:-}" ]; then
    SSH_OPTS+=(-i "$PVE_SSH_KEY")
    SCP_OPTS+=(-i "$PVE_SSH_KEY")
fi

echo "Removing existing BIOS VM $VMID on $PVE_HOST if present"
ssh "${SSH_OPTS[@]}" "$SSH_TARGET" \
    "VMID=$VMID bash -s" <<'REMOTE_CLEANUP'
set -euo pipefail

if qm status "$VMID" >/dev/null 2>&1; then
    STATUS="$(qm status "$VMID" | awk '{print $2}')"

    if [ "$STATUS" != "stopped" ]; then
        qm stop "$VMID"
    fi

    qm destroy "$VMID" --purge 1
fi
REMOTE_CLEANUP

echo "Uploading $LOCAL_IMG to $SSH_TARGET:$REMOTE_IMG"
scp "${SCP_OPTS[@]}" "$LOCAL_IMG" "${SSH_TARGET}:${REMOTE_IMG}"

echo "Creating BIOS VM $VMID on $PVE_HOST"
ssh "${SSH_OPTS[@]}" "$SSH_TARGET" \
    "VMID=$VMID STORAGE=$STORAGE REMOTE_IMG=$REMOTE_IMG bash -s" <<'REMOTE_SCRIPT'
set -euo pipefail

trap 'rm -f "$REMOTE_IMG"' EXIT

qm create "$VMID" \
    --name kaos64-bios \
    --memory 2048 \
    --cores 1 \
    --machine pc \
    --bios seabios \
    --serial0 socket \
    --vga std

qm importdisk "$VMID" "$REMOTE_IMG" "$STORAGE" --format raw

DISK="$(qm config "$VMID" | awk -F': ' '/^unused[0-9]+:/ {print $2; exit}')"

if [ -z "$DISK" ]; then
    echo "No imported unused disk found." >&2
    qm config "$VMID"
    exit 1
fi

qm set "$VMID" --ide0 "$DISK"
qm set "$VMID" --boot order=ide0
qm start "$VMID"
REMOTE_SCRIPT
