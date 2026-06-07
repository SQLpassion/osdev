//! Process lifecycle syscall wrappers: exec, wait, exit, shutdown.

use crate::{raw::{syscall0, syscall1}, SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_IO};

/// Executes a flat binary from the FAT12 disk.
///
/// `name` is automatically null-terminated in a stack buffer before the syscall.
/// Returns the task ID of the spawned process on success.
#[inline(always)]
pub fn exec(name: &[u8]) -> Result<usize, u64> {
    let mut buf = [0u8; 128];
    if name.len() >= 128 {
        return Err(SYSCALL_ERR_INVALID_ARG);
    }
    buf[..name.len()].copy_from_slice(name);
    buf[name.len()] = 0;

    let raw = unsafe {
        // SAFETY:
        // - `buf` is a valid null-terminated string on the stack.
        // - The kernel validates the pointer at the syscall boundary.
        syscall1(SyscallId::Exec as u64, buf.as_ptr() as u64)
    };
    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }
    Ok(raw as usize)
}

/// Blocks until the task with the given `task_id` exits.
#[inline(always)]
pub fn wait(task_id: usize) -> Result<(), u64> {
    let raw = unsafe {
        // SAFETY: `Wait` passes an integer task ID, no pointer arguments.
        syscall1(SyscallId::Wait as u64, task_id as u64)
    };
    if raw >= SYSCALL_ERR_IO {
        return Err(raw);
    }
    Ok(())
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
