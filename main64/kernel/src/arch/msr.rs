use core::arch::asm;

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
