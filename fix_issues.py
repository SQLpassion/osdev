import re
import os

repo_dir = "/Users/klaus/.gemini/antigravity-cli/brain/5d580feb-02d7-4418-a261-2263cd1187a9/.system_generated/worktrees/subagent-Rust-Kernel-Developer--Issue--22--self-9f488aab/main64/kernel"

def replace_in_file(path, old, new):
    full = os.path.join(repo_dir, path)
    with open(full, 'r') as f:
        c = f.read()
    c = c.replace(old, new)
    with open(full, 'w') as f:
        f.write(c)

# L2: current_task_id returns packed ID
api_rs = "src/scheduler/roundrobin/api.rs"
with open(os.path.join(repo_dir, api_rs), 'r') as f:
    api_c = f.read()
api_c = api_c.replace(
    "pub fn current_task_id() -> Option<usize> {\n    with_scheduler(|meta| meta.running_slot)\n}",
    "pub fn current_task_id() -> Option<usize> {\n    with_scheduler(|meta| {\n        meta.running_slot\n            .map(|slot| super::types::pack_task_id(slot, meta.slots[slot].generation))\n    })\n}"
)
with open(os.path.join(repo_dir, api_rs), 'w') as f:
    f.write(api_c)

# L3: Task-id generation truncation
# In types.rs
types_rs = "src/scheduler/roundrobin/types.rs"
with open(os.path.join(repo_dir, types_rs), 'r') as f:
    types_c = f.read()
types_c = types_c.replace("pub generation: u64", "pub generation: u64") # Wait, how to fix L3?
# The instruction says: "Store the truncated generation in the slot too." or "Store the truncated generation in the slot"
# Let's see manager.rs where generation is incremented.

