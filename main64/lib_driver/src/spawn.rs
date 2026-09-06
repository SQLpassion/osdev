//! User-space driver spawn routine.

use crate::kernel_types::{decode_result, SysError, SyscallId, UserDriverGrants, UserPciDevice};
use crate::raw::{syscall1, syscall3};

/// Copies `name` into a fixed-size, NUL-terminated stack buffer suitable for
/// a syscall that takes a `*const u8` filename pointer (`SpawnDriver`,
/// `DrvProbe`) — shared so the two don't each duplicate the same bounds
/// check and NUL-termination dance.
fn name_to_nul_terminated_buf(name: &str) -> Result<[u8; 128], SysError> {
    let mut buf = [0u8; 128];
    let name_bytes = name.as_bytes();
    if name_bytes.len() >= 128 {
        return Err(SysError::InvalidArgument);
    }
    buf[..name_bytes.len()].copy_from_slice(name_bytes);
    buf[name_bytes.len()] = 0;
    Ok(buf)
}

/// Checks whether `name` is a known driver binary and, if so, whether a
/// matching PCI device is currently present — a read-only query over the
/// same binary-name-to-PCI-ID table `SpawnDriver` itself derives grants
/// from, without any of `derive_grants`'s side effects (no device
/// reservation, no Command Register writes). Lets a caller decide whether
/// [`spawn_driver`] is even worth attempting, and print a specific reason
/// if not, without needing its own copy of that table.
///
/// Returns `Ok(true)` if `name` is known and a matching device is present,
/// `Ok(false)` if `name` is known but no matching device is present, or
/// `Err(SysError::InvalidArgument)` if `name` is not a known driver binary
/// at all.
pub fn probe_driver(name: &str) -> Result<bool, SysError> {
    let buf = name_to_nul_terminated_buf(name)?;

    // SAFETY:
    // - Invokes DrvProbe syscall (nr. 44).
    // - `buf` is a valid, NUL-terminated, borrowed buffer for the duration
    //   of this call.
    let raw = unsafe { syscall1(SyscallId::DRV_PROBE, buf.as_ptr() as u64) };
    decode_result(raw).map(|n| n != 0)
}

/// Spawns a driver binary with specified capabilities.
///
/// The MMIO regions the spawned driver may touch are derived by the
/// **kernel** from its own PCI enumeration, keyed on `name`
/// (`kernel/src/drivers/driver_db.rs`). Grants are therefore not something the caller
/// hands out: `grants` is only an optional *request* that the kernel cross-checks
/// against the device it bound the driver to, rejecting anything outside it with
/// `PermissionDenied`. Pass `None` to accept the kernel-derived grant as-is.
///
/// Likewise, `caps` is masked to the driver-grantable flags, so `SPAWN_DRIVER` cannot
/// be propagated into the spawned driver.
///
/// Arguments:
/// - `name`: Binary name as byte slice or string (must be null-terminated or valid string).
/// - `caps`: Coarse capability bitflags (e.g. `Capabilities::MMIO`).
/// - `grants`: Optional reference to a `UserDriverGrants` request to be validated.
pub fn spawn_driver(
    name: &str,
    caps: u64,
    grants: Option<&UserDriverGrants>,
) -> Result<u64, SysError> {
    let buf = name_to_nul_terminated_buf(name)?;
    let name_ptr = buf.as_ptr() as u64;
    let grants_ptr = match grants {
        Some(g) => g as *const UserDriverGrants as u64,
        None => 0,
    };

    // SAFETY:
    // - Invokes SpawnDriver syscall (nr. 32).
    // - Validated by kernel capability system and ELF loader.
    let raw = unsafe { syscall3(SyscallId::SPAWN_DRIVER, name_ptr, caps, grants_ptr) };
    decode_result(raw)
}

/// Returns the PCI device the kernel bound this task to at `SpawnDriver`
/// time (`kernel/src/drivers/driver_db.rs`'s `derive_grants`).
///
/// Only meaningful for a task that was itself spawned via `SpawnDriver`;
/// returns `SysError::InvalidArgument` otherwise, or if that task's device
/// binding was somehow already released. Lets a driver binary recover the
/// exact device it was granted MMIO access to directly from the kernel's own
/// authoritative binding, instead of independently re-scanning the PCI bus
/// and re-matching its own copy of the vendor/device IDs the kernel's
/// driver table already validated — see `DrvBoundDevice`'s doc comment in
/// `kernel/src/syscall/dispatch/driver.rs` for the full rationale.
pub fn bound_device() -> Result<UserPciDevice, SysError> {
    let zero_bar = crate::UserPciBar {
        bar_type: 0,
        flags: 0,
        address: 0,
        size: 0,
        raw_value: 0,
        _padding: 0,
    };
    let mut dev = UserPciDevice {
        bus: 0,
        device: 0,
        function: 0,
        class_code: 0,
        subclass: 0,
        prog_if: 0,
        revision_id: 0,
        header_type: 0,
        vendor_id: 0,
        device_id: 0,
        interrupt_line: 0,
        interrupt_pin: 0,
        _padding: [0; 2],
        bars: [zero_bar; 6],
    };

    // SAFETY:
    // - Invokes DrvBoundDevice syscall (nr. 45).
    // - `&mut dev` is a valid, writable, borrowed buffer for the duration of
    //   this call, sized exactly to `UserPciDevice` as the kernel expects.
    let raw = unsafe {
        syscall1(
            SyscallId::DRV_BOUND_DEVICE,
            &mut dev as *mut UserPciDevice as u64,
        )
    };
    decode_result(raw)?;
    Ok(dev)
}
