//! Syscall table and dispatcher entry point.
//!
//! The low-level interrupt glue passes `(syscall_nr, arg0..arg3)` into
//! [`dispatch`]. Types/constants live in `types`, kernel dispatch logic in
//! `dispatch`, and user/raw wrappers in their dedicated submodules.

mod dispatch;
mod types;

pub mod abi;

/// Safe user-space syscall wrappers.
#[allow(dead_code)]
pub mod user;

#[allow(unused_imports)]
pub use dispatch::dispatch;

#[allow(unused_imports)]
pub use types::{
    decode_result, is_valid_user_buffer, user_alias_rip, user_alias_va_for_kernel, SysError,
    SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_IO, SYSCALL_ERR_UNSUPPORTED, SYSCALL_OK,
};
