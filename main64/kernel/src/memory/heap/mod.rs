//! Kernel heap manager.
//!
//! Design summary:
//! - Contiguous heap region with variable-sized blocks.
//! - Segregated free-list strategy with intrusive free nodes in free blocks.
//! - One header per block (`HeapBlockHeader`) storing `size`, `in_use` flag,
//!   `prev_size`, and an address-bound magic value for robust pointer validation.
//! - Block splitting on allocation and O(1) adjacent coalescing on free.
//! - Backed by a global spinlock for synchronized access.
//!
//! Notes:
//! - Block size includes the header itself.
//! - Payload pointer is always `header + HEADER_SIZE`.
//! - Heap growth is page-sized (`HEAP_GROWTH`) and relies on demand paging.

pub mod types;
pub mod generic;

#[cfg(feature = "kernel")]
pub mod kernel;

// Re-export public items to preserve the original API of the `heap` module.
#[allow(unused_imports)]
pub use types::HEAP_ALIGNMENT;

#[allow(unused_imports)]
pub use generic::{Heap, HeapEnvironment};

#[allow(unused_imports)]
#[cfg(feature = "kernel")]
pub use kernel::{
    init, is_initialized, malloc, free, run_self_test,
    debug_output_enabled, set_debug_output, max_heap_size,
    KernelHeapEnv,
};
