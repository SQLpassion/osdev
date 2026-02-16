//! Hello World demo application
//!
//! A simple application that demonstrates the app framework.
//! Shows colored output and waits for user input before exiting.

use super::AppContext;
use crate::drivers::screen::Color;
use core::fmt::Write;

/// Application entry point.
pub fn app_main(ctx: &mut AppContext) {
    // Display a colorful title
    ctx.screen.set_color(Color::LightCyan);
    writeln!(ctx.screen, "========================================").unwrap();
    writeln!(ctx.screen, "          Hello World App").unwrap();
    writeln!(ctx.screen, "========================================").unwrap();
    writeln!(ctx.screen).unwrap();

    // Display the main message
    ctx.screen.set_color(Color::White);
    writeln!(ctx.screen, "Hello from a pseudo-application!").unwrap();
    writeln!(ctx.screen).unwrap();
    writeln!(ctx.screen, "This app has its own screen context.").unwrap();
    writeln!(
        ctx.screen,
        "The REPL screen was saved before this app started,"
    )
    .unwrap();
    writeln!(ctx.screen, "and will be restored when this app exits.").unwrap();
    writeln!(ctx.screen).unwrap();

    // Prompt for exit
    ctx.screen.set_color(Color::Yellow);
    write!(ctx.screen, "Press Enter to return to the REPL...").unwrap();

    // Wait for Enter
    ctx.wait_for_enter();
}
