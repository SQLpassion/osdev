//! Global allocator backed by the kernel heap.

use core::alloc::{GlobalAlloc, Layout};

use crate::memory::heap;

pub struct KernelAllocator;

// SAFETY:
// - `heap::malloc`/`heap::free` provide exclusive access internally via a spinlock.
// - The heap returns pointers within a valid mapped region.
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if layout.align() > heap::HEAP_ALIGNMENT {
            return core::ptr::null_mut();
        }

        let size = layout.size().max(1);
        heap::malloc(size)
    }

    unsafe fn dealloc(&self, ptr: *mut u8, _layout: Layout) {
        heap::free(ptr);
    }
}

#[global_allocator]
pub static GLOBAL_ALLOCATOR: KernelAllocator = KernelAllocator;
