//! Pseudo-application execution framework
//!
//! Applications are statically registered and run as dedicated scheduler tasks.
//! The REPL launches an app task and waits until it exits.

use crate::drivers::keyboard;
use crate::drivers::screen::Screen;
use crate::scheduler::{self, KernelTaskFn, SpawnError};
use core::fmt::Write;

mod counter;
mod hello;

/// Application entry point function signature.
/// Apps receive an AppContext providing screen and keyboard access.
pub type AppFn = fn(&mut AppContext);

/// Error returned when launching an app task.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunAppError {
    /// No application with this name exists.
    UnknownApp,
    /// Scheduler rejected task creation.
    SpawnFailed(SpawnError),
}

/// Static application registry entry.
pub struct AppEntry {
    /// Command name used to invoke the app (e.g., "hello")
    pub name: &'static str,
    /// Brief description shown in app list
    pub description: &'static str,
    /// Task entry point used by the scheduler.
    pub task_entry: KernelTaskFn,
}

/// Static registry of all available applications.
/// Add new apps here to make them available via "run <name>".
static APPS: &[AppEntry] = &[
    AppEntry {
        name: "hello",
        description: "Simple hello world demo",
        task_entry: hello_task_entry,
    },
    AppEntry {
        name: "counter",
        description: "Interactive counter demo",
        task_entry: counter_task_entry,
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
        keyboard::read_char()
    }

    /// Read a character (blocking, waits for input).
    ///
    /// Uses the keyboard worker's wait-queue so the calling task sleeps
    /// instead of busy-waiting.
    pub fn read_char(&mut self) -> u8 {
        keyboard::read_char_blocking()
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

/// Spawn an application as its own kernel task and return the task slot ID.
pub fn spawn_app(name: &str) -> Result<usize, RunAppError> {
    let app = find_app(name).ok_or(RunAppError::UnknownApp)?;
    scheduler::spawn(app.task_entry).map_err(RunAppError::SpawnFailed)
}

/// Shared app-task launcher: full-screen app context then task exit.
fn run_registered_app(app_fn: AppFn) -> ! {
    let mut screen = Screen::new();
    screen.clear();

    {
        let mut ctx = AppContext::new(&mut screen);
        app_fn(&mut ctx);
    }

    scheduler::exit_current_task();
}

/// Scheduler task entry point for the `hello` app.
extern "C" fn hello_task_entry() -> ! {
    run_registered_app(hello::app_main)
}

/// Scheduler task entry point for the `counter` app.
extern "C" fn counter_task_entry() -> ! {
    run_registered_app(counter::app_main)
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
