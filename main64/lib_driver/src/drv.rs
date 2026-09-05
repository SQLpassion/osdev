//! Driver name registration/resolution and packet transport
//! (`DrvRegister`/`DrvLookup`, `NetSend`/`NetRecv`).

use crate::kernel_types::{decode_result, SysError, SyscallId};
use crate::raw::{syscall2, syscall3, syscall4};

/// Maximum length, in bytes, of a driver name accepted by `DrvRegister`/`DrvLookup`.
///
/// Must match `kernel::drivers::registry::DRIVER_NAME_LEN` — duplicated here
/// because `registry.rs` is not part of the path-imported `kernel_types` ABI
/// module `lib_driver` shares with the kernel.
pub const DRIVER_NAME_LEN: usize = 32;

/// Maximum length, in bytes, of a single packet accepted by `NetSend`/`NetRecv`.
///
/// Must match `kernel::drivers::registry::MAX_PACKET_LEN` — duplicated here
/// for the same reason as `DRIVER_NAME_LEN` above.
pub const MAX_PACKET_LEN: usize = 1536;

/// Registers the calling driver task under `name` (e.g. `"nic:rtl8139"`), so
/// applications can resolve it later via [`drv_lookup`].
///
/// Requires the caller to have been spawned via
/// [`spawn_driver`](crate::spawn::spawn_driver) (holds a non-null
/// `DriverCaps` block) — ordinary Ring-3 apps cannot register.
pub fn drv_register(name: &[u8]) -> Result<(), SysError> {
    if name.is_empty() || name.len() > DRIVER_NAME_LEN {
        return Err(SysError::InvalidArgument);
    }

    // SAFETY:
    // - Invokes the DrvRegister syscall (nr. 39).
    // - `name` is a valid, borrowed slice for the duration of this call, so
    //   the pointer/length pair handed to the kernel stays valid until the
    //   syscall returns.
    let raw = unsafe {
        syscall2(
            SyscallId::DRV_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    decode_result(raw).map(|_| ())
}

/// Resolves the packed task id of a driver previously registered via
/// [`drv_register`].
pub fn drv_lookup(name: &[u8]) -> Result<u64, SysError> {
    if name.is_empty() || name.len() > DRIVER_NAME_LEN {
        return Err(SysError::InvalidArgument);
    }

    // SAFETY:
    // - Invokes the DrvLookup syscall (nr. 40).
    // - `name` is a valid, borrowed slice for the duration of this call.
    let raw = unsafe {
        syscall2(
            SyscallId::DRV_LOOKUP,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    decode_result(raw)
}

/// Sends a raw packet to/from `driver_id`'s channel.
///
/// Role-based direction: if the calling task's own tid equals `driver_id`,
/// this pushes into the driver's own RX ring (Driver → App); otherwise it
/// pushes into `driver_id`'s TX ring (App → Driver). See `NetSend`'s kernel
/// doc comment (`kernel/src/syscall/dispatch/driver.rs`) for the full
/// rationale.
pub fn net_send(driver_id: u64, packet: &[u8]) -> Result<(), SysError> {
    if packet.is_empty() || packet.len() > MAX_PACKET_LEN {
        return Err(SysError::InvalidArgument);
    }

    // SAFETY:
    // - Invokes the NetSend syscall (nr. 41).
    // - `packet` is a valid, borrowed slice for the duration of this call.
    let raw = unsafe {
        syscall3(
            SyscallId::NET_SEND,
            driver_id,
            packet.as_ptr() as u64,
            packet.len() as u64,
        )
    };
    decode_result(raw).map(|_| ())
}

/// Receives a raw packet to/from `driver_id`'s channel, mirroring
/// [`net_send`]'s role-based direction.
///
/// `timeout_ms == 0` polls once and returns `Err(SysError::Timeout)`
/// immediately if nothing is queued — this deliberately differs from
/// `IrqWait`'s "0 = wait forever" convention; see `NetRecv`'s kernel doc
/// comment for why. A non-zero value blocks up to that many milliseconds.
///
/// Returns the number of bytes copied into `buf` (a packet larger than
/// `buf.len()` is truncated).
pub fn net_recv(driver_id: u64, buf: &mut [u8], timeout_ms: u64) -> Result<usize, SysError> {
    // SAFETY:
    // - Invokes the NetRecv syscall (nr. 42).
    // - `buf` is a valid, borrowed, mutable slice for the duration of this
    //   call; the kernel writes at most `buf.len()` bytes into it.
    let raw = unsafe {
        syscall4(
            SyscallId::NET_RECV,
            driver_id,
            buf.as_mut_ptr() as u64,
            buf.len() as u64,
            timeout_ms,
        )
    };
    decode_result(raw).map(|n| n as usize)
}
