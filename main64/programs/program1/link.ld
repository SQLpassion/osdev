ENTRY(main)

SECTIONS
{
    . = 0xFFFF8000FFFF0000;
    .text : AT(ADDR(.text) - 0xFFFF800000000000)
    {
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4K);
    }

    .data : AT(ADDR(.data) - 0xFFFF800000000000)
    {
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss) - 0xFFFF800000000000)
    {
        *(.bss .bss.*)
    }
}