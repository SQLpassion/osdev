//! CR0 control-register configuration.
//!
//! CR0 is a control register (not an MSR), so it lives here rather than in [`super::msr`].
//! Currently this module exists for one job: turning on **write protection** so the
//! kernel's own read-only mappings are actually enforced against ring 0.

use core::arch::asm;

/// CR0.WP — Write Protect (bit 16).
///
/// When **clear**, supervisor-mode (ring-0) stores ignore the page-level read/write bit
/// entirely — the CPU may write through a page-table entry whose `writable` bit is 0. So
/// a read-only `.text`/`.rodata` mapping is *not* enforced against the kernel itself.
///
/// When **set**, ring-0 writes honor `writable=0` at every level of the walk and raise a
/// `#PF` on a write to a read-only page — which is what makes the #63 Phase 5 W^X
/// mappings (kernel `.text` RO+X, `.rodata` RO+NX) load-bearing.
const CR0_WP: u64 = 1 << 16;

/// Enables `CR0.WP` so the kernel's read-only mappings are enforced against ring-0
/// writes. Idempotent (mirrors [`super::msr::enable_no_execute`]'s shape).
///
/// Safe with respect to every existing ring-0 write: page-table edits go through the
/// recursive window whose every level is mapped writable, and all heap/stack/data pages
/// are mapped writable — nothing legitimately writes a read-only page. Enable it early in
/// `KernelMain` (alongside `enable_no_execute`); it only becomes load-bearing once the
/// kernel-owned table with the RO `.text` mapping is installed by the CR3 switch, but is
/// harmless before then (the pre-switch tables map `.text` RWX).
///
/// # Safety contract
/// Caller must be in ring 0 on x86_64. Preserves every other CR0 bit.
pub fn enable_write_protect() {
    // SAFETY: CR0 is readable/writable in ring 0. Read-modify-write preserves every other
    // CR0 bit (PE/PG/…, and the FPU TS/MP/EM bits `fpu` manages) and sets only WP.
    unsafe {
        let mut cr0: u64;
        asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        if cr0 & CR0_WP == 0 {
            cr0 |= CR0_WP;
            asm!("mov cr0, {}", in(reg) cr0, options(nostack, preserves_flags));
        }
    }
}
