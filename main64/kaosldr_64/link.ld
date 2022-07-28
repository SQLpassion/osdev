ENTRY(kaosldr_main)

SECTIONS
{
    . = 0x3000;
    .text : AT(ADDR(.text))
    {
        *(.text .text.*)
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