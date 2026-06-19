# Implementation Plan: Parallel Boot Paths (BIOS & UEFI) for KAOS

This document describes the step-by-step implementation plan to keep KAOS bootable via both Legacy BIOS and UEFI. The goal is to use the **same** kernel binary unmodified for both environments.

---

## 1. System Architecture Overview

```
┌────────────────────────┐      ┌────────────────────────┐
│    Legacy BIOS Boot    │      │       UEFI Boot        │
│    (kaosldr_16/64)     │      │     (kaosldr_uefi)     │
└───────────┬────────────┘      └───────────┬────────────┘
            │                               │
            │  Populates                    │  Populates
            ▼                               ▼
 ┌──────────────────────────────────────────────────────┐
 │              Shared BootInfo Structure               │
 ├──────────────────────────────────────────────────────┤
 │ - VideoMode (VgaText or GopFramebuffer)              │
 │ - MemoryMap (Unified list of free memory regions)    │
 │ - KernelSize / System Data                           │
 └──────────────────────────┬───────────────────────────┘
                            │
                            │  Passed in RDI
                            ▼
               ┌─────────────────────────┐
               │    KAOS Rust Kernel     │
               ├─────────────────────────┤
               │     Boot-Agnostic       │
               │   KernelMain(BootInfo)  │
               └─────────────────────────┘
```

---

## Phase 1: Define the Interface (`BootInfo`)

Since the bootloader `kaosldr_uefi` and the BIOS loader `kaosldr_64` are standalone, minimal binaries, we define the interface as matching data structures with a fixed memory layout (`#[repr(C)]`).

### 1.1 Structure Definitions (`boot_info.rs`)
These structures will be defined in both the kernel and the two bootloaders:

```rust
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoModeType {
    VgaText = 0,
    GopFramebuffer = 1,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub base_address: u64,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scanline: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnifiedMemoryEntry {
    pub start: u64,
    pub size: u64,
    pub is_usable: bool,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub magic: u64,               // E.g., 0x4B414F535F424F4F ("KAOS_BOO")
    pub video_type: VideoModeType,
    pub fb_info: FramebufferInfo,  // Only valid when GopFramebuffer
    pub memory_map_addr: u64,      // Pointer to an array of UnifiedMemoryEntry
    pub memory_map_len: u32,       // Number of entries
    pub kernel_size: u64,
}
```

---

## Phase 2: Adaptations of the Bootloaders

### 2.1 Legacy BIOS Loader (`kaosldr_64`)
1. **Memory Map Translation**: `kaosldr_64` already reads the E820 table. These entries must be copied into an array of `UnifiedMemoryEntry`.
2. **Build BootInfo**:
   - Set `video_type` to `VideoModeType::VgaText`.
   - Fill `fb_info` with zeros.
   - Set the pointer to the translated E820 array.
3. **Handover**: When executing the kernel (`execute_kernel`), load the address of the `BootInfo` struct into the `RDI` register instead of the raw `kernel_size`.

### 2.2 UEFI Loader (`kaosldr_uefi`)
1. **Query GOP Data**: Query GOP and populate `fb_info`.
2. **Memory Map Translation**:
   - Retrieve the UEFI Memory Map.
   - Translate the `EFI_MEMORY_DESCRIPTOR` entries into the `UnifiedMemoryEntry` format (i.e., marking types like `EfiConventionalMemory` as `is_usable = true`).
3. **Exit Boot Services**: Call `ExitBootServices`.
4. **Handover**: Call the kernel, passing the pointer to the `BootInfo` structure (which resides in bootloader-allocated memory) in `RDI`.

---

## Phase 3: Kernel Adaptations (Making it Boot-Agnostic)

### 3.1 KernelMain Signature & Routing
Modify `KernelMain` in `kernel/src/main.rs`:

```rust
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(boot_info_ptr: *const BootInfo) -> ! {
    // 1. Zero BSS
    unsafe { zero_bss(); }

    // 2. Initialize debug serial (always works)
    serial::init();

    // 3. Validate BootInfo
    let boot_info = unsafe { &*boot_info_ptr };
    assert_eq!(boot_info.magic, 0x4B414F535F424F4F, "Invalid BootInfo magic!");

    // 4. Initialize console based on boot type
    tui::init(&boot_info.video_type, &boot_info.fb_info);
    
    // 5. Feed the Physical Memory Manager (PMM) with the unified memory map
    pmm::init_from_map(boot_info.memory_map_addr, boot_info.memory_map_len);

    // Further kernel startup...
}
```

### 3.2 Screen Abstraction (`kernel/src/tui/`)
1. Declare a common output trait (e.g., `KernelConsole`).
2. Create a `VgaConsole` (reusing the existing code writing to `0xB8000`).
3. Create a `GopConsole` that renders pixels using a font (font glyphs).
4. Use a dynamic or static dispatch system (e.g., a global static `Spinlock<Option<Box<dyn KernelConsole>>>`) populated during `tui::init`.

---

## Phase 4: Step-by-Step Testing

1. **Step 1 (Maintain BIOS Stability)**:
   - Adapt `kaosldr_64` and the kernel to use the `BootInfo` format.
   - Verify that KAOS still boots successfully via BIOS (`./build_kaos_debug.sh`) and that the VGA console functions correctly.
2. **Step 2 (UEFI Milestone 1 Integration)**:
   - Implement the UEFI loader to populate `BootInfo` and start the kernel.
   - Add a simple routine to the kernel's `GopConsole` to paint a test pattern (color gradient).
   - Boot UEFI via `./build_uefi.sh` and verify the graphical output.
