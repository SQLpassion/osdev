//! User-space global heap allocator.
//!
//! The allocator is backed by the kernel's generic `Heap` implementation
//! (path-imported) and grows via the [`crate::memory::mmap`] syscall.
//!
//! # Usage
//! Call [`init`] exactly once at the start of `_start()` before any
//! allocation takes place.  Programs that never allocate can skip the call —
//! the `#[global_allocator]` registration is harmless if unused.

#![allow(dead_code)]

extern crate alloc;

#[path = "../../kernel_rust/src/memory/vmm/vmm_constants.rs"]
mod vmm_constants;

#[path = "../../kernel_rust/src/memory/heap/mod.rs"]
mod kernel_heap;

use core::alloc::{GlobalAlloc, Layout};

const USER_HEAP_BASE: usize = vmm_constants::USER_HEAP_BASE as usize;

/// Size of the initial heap page (4 KiB).
const INITIAL_PAGE_SIZE: usize = 4096;

/// Offset within the first page where the allocatable arena starts.
///
/// The first 1 KiB is reserved for the `Heap` metadata struct (~552 bytes)
/// to prevent it from overlapping with the arena.
const ARENA_OFFSET: usize = 1024;

/// Set to `false` to silence per-allocation serial log lines.
const HEAP_DEBUG_LOGGING: bool = true;

// ── HeapEnvironment impl ─────────────────────────────────────────────────────

struct UserHeapEnv;

impl kernel_heap::HeapEnvironment for UserHeapEnv {
    fn map_memory(&self, start: usize, end: usize) -> bool {
        // The first page is pre-mapped in `init`; skip the syscall for it.
        if start >= USER_HEAP_BASE && end <= USER_HEAP_BASE + INITIAL_PAGE_SIZE {
            return true;
        }
        let len = end - start;
        match crate::memory::mmap(start, len) {
            Ok(ptr) => !ptr.is_null(),
            Err(_) => false,
        }
    }

    fn max_heap_size(&self) -> usize {
        256 * 1024 * 1024 // 256 MiB cap
    }

    fn log(&self, msg: &str) {
        let mut writer = BufWriter::new();
        // Rewrite kernel-side prefixes so serial logs are clearly user-space.
        if let Some(rest) = msg.strip_prefix("[KERNEL HEAP]") {
            let _ = writer.write_str("[USER HEAP]");
            let _ = writer.write_str(rest);
        } else if let Some(rest) = msg.strip_prefix("[HEAP]") {
            let _ = writer.write_str("[USER HEAP]");
            let _ = writer.write_str(rest);
        } else {
            let _ = writer.write_str(msg);
        }
        let _ = writer.write_str("\n");
        let _ = crate::console::write_serial(writer.as_bytes());
    }
}

// ── heap pointer helper ──────────────────────────────────────────────────────

/// Returns a `&mut` reference to the `Heap` stored at `USER_HEAP_BASE`.
///
/// # Why `#[inline(never)]`?
/// The `#[inline(never)]` attribute acts as a compiler barrier that prevents
/// LLVM from eliminating the integer-to-pointer cast via provenance analysis.
/// Without it, LLVM may see the constant `USER_HEAP_BASE` as having no valid
/// provenance and fold the dereference into a null access.  Routing the value
/// through a register (`mov`) breaks that analysis.
#[inline(never)]
unsafe fn get_heap_mut() -> &'static mut kernel_heap::Heap<UserHeapEnv> {
    let heap_addr: usize;
    // SAFETY:
    // - Inline assembly loads the constant through a register, defeating LLVM
    //   provenance analysis that would otherwise nullify the cast.
    unsafe {
        core::arch::asm!("mov {}, {}", out(reg) heap_addr, const USER_HEAP_BASE);
    }
    // SAFETY:
    // - `heap_addr` == `USER_HEAP_BASE`.
    // - `init()` placed a valid `Heap` at that address before any call here.
    unsafe { &mut *(heap_addr as *mut kernel_heap::Heap<UserHeapEnv>) }
}

// ── global allocator proxy ───────────────────────────────────────────────────

/// Initialises the global user allocator.
///
/// Maps the first heap page via `mmap` and placement-writes the `Heap` metadata
/// at `USER_HEAP_BASE`. If already initialized, returns `Ok(())`.
fn init_if_needed() -> Result<(), &'static str> {
    match crate::memory::mmap(USER_HEAP_BASE, INITIAL_PAGE_SIZE) {
        Ok(ptr) => {
            if ptr as usize != USER_HEAP_BASE {
                return Err("Heap mapped at unexpected address");
            }
            let heap_ptr = ptr as *mut kernel_heap::Heap<UserHeapEnv>;
            // SAFETY:
            // - `heap_ptr` points to the freshly mapped, writable page.
            unsafe { core::ptr::write(heap_ptr, kernel_heap::Heap::new(UserHeapEnv)) };

            // SAFETY:
            // - `heap_ptr` was just initialised above.
            let h = unsafe { &mut *heap_ptr };
            h.init(
                ptr as usize + ARENA_OFFSET,
                INITIAL_PAGE_SIZE - ARENA_OFFSET,
            )?;
            Ok(())
        }
        Err(crate::SysError::InvalidArgument) => {
            // Already mapped/initialized, so this is a successful check.
            Ok(())
        }
        Err(_) => Err("Failed to map first heap page"),
    }
}

/// Zero-sized proxy that forwards allocations to the `Heap` at `USER_HEAP_BASE`.
///
/// Mutable allocator state lives on the heap page itself (not in the read-only
/// binary image), so a zero-sized proxy avoids linker issues with mutable statics.
pub struct SafeUserHeapAllocator;

// SAFETY: The kernel places user tasks in a single-threaded Ring-3 environment.
unsafe impl Sync for SafeUserHeapAllocator {}

unsafe impl GlobalAlloc for SafeUserHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        if init_if_needed().is_err() {
            return core::ptr::null_mut();
        }

        // SAFETY: Single-threaded context guarantees exclusive access.
        let heap = unsafe { get_heap_mut() };
        let ptr = heap.allocate(layout);

        if HEAP_DEBUG_LOGGING {
            use core::fmt::Write;
            let mut w = BufWriter::new();
            let _ = core::writeln!(
                &mut w,
                "[USER HEAP] alloc ptr={:p} size={} align={}",
                ptr,
                layout.size(),
                layout.align()
            );
            let _ = crate::console::write_serial(w.as_bytes());
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: Single-threaded context guarantees exclusive access.
        let heap = unsafe { get_heap_mut() };
        // SAFETY: `ptr` was allocated by this allocator with the same layout.
        unsafe { heap.deallocate(ptr, layout) };

        if HEAP_DEBUG_LOGGING {
            use core::fmt::Write;
            let mut w = BufWriter::new();
            let _ = core::writeln!(
                &mut w,
                "[USER HEAP] free ptr={:p} size={} align={}",
                ptr,
                layout.size(),
                layout.align()
            );
            let _ = crate::console::write_serial(w.as_bytes());
        }
    }
}

#[global_allocator]
pub static ALLOCATOR: SafeUserHeapAllocator = SafeUserHeapAllocator;

/// Initialises the global user allocator manually.
///
/// # Safety
/// Must be called exactly once, before any heap allocation, from the
/// single-threaded `_start()` entry point.
pub unsafe fn init() -> Result<(), &'static str> {
    init_if_needed()
}

// ── stack-allocated log formatter ────────────────────────────────────────────

/// Fixed-capacity formatter used in log calls to avoid heap allocations.
struct BufWriter {
    buf: [u8; 128],
    len: usize,
}

impl BufWriter {
    fn new() -> Self {
        Self {
            buf: [0u8; 128],
            len: 0,
        }
    }

    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }

    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let remaining = self.buf.len() - self.len;
        let to_copy = bytes.len().min(remaining);
        self.buf[self.len..self.len + to_copy].copy_from_slice(&bytes[..to_copy]);
        self.len += to_copy;
        if bytes.len() > remaining {
            Err(core::fmt::Error)
        } else {
            Ok(())
        }
    }
}

impl core::fmt::Write for BufWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_str(s)
    }
}
