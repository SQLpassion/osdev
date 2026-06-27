//! Stack and context-frame allocation helpers for the scheduler.

use core::alloc::Layout;
use core::mem::size_of;
use core::ptr;

extern crate alloc;
use alloc::alloc as heap_alloc;

use super::types::KernelTaskFn;
use super::{
    DEFAULT_RFLAGS, KERNEL_CODE_SELECTOR, KERNEL_DATA_SELECTOR, STACK_ALIGNMENT, TASK_STACK_SIZE,
    USER_CODE_SELECTOR, USER_DATA_SELECTOR,
};
use crate::arch::constants::PAGE_SIZE;
use crate::arch::interrupts::{InterruptStackFrame, SavedRegisters};
use crate::scheduler::roundrobin::exit_current_task;

/// Allocates a stack from the kernel heap and touches every page.
///
/// Returns a pointer to the base of the allocated block, or null on failure.
/// The returned memory is 16-byte aligned and zero-touched on every page
/// boundary to force demand paging at allocation time rather than in IRQ
/// context.
pub(crate) fn allocate_task_stack() -> *mut u8 {
    // Step 1: Pre-calculate layout with safety constraints for the stack size and alignment.
    // SAFETY:
    // - This requires `unsafe` because it dereferences or performs arithmetic on raw pointers, which Rust cannot validate.
    // - Layout is non-zero and alignment is a power of two.
    let layout = unsafe { Layout::from_size_align_unchecked(TASK_STACK_SIZE, STACK_ALIGNMENT) };

    // Step 2: Request raw block allocation from the global heap.
    // SAFETY:
    // - This requires `unsafe` because unchecked `Layout` construction bypasses runtime validation of size/alignment constraints.
    // - Layout has non-zero size.
    let ptr = unsafe { heap_alloc::alloc(layout) };
    if ptr.is_null() {
        return ptr::null_mut();
    }

    // Step 3: Touch every page to force demand paging now, not during IRQ context.
    // SAFETY:
    // - This requires `unsafe` because raw pointer memory access is performed directly and Rust cannot verify pointer validity.
    // - `ptr` points to a valid allocation of `TASK_STACK_SIZE` bytes.
    unsafe {
        for page_off in (0..TASK_STACK_SIZE).step_by(PAGE_SIZE) {
            ptr::write_volatile(ptr.add(page_off), 0);
        }
    }

    ptr
}

/// Frees a heap-allocated task stack.
///
/// # Safety
///
/// `ptr` must have been returned by `allocate_task_stack` and must not be
/// used after this call.
pub(crate) unsafe fn free_task_stack(ptr: *mut u8) {
    if ptr.is_null() {
        return;
    }
    // Step 1: Re-create layout corresponding to the original allocation parameters.
    // SAFETY:
    // - Constants match the layout used by `allocate_task_stack`.
    // - Size is non-zero and alignment is a valid power of two.
    let layout = Layout::from_size_align_unchecked(TASK_STACK_SIZE, STACK_ALIGNMENT);

    // Step 2: Deallocate memory block using the constructed layout.
    heap_alloc::dealloc(ptr, layout);
}

extern "C" fn task_return_trap() -> ! {
    exit_current_task()
}

/// Builds the initial kernel-task context on a heap-allocated stack.
///
/// Returns a pointer to the saved [`SavedRegisters`] used as scheduler context,
/// and the stack top address (for `kernel_rsp_top`).
pub(crate) fn build_initial_kernel_task_frame(
    stack_base: *mut u8,
    stack_size: usize,
    entry: KernelTaskFn,
) -> (*mut SavedRegisters, u64) {
    // SAFETY:
    // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
    // - `stack_base` points to a valid heap allocation of `stack_size` bytes.
    // - The caller guarantees exclusive access to this stack memory.
    unsafe {
        // Step 1: Calculate stack limits.
        let stack_top = stack_base as usize + stack_size;

        // Step 2: Set up SysV-friendly entry stack alignment.
        // Keep one return-address slot below RSP for a synthetic trap target.
        let entry_rsp = align_down(stack_top, 16) - 8;
        let iret_addr = entry_rsp - size_of::<InterruptStackFrame>();
        let frame_addr = iret_addr - size_of::<SavedRegisters>();

        let frame_ptr = frame_addr as *mut SavedRegisters;
        let iret_ptr = iret_addr as *mut InterruptStackFrame;

        // Step 3: Register a safety fallback trap for accidental returns.
        // SAFETY:
        // - `entry_rsp` lies within the task's private stack memory.
        // - Writing a synthetic return address ensures an accidental task return
        //   traps into scheduler-controlled termination.
        ptr::write(
            entry_rsp as *mut u64,
            task_return_trap as *const () as usize as u64,
        );

        // Step 4: Write default register states.
        ptr::write(frame_ptr, SavedRegisters::default());

        // Step 5: Write the initial interrupt return frame pointing to kernel entry.
        ptr::write(
            iret_ptr,
            InterruptStackFrame {
                rip: entry as usize as u64,
                cs: KERNEL_CODE_SELECTOR,
                rflags: DEFAULT_RFLAGS,
                rsp: entry_rsp as u64,
                ss: KERNEL_DATA_SELECTOR,
            },
        );

        (frame_ptr, stack_top as u64)
    }
}

/// Builds an initial user-mode task context on a heap-allocated stack.
///
/// The saved interrupt frame is configured so that the next scheduler-selected
/// `iretq` transitions to ring 3 at `entry_rip` with user stack `user_rsp`.
pub(crate) fn build_initial_user_task_frame(
    stack_base: *mut u8,
    stack_size: usize,
    entry_rip: u64,
    user_rsp: u64,
) -> (*mut SavedRegisters, u64) {
    // SAFETY:
    // - This requires `unsafe` because it performs operations that Rust marks as potentially violating memory or concurrency invariants.
    // - `stack_base` points to a valid heap allocation of `stack_size` bytes.
    // - The caller guarantees exclusive access to this stack memory.
    unsafe {
        let stack_top = stack_base as usize + stack_size;

        let frame_addr = align_down(stack_top, 16)
            - size_of::<SavedRegisters>()
            - size_of::<InterruptStackFrame>();
        let frame_ptr = frame_addr as *mut SavedRegisters;
        let iret_ptr = (frame_addr + size_of::<SavedRegisters>()) as *mut InterruptStackFrame;

        ptr::write(frame_ptr, SavedRegisters::default());
        ptr::write(
            iret_ptr,
            InterruptStackFrame {
                rip: entry_rip,
                cs: USER_CODE_SELECTOR,
                rflags: DEFAULT_RFLAGS, // IF=1 so timer preemption remains active in user mode.
                rsp: user_rsp,
                ss: USER_DATA_SELECTOR,
            },
        );

        (frame_ptr, stack_top as u64)
    }
}

/// Aligns `value` down to the given power-of-two `align`.
#[inline]
const fn align_down(value: usize, align: usize) -> usize {
    value & !(align - 1)
}
