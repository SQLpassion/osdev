//! User-mode syscall API for KAOS Ring-3 programs (`int 0x80` ABI).
//!
//! Modules are grouped thematically:
//! - [`console`]  — VGA/serial output and keyboard input
//! - [`fs`]       — file system operations (open/read/write/delete)
//! - [`memory`]   — memory mapping (`mmap`)
//! - [`process`]  — process lifecycle (`exec`, `wait`, `exit`, `shutdown`)
//! - [`heap`]     — user-space global heap allocator (lazy-initialized on first allocation)
//!
//! All public items are also re-exported at crate root so user programs can write:
//! ```no_run
//! use lib_kaos as syscall;
//! syscall::write_console(b"hello\n").ok();
//! ```

#![no_std]
#![allow(dead_code)]

// Pull in kernel ABI types via path import — no separate Cargo crate needed.
#[path = "../../kernel_rust/src/syscall/types.rs"]
mod kernel_types;

pub(crate) use kernel_types::{
    SyscallId, SYSCALL_ERR_INVALID_ARG, SYSCALL_ERR_IO, SYSCALL_ERR_OUT_OF_MEMORY,
    SYSCALL_ERR_UNSUPPORTED,
};

mod raw;

pub mod console;
pub mod fs;
pub mod heap;
pub mod memory;
pub mod process;

// Flat re-exports — preserve the `syscall::write_console(...)` call pattern.
pub use console::{clear_screen, user_readline, write_console, write_serial};
pub use fs::{delete_file, file_exists, print_root_directory, File, FileMode};
pub use memory::mmap;
pub use process::{exec, exit, shutdown, wait};
