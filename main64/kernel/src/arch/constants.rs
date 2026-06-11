//! Architecture-wide constants shared across subsystems.

/// Base page size used by x86_64 4 KiB pages.
pub const PAGE_SIZE: usize = 4096;

/// Base page size as `u64` for address arithmetic.
pub const PAGE_SIZE_U64: u64 = PAGE_SIZE as u64;
