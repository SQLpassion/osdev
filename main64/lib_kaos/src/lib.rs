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
//! use lib_kaos::console;
//! console::writeline(b"hello\n").ok();
//! ```

#![no_std]
#![allow(dead_code)]

// Pull in kernel ABI types via path import — no separate Cargo crate needed.
#[path = "../../kernel_rust/src/syscall/types.rs"]
mod kernel_types;

pub use kernel_types::SysError;
pub(crate) use kernel_types::{decode_result, SyscallId};

/// Maximum allowed length of a path or filename (including directory separators).
pub const MAX_PATH_LEN: usize = 128;

mod raw;

pub mod console;
pub mod fs;
pub mod heap;
pub mod memory;
pub mod pci;
pub mod process;

#[macro_export]
macro_rules! print {
    ($($arg:tt)*) => {
        $crate::console::_print(format_args!($($arg)*))
    };
}

#[macro_export]
macro_rules! println {
    () => {
        $crate::print!("\n")
    };
    ($($arg:tt)*) => {
        $crate::print!("{}\n", format_args!($($arg)*))
    };
}
