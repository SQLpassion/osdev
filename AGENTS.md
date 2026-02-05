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