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
#[path = "../../kernel/src/syscall/types.rs"]
mod kernel_types;

pub use kernel_types::SysError;
pub(crate) use kernel_types::{decode_result, SyscallId};

/// Maximum allowed length of a path or filename (including directory separators).
pub const MAX_PATH_LEN: usize = 128;

mod raw;

pub mod bios;
pub mod console;
pub mod fs;
pub mod heap;
pub mod memory;
pub mod pci;
pub mod process;
pub mod time;

/// Non-zero dummy variable to force the `.data` output section of all user programs
/// to be compiled as `SHT_PROGBITS` instead of `SHT_NOBITS`. This prevents `objcopy`
/// from stripping the `.data` (and merged `.bss`) section, ensuring the flat binary
/// file has the correct size, which is needed so the kernel loader pre-maps all
/// required user memory pages.
#[used]
#[no_mangle]
#[cfg_attr(target_os = "macos", link_section = "__DATA,.data.keep")]
#[cfg_attr(not(target_os = "macos"), link_section = ".data.keep")]
static DUMMY_PROGBITS_FORCE_DATA: u8 = 1;

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
