use crate::boot_info::BootInfo;
use core::arch::global_asm;

global_asm!(
    r#"
    .global execute_kernel
    .type execute_kernel, @function
    execute_kernel:
        # System V AMD64 ABI passes the first parameter (BootInfo pointer) in RDI.
        # We jump directly to the kernel at 0xFFFF800000100000, passing the pointer in RDI.
        mov rax, 0xFFFF800000100000
        call rax
    "#
);

extern "C" {
    /// Executes the loaded x64 OS Kernel by jumping to 0xFFFF800000100000.
    pub fn execute_kernel(boot_info: *const BootInfo) -> !;
}
