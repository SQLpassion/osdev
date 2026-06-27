//! Process lifecycle syscall wrappers: exec, wait, exit, shutdown.

use crate::{
    decode_result,
    raw::{syscall0, syscall1},
    SysError, SyscallId, MAX_PATH_LEN,
};

/// Executes a flat binary from the FAT12 disk.
///
/// `name` is automatically null-terminated in a stack buffer before the syscall.
/// Returns the task ID of the spawned process on success.
#[inline(always)]
pub fn exec(name: &str) -> Result<usize, SysError> {
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
        syscall1(SyscallId::Exec as u64, buf.as_ptr() as u64)
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
