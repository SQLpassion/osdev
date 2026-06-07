//! User-space global heap allocator implementation.

#![allow(dead_code)]

extern crate alloc;

#[path = "../../kernel_rust/src/memory/vmm/vmm_constants.rs"]
pub mod vmm_constants;

#[path = "syscall.rs"]
pub mod syscall;

#[path = "../../kernel_rust/src/memory/heap/mod.rs"]
pub mod kernel_heap;

const USER_HEAP_BASE: usize = vmm_constants::USER_HEAP_BASE as usize;
use core::alloc::{GlobalAlloc, Layout};

/// Size of the initial heap page mapping (4 KiB).
const INITIAL_PAGE_SIZE: usize = 4096;

/// Offset in bytes within the first page where the allocatable arena starts.
/// This leaves 1024 bytes at the beginning of the page to store the `Heap` metadata,
/// ensuring that it does not overlap with the arena (which previously corrupted the
/// state since Heap needs 552 bytes).
const ARENA_OFFSET: usize = 1024;

/// Controls verbose per-allocation serial logging.
/// Set to `false` to disable in production for better performance.
const HEAP_DEBUG_LOGGING: bool = true;

/// Ring 3 implementation of the `HeapEnvironment` trait.
struct UserHeapEnv;

impl kernel_heap::HeapEnvironment for UserHeapEnv {
    fn map_memory(&self, start: usize, end: usize) -> bool {
        // If the range is within the manually pre-mapped first page, skip calling mmap.
        if start >= USER_HEAP_BASE && end <= USER_HEAP_BASE + INITIAL_PAGE_SIZE {
            return true;
        }

        // Map new pages at the specific target address via the kernel mmap syscall.
        let len = end - start;
        match syscall::mmap(start, len) {
            Ok(ptr) => !ptr.is_null(),
            Err(_) => false,
        }
    }

    fn max_heap_size(&self) -> usize {
        // User heap is capped at 256 MiB
        256 * 1024 * 1024
    }

    fn log(&self, msg: &str) {
        let mut writer = BufWriter::new();
        // Rewrite the generic allocator's prefixes to [USER HEAP] to distinguish from kernel logs
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
        let _ = syscall::write_serial(writer.as_bytes());
    }
}

/// Helper function to retrieve the mutable reference to the generic heap structure.
///
/// We annotate this helper with `#[inline(never)]` as a compiler barrier. This prevents
/// LLVM from propagating pointer provenance analysis across the function boundary,
/// ensuring that the raw integer address cast is not optimized to NULL.
///
/// # Provenance Workaround
/// LLVM's pointer provenance analysis may determine that a `const usize` cast to a
/// pointer has no valid provenance and optimize the access to a null dereference.
/// The `mov` instruction in inline assembly forces the value through a register,
/// breaking LLVM's ability to track its origin. This is a known workaround;
/// `core::ptr::with_exposed_provenance_mut` (nightly-only) would be the stable
/// alternative once it stabilizes.
#[inline(never)]
unsafe fn get_heap_mut() -> &'static mut kernel_heap::Heap<UserHeapEnv> {
    let heap_addr: usize;
    // SAFETY:
    // - We use inline assembly to load the constant USER_HEAP_BASE into a register.
    // - This serves as a physical instruction barrier to prevent LLVM optimizations.
    unsafe {
        core::arch::asm!("mov {}, {}", out(reg) heap_addr, const USER_HEAP_BASE);
    }
    // SAFETY:
    // - `heap_addr` is `USER_HEAP_BASE`.
    // - The heap metadata was successfully written at `USER_HEAP_BASE` in `init()`.
    unsafe { &mut *(heap_addr as *mut kernel_heap::Heap<UserHeapEnv>) }
}

/// Zero-sized global allocator proxy.
///
/// Since the loaded binary image pages are mapped read-only by the process loader,
/// we cannot store the mutable heap allocator state in static variables (which reside
/// in read-only pages).
///
/// To bypass this restriction, the actual `Heap` allocator metadata is stored directly
/// at the start of the mapped, writable user heap page (`USER_HEAP_BASE`). The global
/// allocator proxy only carries out pointer casts to access this state.
pub struct SafeUserHeapAllocator;

/// Stack-allocated buffer formatter to write formatted text without heap allocation
/// and reduce the number of individual WriteSerial syscalls.
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
        if bytes.len() > remaining {
            let to_copy = remaining;
            self.buf[self.len..self.len + to_copy].copy_from_slice(&bytes[..to_copy]);
            self.len += to_copy;
            Err(core::fmt::Error)
        } else {
            self.buf[self.len..self.len + bytes.len()].copy_from_slice(bytes);
            self.len += bytes.len();
            Ok(())
        }
    }
}

impl core::fmt::Write for BufWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.write_str(s)
    }
}

// SAFETY:
// - Single-threaded Ring 3 environment prevents concurrent access to the allocator.
unsafe impl Sync for SafeUserHeapAllocator {}

unsafe impl GlobalAlloc for SafeUserHeapAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY:
        // - Single-threaded context ensures exclusive access.
        let heap = unsafe { get_heap_mut() };
        let ptr = heap.allocate(layout);

        if HEAP_DEBUG_LOGGING {
            use core::fmt::Write;
            let mut writer = BufWriter::new();
            let _ = core::write!(
                &mut writer,
                "[USER HEAP] alloc ptr={:p} size={} align={}\n",
                ptr,
                layout.size(),
                layout.align()
            );
            let _ = syscall::write_serial(writer.as_bytes());
        }

        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY:
        // - Single-threaded context ensures exclusive access.
        let heap = unsafe { get_heap_mut() };

        // SAFETY:
        // - Single-threaded context ensures exclusive access.
        // - `ptr` is a valid pointer allocated by this allocator and satisfies layout.
        unsafe {
            heap.deallocate(ptr, layout);
        }

        if HEAP_DEBUG_LOGGING {
            use core::fmt::Write;
            let mut writer = BufWriter::new();
            let _ = core::write!(
                &mut writer,
                "[USER HEAP] free ptr={:p} size={} align={}\n",
                ptr,
                layout.size(),
                layout.align()
            );
            let _ = syscall::write_serial(writer.as_bytes());
        }
    }
}

#[global_allocator]
pub static ALLOCATOR: SafeUserHeapAllocator = SafeUserHeapAllocator;

/// Initializes the global user allocator by mapping the first heap page and placement-writing
/// the `Heap` metadata at `USER_HEAP_BASE`.
///
/// # Safety
/// - Must be called only once during early binary startup (typically in `_start()`).
pub unsafe fn init() -> Result<(), &'static str> {
    // Step 1: Manually map the first page of the user heap.
    let ptr = syscall::mmap(USER_HEAP_BASE, INITIAL_PAGE_SIZE)
        .map_err(|_| "Failed to map first heap page")?;
    if ptr as usize != USER_HEAP_BASE {
        return Err("Heap mapped at unexpected address");
    }

    // Step 2: Placement-write the generic Heap struct at the beginning of the mapped page.
    let heap_ptr = ptr as *mut kernel_heap::Heap<UserHeapEnv>;
    // SAFETY:
    // - `heap_ptr` points to the newly mapped writable page.
    // - Memory is initialized using core::ptr::write.
    unsafe {
        core::ptr::write(heap_ptr, kernel_heap::Heap::new(UserHeapEnv));
    }

    // Step 3: Initialize the allocatable arena, starting after the Heap metadata.
    // SAFETY:
    // - The heap instance was initialized in the step above.
    let h = unsafe { &mut *heap_ptr };
    h.init(
        ptr as usize + ARENA_OFFSET,
        INITIAL_PAGE_SIZE - ARENA_OFFSET,
    )?;

    Ok(())
}
