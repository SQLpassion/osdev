/* User-program linker script.
 * - fixes the virtual load base to the kernel's user-code window
 * - enforces `_start` as entry symbol
 * - places `_start` at image offset 0 so exec can jump to base RIP
 */
ENTRY(_start)

SECTIONS
{
    /* User virtual base address where flat binaries are mapped by the kernel loader. */
    . = 0x0000700000000000;

    .text : AT(ADDR(.text))
    {
        /* Keep startup section first: kernel exec jumps to image base. */
        *(.ltext._start)
        *(.text._start)

        /* Remaining executable code emitted by rustc/lld. */
        *(.ltext .ltext.*)
        *(.text .text.*)

        /* Read-only data (string literals, const tables). */
        *(.rodata .rodata.*)

        /* Page-align code/rodata boundary for paging-friendly layout. */
        . = ALIGN(4K);
    }

    .data : AT(ADDR(.data))
    {
        /* Initialized writable globals. */
        *(.data .data.*)
    }

    .bss : AT(ADDR(.bss))
    {
        /* Zero-initialized writable globals. */
        *(.bss .bss.*)
    }
}
