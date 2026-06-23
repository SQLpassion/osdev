//! Kernel Console Abstraction Module
//!
//! Provides a unified trait `KernelConsole` and dynamic initialization for
//! VGA text-mode and graphics framebuffer consoles.

mod dispatch;
mod framebuffer;
mod interface;
mod vga;
mod font_basic;
mod font_alternative;

pub use dispatch::ConsoleImpl;
pub use framebuffer::FramebufferConsole;
pub use interface::{init, with_console, KernelConsole};
pub use vga::VgaConsole;
