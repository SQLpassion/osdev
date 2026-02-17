# AGENTS.md — OS Development (Rust Kernel)

This repository contains an experimental operating system.
The kernel is written in Rust, runs in Ring 0, and uses `#![no_std]`.

This file defines **binding rules** for AI coding agents (e.g., Codex)
that read/modify code in this repo.

---

## 1) Core Principles

- Correctness and safety come first (kernel code is security-sensitive)
- Prefer small, reviewable changes over large refactors
- Be explicit; avoid hidden behavior
- No silent ABI/behavior changes
- Maintain a small, well-audited `unsafe` surface
- Every change MUST be covered by tests
- Every functional change MUST introduce at least one new test

---

## 2) Toolchain & Rust Policy (Nightly Required)

- Rust **nightly** is required and technically necessary for this project.
- Do **not** remove nightly usage or rewrite code to avoid nightly unless explicitly asked.
- New nightly features are allowed only when justified (why needed, alternatives considered).
- The kernel is `#![no_std]` by default.

---

## 3) Dependency Policy (Non-Negotiable)

- **NO external dependencies are allowed.**
  - Do not add crates from crates.io.
  - Do not add git/path dependencies.
  - Do not add build-time helper crates.
- Use only:
  - `core`
  - (optionally) `alloc` **only if the repo already provides an allocator** and it is explicitly part of the design
- If a change seems to “need a crate”, implement the minimal required functionality in-tree instead.

---

## 4) Unsafe Code Policy (Very Important)

### 4.1 Default stance
`unsafe` is permitted, but never casual. Keep it minimal and localized.

### 4.2 Mandatory `SAFETY:` comment
Every `unsafe` block MUST include a `SAFETY:` comment that explains:
- the invariants relied upon
- why it is safe in this context
- what would make it unsafe

Example:

```rust
// SAFETY:
// - interrupts are disabled on this CPU
// - `ptr` is mapped and valid for `len` bytes
// - no aliasing mutable references exist during this scope
unsafe {
    core::ptr::copy_nonoverlapping(src, dst, len);
}
```

---

## 5) Inline Documentation Policy (Mandatory)

### 5.1 What MUST be documented inline
For every edited Rust function in kernel code, document **all non-trivial code blocks** with inline comments.

Non-trivial blocks include, at minimum:
- multi-step setup/teardown sequences
- ownership/lifetime transitions (e.g. who frees what)
- permission/state transitions (e.g. writable->readonly, user/supervisor, CR3 changes)
- error paths and early returns
- loops that mutate global/shared state
- branch paths with different safety or resource behavior
- any architecture-sensitive operations (paging/TLB/interrupt state/register writes)

### 5.2 What does NOT need extra comments
- trivial one-liners with obvious intent (simple getters/setters, direct returns)
- boilerplate that is already fully explained by the surrounding block comment

### 5.3 Comment quality rules
- Explain **why** the block exists and what invariants it preserves.
- Do not just restate syntax.
- Use short step markers (`Step 1`, `Step 2`, ...) for multi-phase flows.
- Keep comments synchronized with behavior when code changes.

### 5.4 Required when user asks for documentation
If the user asks to "document function X", ensure every non-trivial block in that function has an inline comment before considering the task done.

---

## 6) Code Layout & Formatting Baseline (Mandatory)

### 6.1 Canonical style reference
The canonical formatting/style reference is:
- `main64/kernel_rust/src/memory/vmm.rs`

All newly generated Rust kernel code MUST match this look & feel.

### 6.2 Required formatting behavior
- Use 4-space indentation, never tabs.
- Keep brace placement and control-flow layout in the same style as `vmm.rs`.
- Keep import grouping consistent with `vmm.rs` (logical groups separated by a blank line).
- Keep one blank line between top-level items (`const`, `struct`, `enum`, `impl`, `fn`).
- Inside functions, separate logical phases with blank lines exactly like in `vmm.rs`.
- For multi-line expressions/signatures/match arms, use the same wrapping style as `vmm.rs` (including trailing commas where applicable).
- Keep comments/doc-comments layout consistent (`///` for API docs, `//` for inline block rationale).

### 6.3 Change-scope formatting rule
- Do not reformat unrelated files/sections.
- When editing an existing file, preserve and extend the local formatting style already present in that file.
- If style conflicts arise, prefer the `vmm.rs` style baseline unless the user explicitly requests otherwise.
