use core::arch::asm;

/// Extended Feature Enable Register MSR index.
pub const IA32_EFER: u32 = 0xC000_0080;

/// EFER.NXE — No-Execute Enable (bit 11). When set, bit 63 of page-table entries
/// is honored as the No-Execute flag; when clear, the CPU treats bit 63 as
/// reserved-must-be-zero and faults on any access to an entry that sets it.
const EFER_NXE: u64 = 1 << 11;

/// Enables `EFER.NXE` so the No-Execute (bit 63) flag in page-table entries is
/// honored by the CPU.
///
/// The kernel marks user stack/heap pages non-executable (NX). Without NXE the
/// CPU treats bit 63 as reserved, so the first access to such a page raises a
/// reserved-bit page fault — fatal on real hardware (QEMU happens to be lax).
/// The legacy loader (`kaosldr_16/longmode.asm`) enables NXE, but the UEFI loader
/// (`kaosldr_uefi`) does not, so the kernel enables it here to be independent of
/// which loader booted it. Idempotent: a no-op when NXE is already set.
///
/// Must run before any NX-marked mapping is accessed (i.e. before user programs
/// are loaded). It only changes how bit 63 is interpreted; it does not by itself
/// make any currently-executing code non-executable.
pub fn enable_no_execute() {
    // SAFETY:
    // - EFER (0xC000_0080) is a valid, architectural MSR readable/writable in ring 0.
    // - We preserve every other EFER bit (LME/LMA/SCE) and only set NXE.
    unsafe {
        let efer = rdmsr(IA32_EFER);
        if efer & EFER_NXE == 0 {
            wrmsr(IA32_EFER, efer | EFER_NXE);
        }
    }
}

/// Reads a 64-bit value from the specified Model-Specific Register (MSR).
///
/// # Safety
/// The caller must ensure that the given MSR index is valid for the current processor.
/// Reading an invalid or unsupported MSR will raise a #GP fault.
#[inline]
pub unsafe fn rdmsr(msr: u32) -> u64 {
    let low: u32;
    let high: u32;
    asm!(
        "rdmsr",
        in("ecx") msr,
        out("eax") low,
        out("edx") high,
        options(nomem, nostack, preserves_flags)
    );
    ((high as u64) << 32) | (low as u64)
}

/// Writes a 64-bit value to the specified Model-Specific Register (MSR).
///
/// # Safety
/// The caller must ensure that the given MSR index is valid and that the value
/// being written does not violate any CPU constraints. Writing invalid data or
/// targeting an unsupported MSR will raise a #GP fault.
#[inline]
pub unsafe fn wrmsr(msr: u32, value: u64) {
    let low = value as u32;
    let high = (value >> 32) as u32;
    asm!(
        "wrmsr",
        in("ecx") msr,
        in("eax") low,
        in("edx") high,
        options(nomem, nostack, preserves_flags)
    );
}
