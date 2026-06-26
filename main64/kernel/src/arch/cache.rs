//! CPU Cache Management

/// Flushes the internal CPU caches (L1, L2, L3) and signals external caches to write back
/// their data to main memory, then invalidates them.
///
/// # Safety
/// This is a privileged instruction (Ring 0). It is very expensive and halts other CPUs
/// temporarily during the flush, but is necessary when changing page cache types (like PAT).
#[inline(always)]
pub unsafe fn wbinvd() {
    // SAFETY:
    // - wbinvd is a valid x86 instruction.
    // - Caller must ensure we are in ring 0 and it is safe to stall the CPU temporarily.
    unsafe {
        core::arch::asm!("wbinvd", options(nostack));
    }
}
