/* Interactive exception exerciser linker script.
 * Groups sections into two page-aligned, non-overlapping PT_LOAD segments
 * (R-X text+rodata, RW- data+bss) via explicit PHDRS -- see docs/todo_elf.md.
 */
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5); /* R-X */
    data PT_LOAD FLAGS(6); /* RW- */
}

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
    } :text

    . = ALIGN(4K);

    .data : AT(ADDR(.data))
    {
        KEEP(*(.data.keep))
        *(.data .data.*)
    } :data

    .bss : AT(ADDR(.bss))
    {
        *(.bss .bss.*)
    } :data
}
