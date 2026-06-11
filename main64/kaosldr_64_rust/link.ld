ENTRY(kaosldr_main)

SECTIONS
{
    . = 0x3000;
    .text : AT(ADDR(.text))
    {
        *(.text.boot)
        *(.text .text.*)
        *(.ltext .ltext.*)
        . = ALIGN(4K);
    }

    .rodata : AT(ADDR(.rodata))
    {
        *(.rodata .rodata.*)
        *(.lrodata .lrodata.*)
    }

    .data : AT(ADDR(.data))
    {
        *(.data .data.*)
        *(.ldata .ldata.*)
    }

    .bss : AT(ADDR(.bss))
    {
        *(.bss .bss.*)
        *(.lbss .lbss.*)
        *(COMMON)
    }

    /DISCARD/ :
    {
        *(.eh_frame)
        *(.comment)
    }
}
