//! Shared virtual memory layout constants between kernel and user space.

/// User executable base virtual address.
pub const USER_CODE_BASE: u64 = 0x0000_7000_0000_0000;

/// User executable mapping size (2 MiB).
pub const USER_CODE_SIZE: u64 = 0x0020_0000;

/// User executable end address (exclusive).
pub const USER_CODE_END: u64 = USER_CODE_BASE + USER_CODE_SIZE;

/// User stack top (exclusive upper boundary).
pub const USER_STACK_TOP: u64 = 0x0000_7FFF_F000_0000;

/// User stack size (1 MiB).
pub const USER_STACK_SIZE: u64 = 0x0010_0000;

/// User stack start (inclusive).
pub const USER_STACK_BASE: u64 = USER_STACK_TOP - USER_STACK_SIZE;

/// Optional guard page below the user stack.
pub const USER_STACK_GUARD_BASE: u64 = USER_STACK_BASE - 4096;

/// Optional guard page end (exclusive).
pub const USER_STACK_GUARD_END: u64 = USER_STACK_BASE;

/// User heap base virtual address (grows upwards).
pub const USER_HEAP_BASE: u64 = 0x0000_7000_1000_0000;

/// User heap size limit (256 MiB).
pub const USER_HEAP_SIZE: u64 = 0x0000_0000_1000_0000;

/// User heap end address (exclusive).
pub const USER_HEAP_END: u64 = USER_HEAP_BASE + USER_HEAP_SIZE;
