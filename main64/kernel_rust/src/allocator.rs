//! Global allocator backed by the kernel heap.

use core::alloc::{GlobalAlloc, Layout};
use core::mem::size_of;

use crate::memory::heap;

pub struct KernelAllocator;

#[inline]
fn align_up(addr: usize, align: usize) -> Option<usize> {
    let mask = align.checked_sub(1)?;
    addr.checked_add(mask).map(|v| v & !mask)
}

#[inline]
fn aligned_backref_slot(aligned_ptr: *mut u8) -> *mut *mut u8 {
    aligned_ptr
        .wrapping_sub(size_of::<*mut u8>())
        .cast::<*mut u8>()
}

// SAFETY:
// - `heap::malloc`/`heap::free` provide exclusive access internally via a spinlock.
// - The heap returns pointers within a valid mapped region.
unsafe impl GlobalAlloc for KernelAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let size = layout.size().max(1);
        let align = layout.align();
        if align <= heap::HEAP_ALIGNMENT {
            return heap::malloc(size);
        }

        let overhead = match align
            .checked_sub(1)
            .and_then(|v| v.checked_add(size_of::<*mut u8>()))
        {
            Some(v) => v,
            None => return core::ptr::null_mut(),
        };
        let total_size = match size.checked_add(overhead) {
            Some(v) => v,
            None => return core::ptr::null_mut(),
        };

        let raw_ptr = heap::malloc(total_size);
        if raw_ptr.is_null() {
            return core::ptr::null_mut();
        }

        let Some(aligned_addr) = align_up(raw_ptr as usize + size_of::<*mut u8>(), align) else {
            heap::free(raw_ptr);
            return core::ptr::null_mut();
        };
        let aligned_ptr = aligned_addr as *mut u8;

        // SAFETY:
        // - `aligned_ptr` lies within the over-allocated region returned by `heap::malloc`.
        // - One pointer-sized slot before `aligned_ptr` is reserved for storing `raw_ptr`.
        unsafe {
            core::ptr::write(aligned_backref_slot(aligned_ptr), raw_ptr);
        }
        aligned_ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        if ptr.is_null() {
            return;
        }

        if layout.align() <= heap::HEAP_ALIGNMENT {
            heap::free(ptr);
            return;
        }

        // SAFETY:
        // - For over-aligned allocations, `alloc` stored the original heap pointer
        //   one pointer-sized slot before `ptr`.
        let raw_ptr = unsafe { core::ptr::read(aligned_backref_slot(ptr)) };
        heap::free(raw_ptr);
    }
}

#[global_allocator]
pub static GLOBAL_ALLOCATOR: KernelAllocator = KernelAllocator;
