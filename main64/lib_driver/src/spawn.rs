//! User-space driver spawn routine.

use crate::kernel_types::{decode_result, SysError, SyscallId, UserDriverGrants};
use crate::raw::syscall3;

/// Spawns a driver binary with specified capabilities and optional resource grants.
///
/// Arguments:
/// - `name`: Binary name as byte slice or string (must be null-terminated or valid string).
/// - `caps`: Coarse capability bitflags (e.g. `Capabilities::MMIO | Capabilities::IRQ`).
/// - `grants`: Optional reference to `UserDriverGrants` struct defining authorized MMIO regions and IRQ lines.
pub fn spawn_driver(
    name: &str,
    caps: u64,
    grants: Option<&UserDriverGrants>,
) -> Result<u64, SysError> {
    let mut buf = [0u8; 128];
    let name_bytes = name.as_bytes();
    if name_bytes.len() >= 128 {
        return Err(SysError::InvalidArgument);
    }
    buf[..name_bytes.len()].copy_from_slice(name_bytes);
    buf[name_bytes.len()] = 0;

    let name_ptr = buf.as_ptr() as u64;
    let grants_ptr = match grants {
        Some(g) => g as *const UserDriverGrants as u64,
        None => 0,
    };

    // SAFETY:
    // - Invokes SpawnDriver syscall (nr. 35).
    // - Validated by kernel capability system and ELF loader.
    let raw = unsafe { syscall3(SyscallId::SPAWN_DRIVER, name_ptr, caps, grants_ptr) };
    decode_result(raw)
}
