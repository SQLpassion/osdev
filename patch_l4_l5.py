import sys

# Patch keyboard.rs
path_kb = "/Users/klaus/.gemini/antigravity-cli/brain/5d580feb-02d7-4418-a261-2263cd1187a9/.system_generated/worktrees/subagent-Rust-Kernel-Developer--Issue--22--self-9f488aab/main64/kernel/src/drivers/keyboard.rs"
with open(path_kb, "r") as f:
    content = f.read()

repl_kb = """        0x44 => {
            let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(10)));
            return;
        }
        0x57 => {
            let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(11)));
            return;
        }
        0x58 => {
            let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(12)));
            return;
        }"""
content = content.replace("""        0x44 => {
            let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(10)));
            return;
        }""", repl_kb)

with open(path_kb, "w") as f:
    f.write(content)

# Patch pci/mod.rs
path_pci = "/Users/klaus/.gemini/antigravity-cli/brain/5d580feb-02d7-4418-a261-2263cd1187a9/.system_generated/worktrees/subagent-Rust-Kernel-Developer--Issue--22--self-9f488aab/main64/kernel/src/drivers/pci/mod.rs"
with open(path_pci, "r") as f:
    content_pci = f.read()

content_pci = content_pci.replace(
    "// - Reading offset 0x0C is safe as it is a standard PCI configuration register.",
    "// - Reading offset 0x0E is safe as it is a standard PCI configuration register."
)

with open(path_pci, "w") as f:
    f.write(content_pci)

