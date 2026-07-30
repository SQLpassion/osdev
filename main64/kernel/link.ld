/* Linker script for KAOS Rust Kernel */
/* Higher-half kernel at virtual address 0xFFFF800000100000 */
/* Physical load address 0x100000 */

ENTRY(KernelMain)

KERNEL_VIRT_BASE = 0xFFFF800000000000;

SECTIONS
{
    /* Virtual address where kernel is mapped */
    . = 0xFFFF800000100000;

    /*
     * #63 Phase 5 (W^X): each section is page-aligned and delimited by boundary symbols
     * so the kernel-owned page table can map the higher-half kernel image (PML4 slot 256)
     * per-section at 4 KiB granularity with distinct permissions — .text RO+X,
     * .rodata RO+NX, .data/.bss RW+NX (see `memory::vmm::direct_map::kernel_image_layout`).
     * Two sections must never share a 4 KiB page, or they cannot carry different rights.
     *
     * LMA = VMA - KERNEL_VIRT_BASE for every section, so aligning the VMA aligns the LMA
     * identically; `objcopy -O binary` fills the inter-section gaps with zeros at the
     * correct file offsets, keeping the flat binary the loaders copy to 0x100000 correct.
     * `.text.boot` MUST stay first: both loaders jump to the literal entry VA
     * 0xFFFF800000100000 (kaosldr_64/src/asm.rs, kaosldr_uefi/src/main.rs) without
     * parsing an ELF entry, so the entry stub has to sit at the image base.
     */
    . = ALIGN(4K);
    __kernel_start = .;

    /* Code section - catch all text variants (RO + X) */
    .text : AT(ADDR(.text) - KERNEL_VIRT_BASE)
    {
        __text_start = .;
        *(.text.boot)      /* Entry point first */
        *(.text .text.*)
        *(.ltext .ltext.*)
    }
    . = ALIGN(4K);
    __text_end = .;

    /* Read-only data - catch all rodata variants (RO + NX) */
    .rodata : AT(ADDR(.rodata) - KERNEL_VIRT_BASE)
    {
        __rodata_start = .;
        *(.rodata .rodata.*)
        *(.lrodata .lrodata.*)
    }
    . = ALIGN(4K);
    __rodata_end = .;

    /* Initialized data (RW + NX); merged with .bss below into one RW+NX range) */
    .data : AT(ADDR(.data) - KERNEL_VIRT_BASE)
    {
        __data_start = .;
        *(.data .data.*)
        *(.ldata .ldata.*)
        /*
         * #63 B5: place the relocation-related sections explicitly. Without these lines
         * they are ORPHAN sections, and ld drops orphans wherever its own heuristic
         * prefers - in practice right after .rodata, i.e. *inside* the
         * [__rodata_start, __rodata_end) window, which the kernel-owned page table maps
         * RO+NX. That happens to be harmless today (relocation-model=static, so nothing
         * writes the GOT at runtime), but nothing pins it: the moment a codegen or target
         * change makes one of them need a write, CR0.WP turns it into a #PF that only
         * shows up at runtime, far from this file. Naming them here puts them in the
         * RW+NX range where relocation data belongs, independent of ld's heuristic.
         */
        *(.got .got.plt)
        *(.data.rel.ro .data.rel.ro.*)
    }

    /* Align to page boundary before BSS */
    . = ALIGN(4K);

    /* Uninitialized data (RW + NX) */
    .bss : AT(ADDR(.bss) - KERNEL_VIRT_BASE)
    {
        __bss_start = .;
        *(.bss .bss.*)
        *(.lbss .lbss.*)
        *(COMMON)
        __bss_end = .;
    }
    . = ALIGN(4K);
    __kernel_end = .;

    /* Discard unwanted sections */
    /DISCARD/ :
    {
        *(.eh_frame)
        /*
         * .eh_frame_hdr is the lookup index *for* .eh_frame, so it is dead weight once
         * .eh_frame is gone (the workspace builds with panic = "abort", so nothing
         * unwinds). Discarding it explicitly rather than leaving it an orphan is the
         * same #63 B5 reasoning as the .got placement above.
         */
        *(.eh_frame_hdr)
        *(.comment)
    }
}
