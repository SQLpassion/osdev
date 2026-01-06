/* Linker script for KAOS Rust Kernel */
/* Higher-half kernel at virtual address 0xFFFF800000100000 */
/* Physical load address 0x100000 */

ENTRY(KernelMain)

KERNEL_VIRT_BASE = 0xFFFF800000000000;

SECTIONS
{
    /* Virtual address where kernel is mapped */
    . = 0xFFFF800000100000;

    /* Code section - catch all text variants */
    .text : AT(ADDR(.text) - KERNEL_VIRT_BASE)
    {
        *(.text.boot)      /* Entry point first */
        *(.text .text.*)
        *(.ltext .ltext.*)
    }

    /* Read-only data - catch all rodata variants */
    .rodata : AT(ADDR(.rodata) - KERNEL_VIRT_BASE)
    {
        *(.rodata .rodata.*)
        *(.lrodata .lrodata.*)
    }

    /* Initialized data */
    .data : AT(ADDR(.data) - KERNEL_VIRT_BASE)
    {
        *(.data .data.*)
        *(.ldata .ldata.*)
    }

    /* Align to page boundary before BSS */
    . = ALIGN(4K);

    /* Uninitialized data */
    .bss : AT(ADDR(.bss) - KERNEL_VIRT_BASE)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.lbss .lbss.*)
        *(COMMON)
        __bss_end = .;
    }

    /* Discard unwanted sections */
    /DISCARD/ :
    {
        *(.eh_frame)
        *(.comment)
    }
}
