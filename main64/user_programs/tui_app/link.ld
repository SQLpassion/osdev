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

        /* Read-only data (string literals, const tables).
         * `.lrodata.*` is emitted by rustc under code-model=large; without
         * this match line lld would place it as an orphan output section.
         */
        *(.rodata .rodata.*)
        *(.lrodata .lrodata.*)

        /* Page-align code/rodata boundary for paging-friendly layout. */
        . = ALIGN(4K);
    }

    /* Writable globals.
     *
     * BSS sections are intentionally merged into the `.data` output section
     * instead of living in their own `.bss` output section. Reason:
     *
     * - The kernel loader (process/loader.rs) consumes a flat binary and
     *   maps `ceil(image.len() / PAGE_SIZE)` user-code pages. Anything past
     *   `image.len()` is not pre-mapped and falls into the page-fault path.
     * - `objcopy -O binary` strips SHT_NOBITS sections from the produced
     *   `.bin` file. If `.bss` is its own output section it stays NOBITS,
     *   the BSS bytes never reach disk, and `image.len()` underestimates
     *   the writable footprint of the program.
     * - When BSS input is placed inside an output section that already
     *   contains PROGBITS input (`.data`), the output section becomes
     *   PROGBITS, and lld emits zero-filled bytes for the BSS region into
     *   the binary. The loader then pre-maps those pages as writable user
     *   code pages — matching the policy described in loader.rs for flat
     *   binaries that mix code/data.
     * - `.lbss.*` is emitted by rustc under code-model=large; without an
     *   explicit match the BSS would otherwise drift back into an orphan
     *   `.bss` output section (and therefore back to SHT_NOBITS).
     */
    .data : AT(ADDR(.data))
    {
        /* Initialized writable globals. */
        *(.data .data.*)
        *(.ldata .ldata.*)

        /* Zero-initialized writable globals — see comment above for why
         * these are placed here instead of in a separate `.bss` section.
         */
        *(.bss .bss.*)
        *(.lbss .lbss.*)
        *(COMMON)

        /* Round up to a full page so the loader's page-rounded mapping
         * never leaves a partial last page short of the BSS end.
         */
        . = ALIGN(4K);
    }
}
