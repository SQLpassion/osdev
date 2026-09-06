//! Driver name registration/resolution, packet transport, and status
//! publishing (`DrvRegister`/`DrvLookup`, `NetSend`/`NetRecv`,
//! `DrvPublishStatus`/`DrvQuery`).

use crate::kernel_types::{decode_result, SysError, SyscallId, UserDriverInfo, UserDriverStatus};
use crate::raw::{syscall1, syscall2, syscall3, syscall4};

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
    // - Invokes the DrvRegister syscall (nr. 36).
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
    // - Invokes the DrvLookup syscall (nr. 37).
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
    // - Invokes the NetSend syscall (nr. 38).
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
/// immediately if nothing is queued; see `NetRecv`'s kernel doc comment for
/// why. A non-zero value blocks up to that many milliseconds.
///
/// Returns the number of bytes copied into `buf` (a packet larger than
/// `buf.len()` is truncated).
pub fn net_recv(driver_id: u64, buf: &mut [u8], timeout_ms: u64) -> Result<usize, SysError> {
    // SAFETY:
    // - Invokes the NetRecv syscall (nr. 39).
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

/// Publishes the calling driver task's current status snapshot for later
/// [`query_status`] reads. Requires the caller to already be registered via
/// [`drv_register`].
pub fn publish_status(status: &UserDriverStatus) -> Result<(), SysError> {
    // SAFETY:
    // - Invokes the DrvPublishStatus syscall (nr. 40).
    // - `status` is a valid, borrowed reference for the duration of this call.
    let raw = unsafe { syscall1(SyscallId::DRV_PUBLISH_STATUS, status as *const _ as u64) };
    decode_result(raw).map(|_| ())
}

/// Terminates a registered driver task by name (hard kill — no clean device
/// shutdown, see `docs/drivers.md` §15).
///
/// Requires the caller to have been delegated `Capabilities::UNLOAD_DRIVER`
/// at `Exec` time (or to be privileged) — ordinary Ring-3 apps cannot unload
/// a driver. Fails with `SysError::InvalidArgument` if `name` is not
/// currently registered.
pub fn unload_driver(name: &[u8]) -> Result<(), SysError> {
    if name.is_empty() || name.len() > DRIVER_NAME_LEN {
        return Err(SysError::InvalidArgument);
    }

    // SAFETY:
    // - Invokes the DrvUnload syscall (nr. 42).
    // - `name` is a valid, borrowed slice for the duration of this call.
    let raw = unsafe {
        syscall2(
            SyscallId::DRV_UNLOAD,
            name.as_ptr() as u64,
            name.len() as u64,
        )
    };
    decode_result(raw).map(|_| ())
}

/// Maximum number of drivers the registry can ever hold at once.
///
/// Must match `kernel::drivers::registry::MAX_DRIVERS` — duplicated here for
/// the same reason as `DRIVER_NAME_LEN` above. Sizing a [`list_drivers`]
/// buffer to this constant guarantees the call never truncates.
pub const MAX_DRIVERS: usize = 16;

/// Fills `out` with metadata for up to `out.len()` currently registered
/// drivers, in one syscall, and returns the *total* number of currently
/// registered drivers.
///
/// The return value may exceed `out.len()` — that is how a caller detects
/// truncation, since only `min(returned, out.len())` entries were actually
/// written. A buffer sized to [`MAX_DRIVERS`] never truncates, since that is
/// the registry's own fixed capacity.
///
/// A single registry-lock acquisition inside the kernel produces the whole
/// snapshot, so unlike a naive count-then-fetch-by-index design, there is no
/// window in which a driver registering or exiting mid-call could shift
/// what an index refers to.
pub fn list_drivers(out: &mut [UserDriverInfo]) -> Result<usize, SysError> {
    // SAFETY:
    // - Invokes the DrvList syscall (nr. 43).
    // - `out` is a valid, writable slice for the duration of this call; the
    //   kernel writes at most `out.len()` entries into it, and only ever
    //   dereferences the pointer at all when `out.len() > 0`.
    let raw = unsafe {
        syscall2(
            SyscallId::DRV_LIST,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    decode_result(raw).map(|n| n as usize)
}

/// Reads the last status snapshot published by `driver_id` via
/// [`publish_status`]. Fails with `SysError::InvalidArgument` if `driver_id`
/// is unknown or has never published.
pub fn query_status(driver_id: u64) -> Result<UserDriverStatus, SysError> {
    let mut out = core::mem::MaybeUninit::<UserDriverStatus>::uninit();
    // SAFETY:
    // - Invokes the DrvQuery syscall (nr. 41).
    // - `out` is a valid, writable destination for exactly
    //   `size_of::<UserDriverStatus>()` bytes for the duration of this call;
    //   the kernel only writes into it on success (`decode_result(raw)` is
    //   checked with `?` below before `out` is ever read).
    let raw = unsafe { syscall2(SyscallId::DRV_QUERY, driver_id, out.as_mut_ptr() as u64) };
    decode_result(raw)?;
    // SAFETY: the syscall succeeded, so the kernel wrote a complete
    // `UserDriverStatus` into `out` before returning.
    Ok(unsafe { out.assume_init() })
}

#[cfg(test)]
mod tests {
    use super::*;

    // These only exercise `unload_driver`'s length validation, which returns
    // before ever reaching the `DrvUnload` syscall — safe to run as a plain
    // host unit test, unlike the syscall-touching code paths in this module
    // (which require the kernel's `int 0x80` handler and are only covered by
    // `kernel/tests/driver_unload_test.rs`).

    #[test]
    fn test_unload_driver_rejects_empty_name() {
        assert_eq!(unload_driver(b""), Err(SysError::InvalidArgument));
    }

    #[test]
    fn test_unload_driver_rejects_name_too_long() {
        let too_long = [b'a'; DRIVER_NAME_LEN + 1];
        assert_eq!(unload_driver(&too_long), Err(SysError::InvalidArgument));
    }
}
