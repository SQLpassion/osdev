ENTRY(ShellMain)

SECTIONS
{
    . = 0x0000700000000000;
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