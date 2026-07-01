#!/usr/bin/env bash
# deploy_pve_all.sh - Trigger deployment of both BIOS and UEFI virtual machines on Proxmox VE.
#
# This script sequentially runs deploy_pve_bios.sh and deploy_pve_uefi.sh to upload both
# legacy BIOS (kaos64.img) and UEFI (kaos64-uefi.img) disk images to the remote Proxmox
# server, configuring and starting VMs 601 (BIOS) and 602 (UEFI).
#
# Required tools: ssh, scp.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"${SCRIPT_DIR}/deploy_pve_bios.sh"
"${SCRIPT_DIR}/deploy_pve_uefi.sh"
