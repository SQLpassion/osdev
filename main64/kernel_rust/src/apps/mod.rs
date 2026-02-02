//! Pseudo-application execution framework
//!
//! Provides a simple way to run "applications" that are statically registered
//! functions. The kernel saves screen state, runs the app with full-screen
//! access, then restores the screen when the app returns.
//!
//! This implements the same concept as xterm's "alternate screen buffer" -
//! apps get a clean canvas and the original screen is preserved.

use crate::drivers::keyboard;
use crate::drivers::screen::Screen;
use core::fmt::Write;

mod counter;
mod hello;

/// Application entry point function signature.
/// Apps receive an AppContext providing screen and keyboard access.
pub type AppFn = fn(&mut AppContext);

/// Static application registry entry.
pub struct AppEntry {
    /// Command name used to invoke the app (e.g., "hello")
    pub name: &'static str,
    /// Brief description shown in app list
    pub description: &'static str,
    /// Entry point function
    pub func: AppFn,
}

/// Static registry of all available applications.
/// Add new apps here to make them available via "run <name>".
static APPS: &[AppEntry] = &[
    AppEntry {
        name: "hello",
        description: "Simple hello world demo",
        func: hello::app_main,
    },
    AppEntry {
        name: "counter",
        description: "Interactive counter demo",
        func: counter::app_main,
    },
];

/// Context passed to applications for I/O operations.
/// Provides access to the screen and keyboard input helpers.
pub struct AppContext<'a> {
    /// Mutable reference to the screen for drawing
    pub screen: &'a mut Screen,
}

impl<'a> AppContext<'a> {
    /// Create a new application context.
    fn new(screen: &'a mut Screen) -> Self {
        Self { screen }
    }

    /// Read a line of input (blocking), echoing characters to screen.
    /// Returns the number of bytes written to the buffer.
    /// The newline is echoed but not stored in the buffer.
    #[allow(dead_code)]
    pub fn read_line(&mut self, buf: &mut [u8]) -> usize {
        keyboard::read_line(self.screen, buf)
    }

    /// Try to read a character (non-blocking).
    /// Returns None if no input is available.
    #[allow(dead_code)]
    pub fn try_read_char(&mut self) -> Option<u8> {
        keyboard::poll();
        keyboard::read_char()
    }

    /// Read a character (blocking, waits for input).
    pub fn read_char(&mut self) -> u8 {
        loop {
            keyboard::poll();
            if let Some(ch) = keyboard::read_char() {
                return ch;
            }
            // Sleep until the next interrupt to avoid busy-waiting
            unsafe {
                core::arch::asm!("hlt");
            }
        }
    }

    /// Wait for the Enter key to be pressed.
    /// Useful for "Press Enter to continue" prompts.
    pub fn wait_for_enter(&mut self) {
        loop {
            let ch = self.read_char();
            if ch == b'\n' || ch == b'\r' {
                return;
            }
        }
    }
}

/// Find an application by name (case-insensitive).
fn find_app(name: &str) -> Option<&'static AppEntry> {
    APPS.iter().find(|app| app.name.eq_ignore_ascii_case(name))
}

/// Run an application by name.
///
/// This function:
/// 1. Saves the current screen state
/// 2. Clears the screen for the app
/// 3. Runs the app with an AppContext
/// 4. Restores the original screen when the app returns
///
/// Returns true if the app was found and executed, false otherwise.
pub fn run_app(name: &str, screen: &mut Screen) -> bool {
    let Some(app) = find_app(name) else {
        return false;
    };

    // Save current screen state (VGA buffer + cursor + colors)
    let snapshot = screen.save();

    // Clear screen for the application
    screen.clear();

    // Create context and run the application
    {
        let mut ctx = AppContext::new(screen);
        (app.func)(&mut ctx);
    }

    // Restore original screen state
    screen.restore(&snapshot);

    true
}

/// List all available applications to the screen.
pub fn list_apps(screen: &mut Screen) {
    writeln!(screen, "Available applications:").unwrap();
    for app in APPS {
        writeln!(screen, "  {:12} - {}", app.name, app.description).unwrap();
    }
    writeln!(screen).unwrap();
    writeln!(screen, "Use 'run <name>' to launch an application.").unwrap();
}
