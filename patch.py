import sys
path = "/Users/klaus/.gemini/antigravity-cli/brain/5d580feb-02d7-4418-a261-2263cd1187a9/.system_generated/worktrees/subagent-Rust-Kernel-Developer--Issue--22--self-9f488aab/main64/kernel/src/boot_info.rs"
with open(path, "r") as f:
    content = f.read()

replacement = """pub static BOOT_INFO_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);

impl BootInfo {
    /// Returns a static reference to the active `BootInfo` structure, if it has been validated and published.
    pub fn get() -> Option<&'static BootInfo> {
        let ptr = BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Acquire);
        if ptr == 0 {
            None
        } else {
            // SAFETY:
            // - If the pointer is non-zero, it was validated in `KernelMain` via magic check.
            // - The boot loader guarantees the memory is valid and immutable for the kernel's lifetime.
            Some(unsafe { &*(ptr as *const BootInfo) })
        }
    }
}"""
content = content.replace("pub static BOOT_INFO_PTR: core::sync::atomic::AtomicU64 = core::sync::atomic::AtomicU64::new(0);", replacement)
with open(path, "w") as f:
    f.write(content)
