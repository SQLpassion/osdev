/* User-program linker script.
 * - fixes the virtual load base to the kernel's user-code window
 * - enforces `_start` as entry symbol
 * - places `_start` at image offset 0 so exec can jump to base RIP
 * - groups sections into exactly two page-aligned, non-overlapping PT_LOAD
 *   segments (R-X text+rodata, RW- data+bss) via explicit PHDRS, so the
 *   kernel's ELF loader (process/elf.rs, process/loader.rs) can map each
 *   segment with its own permissions
 */
ENTRY(_start)

PHDRS
{
    text PT_LOAD FLAGS(5); /* R-X */
    data PT_LOAD FLAGS(6); /* RW- */
}

SECTIONS
{
    /* User virtual base address where the kernel's ELF loader maps this program. */
    . = 0x0000700000000000;

    .text : AT(ADDR(.text))
    {
        /* Keep startup section first: kernel exec jumps to e_entry. */
        *(.ltext._start)
        *(.text._start)

        /* Remaining executable code emitted by rustc/lld. */
        *(.ltext .ltext.*)
        *(.text .text.*)

        /* Read-only data (string literals, const tables). */
        *(.rodata .rodata.*)
    } :text

    /* Force the next PT_LOAD to start on a fresh page: without this, lld may
     * pack the data segment's first bytes into the same page as the text
     * segment's tail, which the kernel's page-granularity ELF loader would
     * reject as an overlapping PT_LOAD (see elf::ElfError::SegmentsOverlap). */
    . = ALIGN(4K);

    .data : AT(ADDR(.data))
    {
        /* Force section to be PROGBITS by keeping the dummy static from lib_kaos */
        KEEP(*(.data.keep))

        /* Initialized writable globals. */
        *(.data .data.*)
    } :data

    .bss : AT(ADDR(.bss))
    {
        /* Zero-initialized writable globals. The kernel's ELF loader zero-fills
         * this from p_filesz to p_memsz itself, so this stays a normal
         * SHT_NOBITS section -- no need to force it into .data anymore. */
        *(.bss .bss.*)
    } :data
}
