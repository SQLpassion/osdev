ENTRY(_start)

SECTIONS
{
    . = 0x0000700000000000;

    .text : AT(ADDR(.text))
    {
        *(.ltext* .ltext.*)     /* catch LTO code sections */
    }

    .rodata : AT(ADDR(.rodata))
    {
        *(.lrodata* .lrodata.*) /* catch string/data sections from LTO */
    }

    .data : AT(ADDR(.data))
    {
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss))
    {
        *(.bss .bss.*)
        *(COMMON)
    }
}