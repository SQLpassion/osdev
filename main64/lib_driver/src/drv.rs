//! Driver name registration and resolution (`DrvRegister` / `DrvLookup`).

use crate::kernel_types::{decode_result, SysError, SyscallId};
use crate::raw::syscall2;

/// Maximum length, in bytes, of a driver name accepted by `DrvRegister`/`DrvLookup`.
///
/// Must match `kernel::drivers::registry::DRIVER_NAME_LEN` — duplicated here
/// because `registry.rs` is not part of the path-imported `kernel_types` ABI
/// module `lib_driver` shares with the kernel.
pub const DRIVER_NAME_LEN: usize = 32;

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
