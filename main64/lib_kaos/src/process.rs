//! Process lifecycle syscall wrappers: exec, wait, exit, shutdown.

use crate::{
    decode_result,
    raw::{syscall0, syscall1, syscall2},
    SysError, SyscallId, MAX_PATH_LEN,
};

/// Coarse capability bits that `exec_with_capabilities` may ask the kernel to
/// delegate to a spawned child. Mirrors `kernel::process::capabilities::Capabilities`
/// (see `kernel/src/process/capabilities.rs`) — kept as raw bit constants here
/// since user-space processes only ever pass these as an opaque `u64` and
/// never need the kernel-side `Capabilities` type itself.
pub mod capabilities {
    /// May call SpawnDriver — reserved for the driver manager only.
    pub const SPAWN_DRIVER: u64 = 1 << 2;
    /// May call Unload to terminate a registered driver task by name.
    pub const UNLOAD_DRIVER: u64 = 1 << 3;
}

/// Executes a flat binary from the mounted filesystem.
///
/// `name` is automatically null-terminated in a stack buffer before the syscall.
/// Returns the task ID of the spawned process on success.
///
/// Delegates no capabilities to the spawned child. Use
/// [`exec_with_capabilities`] to delegate a subset of the caller's own
/// capabilities (only ever effective for a privileged caller, or one that
/// already holds the requested bits itself — see the kernel's `Exec` handler).
#[inline(always)]
pub fn exec(name: &str) -> Result<usize, SysError> {
    exec_with_capabilities(name, 0)
}

/// Same as [`exec`], but additionally requests that `requested_caps` (a
/// bitmask built from [`capabilities`]) be delegated to the spawned child.
///
/// The kernel grants at most the intersection of what is requested and what
/// the caller itself is entitled to delegate — an unprivileged caller can
/// never grant a child more than it already holds itself. See
/// `kernel::syscall::dispatch::process::syscall_exec_impl` for the exact
/// authorization rule.
#[inline(always)]
pub fn exec_with_capabilities(name: &str, requested_caps: u64) -> Result<usize, SysError> {
    let mut buf = [0u8; MAX_PATH_LEN];
    let name_bytes = name.as_bytes();
    if name_bytes.len() >= MAX_PATH_LEN {
        return Err(SysError::InvalidArgument);
    }
    buf[..name_bytes.len()].copy_from_slice(name_bytes);
    buf[name_bytes.len()] = 0;

    let raw = unsafe {
        // SAFETY:
        // - `buf` is a valid null-terminated string on the stack.
        // - The kernel validates the pointer at the syscall boundary.
        syscall2(SyscallId::Exec as u64, buf.as_ptr() as u64, requested_caps)
    };
    decode_result(raw).map(|pid| pid as usize)
}

/// Blocks until the task with the given `task_id` exits.
#[inline(always)]
pub fn wait(task_id: usize) -> Result<(), SysError> {
    let raw = unsafe {
        // SAFETY: `Wait` passes an integer task ID, no pointer arguments.
        syscall1(SyscallId::Wait as u64, task_id as u64)
    };
    decode_result(raw).map(|_| ())
}

/// Terminates the current user task.
#[inline(always)]
pub fn exit() -> ! {
    unsafe {
        // SAFETY: `Exit` terminates the task; the kernel never returns from it.
        let _ = syscall0(SyscallId::Exit as u64);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Shuts down the machine.
#[inline(always)]
pub fn shutdown() -> ! {
    unsafe {
        // SAFETY: `Shutdown` halts the machine; the kernel never returns from it.
        let _ = syscall0(SyscallId::Shutdown as u64);
    }
    loop {
        core::hint::spin_loop();
    }
}

/// Yields the CPU cooperatively to allow other tasks to run.
#[inline(always)]
pub fn yield_now() {
    unsafe {
        // SAFETY: Yield is a safe syscall that does not access memory.
        let _ = syscall0(SyscallId::Yield as u64);
    }
}
