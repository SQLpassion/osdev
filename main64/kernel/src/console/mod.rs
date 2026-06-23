//! Kernel Console Abstraction Module
//!
//! Provides a unified trait `KernelConsole` and dynamic initialization for
//! VGA text-mode and graphics framebuffer consoles.

mod interface;
mod dispatch;
mod vga;
mod framebuffer;

pub use interface::{init, with_console, KernelConsole};
pub use dispatch::ConsoleImpl;
pub use vga::VgaConsole;
pub use framebuffer::FramebufferConsole;
