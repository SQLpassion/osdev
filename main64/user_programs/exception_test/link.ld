/* Interactive exception exerciser linker script. */
ENTRY(_start)

SECTIONS
{
    . = 0x0000700000000000;

    .text : AT(ADDR(.text))
    {
        *(.ltext._start)
        *(.text._start)
        *(.ltext .ltext.*)
        *(.text .text.*)
        *(.rodata .rodata.*)
        . = ALIGN(4K);
    }

    .data : AT(ADDR(.data))
    {
        KEEP(*(.data.keep))
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss))
    {
        *(.bss .bss.*)
    }
}
