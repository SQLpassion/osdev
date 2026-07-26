import sys
path = "/Users/klaus/.gemini/antigravity-cli/brain/5d580feb-02d7-4418-a261-2263cd1187a9/.system_generated/worktrees/subagent-Rust-Kernel-Developer--Issue--22--self-9f488aab/main64/kernel/src/main.rs"
with open(path, "r") as f:
    content = f.read()

import re

# We will remove the SAFETY comments preceding these lines where appropriate, but it's easier to just replace the line and keep the comment for now, or replace both.
# But wait, keeping the SAFETY comment for safe code is bad practice, clippy might warn or it's just confusing.
# Let's replace the block:
repl1 = """            // SAFETY:
            // - The magic check succeeded, indicating the pointer points to a valid BootInfo struct.
            let boot_info = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };"""
content = content.replace(repl1, "            let boot_info = boot_info::BootInfo::get().unwrap();")

repl2 = "let boot_info = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };"
content = content.replace(repl2, "let boot_info = boot_info::BootInfo::get().unwrap();")

repl3 = """        // SAFETY:
        // - `boot_info_raw` was validated above in `KernelMain` to ensure it points to a valid `BootInfo` structure.
        // - Dereferencing is read-only and within bounds.
        let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };"""
content = content.replace(repl3, "        let bi = boot_info::BootInfo::get().unwrap();")

repl4 = """        // SAFETY:
        // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
        let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };"""
content = content.replace(repl4, "        let bi = boot_info::BootInfo::get().unwrap();")

repl5 = """    // SAFETY:
    // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
    // - This structure is published in `KernelMain` and has been validated by the bootloader.
    // - The memory range is mapped and valid for read access.
    // - Structure alignment is guaranteed by `#[repr(C)]`.
    // - If `boot_info_raw` was null or pointing to invalid memory, this dereference would trigger a page fault.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };"""
content = content.replace(repl5, "    let bi = boot_info::BootInfo::get().unwrap();")

repl6 = """    // SAFETY:
    // - `boot_info_raw` contains a valid physical address to a `BootInfo` structure.
    // - The structure is mapped, valid for reads, and alignment is guaranteed by `#[repr(C)]`.
    // - If it was invalid, the dereference would cause a page fault.
    let bi = unsafe { &*(boot_info_raw as *const boot_info::BootInfo) };"""
content = content.replace(repl6, "    let bi = boot_info::BootInfo::get().unwrap();")

with open(path, "w") as f:
    f.write(content)
