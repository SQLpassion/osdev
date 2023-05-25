ENTRY(ShellMain)

SECTIONS
{
    . = 0x0000700000000000;
    .text : AT(ADDR(.text))
    {
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4K);
    }

    .data : AT(ADDR(.data))
    {
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss))
    {
        *(.bss .bss.*)
    }
}