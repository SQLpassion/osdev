#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

"${SCRIPT_DIR}/kaos64-bios-vm.sh"
"${SCRIPT_DIR}/kaos64-uefi-vm.sh"
