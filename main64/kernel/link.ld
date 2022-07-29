ENTRY(kernel_main)

SECTIONS
{
    . = 0x100000;
    .text : AT(ADDR(.text))
    {
        *(.text .text.*)
        *(.data .data.*)
        *(.rodata .rodata.*)
        . = ALIGN(4K);
    }

    .bss : AT(ADDR(.bss))
    {
        *(.bss .bss.*)
    }

    .eh_frame : AT(ADDR(.eh_frame))
    {
        *(.eh_frame .eh_frame.*)
    }
}