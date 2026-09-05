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

/// Base virtual address for MMIO regions mapped into driver address spaces.
/// Placed above the user heap to avoid collisions with program data.
/// Each MapPhysical call advances a per-task bump pointer from this base.
pub const USER_MMIO_BASE: u64 = 0x0000_7800_0000_0000;

/// Exclusive upper bound of the canonical "low half" of the virtual address
/// space that per-task user mappings (Code/Stack/Heap and any future
/// mmap-created regions) live in.
///
/// `USER_CODE_BASE` through this bound covers exactly the PML4 slots used for
/// user mappings (currently slots 224-255) and deliberately excludes PML4
/// slot 0 (the low-memory identity map, shared by every address space) and
/// the higher-half kernel slots (256 and above, also shared). A full-range
/// scan bounded by `[USER_CODE_BASE, USER_ADDRESS_SPACE_SCAN_END)` can
/// therefore safely reclaim *any* present user leaf mapping — including ones
/// outside the fixed Code/Stack/Heap windows — without risking a scan into
/// address-space-shared infrastructure.
pub const USER_ADDRESS_SPACE_SCAN_END: u64 = 0x0000_8000_0000_0000;
