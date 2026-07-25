//! Kernel Console Abstraction Module
//!
//! Provides a unified trait `KernelConsole` and dynamic initialization for
//! VGA text-mode and graphics framebuffer consoles.

mod dispatch;
mod font_basic;
mod framebuffer;
// `interface` is `pub` (rather than private + re-exported like the other
// submodules) solely so integration tests can reach the test-only
// introspection hook `interface::last_flush_ran_with_interrupts_enabled`
// (see issue #16) via its full path, following the same "plain `pub` +
// `#[doc(hidden)]`" convention used by e.g. `block::reset_active_device`.
pub mod interface;
mod vga;

pub use dispatch::ConsoleImpl;
pub use framebuffer::FramebufferConsole;
pub use interface::{init, with_console, KernelConsole};
pub use vga::VgaConsole;
