//! Interactive counter demo application
//!
//! Demonstrates an interactive application with keyboard input.
//! The user can increment/decrement a counter and see the screen
//! update in real-time.

use super::AppContext;
use crate::drivers::screen::Color;
use core::fmt::Write;

/// Application entry point.
pub fn app_main(ctx: &mut AppContext) {
    let mut count: i32 = 0;

    loop {
        // Redraw the entire screen
        draw_screen(ctx, count);

        // Wait for and handle input
        let ch = ctx.read_char();

        match ch {
            b'+' | b'=' => count = count.saturating_add(1),
            b'-' => count = count.saturating_sub(1),
            b'0' => count = 0,
            b'q' | b'Q' => break,
            _ => {}
        }
    }
}

/// Draw the counter screen.
fn draw_screen(ctx: &mut AppContext, count: i32) {
    ctx.screen.clear();

    // Title
    ctx.screen.set_color(Color::LightGreen);
    writeln!(ctx.screen, "========================================").unwrap();
    writeln!(ctx.screen, "          Counter App").unwrap();
    writeln!(ctx.screen, "========================================").unwrap();
    writeln!(ctx.screen).unwrap();

    // Current count display
    ctx.screen.set_color(Color::White);
    write!(ctx.screen, "Current count: ").unwrap();

    // Color the number based on value
    if count > 0 {
        ctx.screen.set_color(Color::LightGreen);
    } else if count < 0 {
        ctx.screen.set_color(Color::LightRed);
    } else {
        ctx.screen.set_color(Color::Yellow);
    }
    writeln!(ctx.screen, "{}", count).unwrap();

    writeln!(ctx.screen).unwrap();

    // Controls
    ctx.screen.set_color(Color::LightGray);
    writeln!(ctx.screen, "Controls:").unwrap();
    writeln!(ctx.screen, "  + or =    Increment counter").unwrap();
    writeln!(ctx.screen, "  -         Decrement counter").unwrap();
    writeln!(ctx.screen, "  0         Reset to zero").unwrap();
    writeln!(ctx.screen, "  q         Quit and return to REPL").unwrap();
}
