use core::arch::global_asm;

global_asm!(
    r#"
    .global execute_kernel
    .type execute_kernel, @function
    execute_kernel:
        # System V AMD64 ABI passes the first parameter (KernelSize in bytes) in RDI/EDI.
        # We jump directly to the kernel at 0xFFFF800000100000, passing the size in RDI.
        mov rax, 0xFFFF800000100000
        call rax
    "#
);

extern "C" {
    /// Executes the loaded x64 OS Kernel by jumping to 0xFFFF800000100000.
    pub fn execute_kernel(kernel_size: i32) -> !;
}
