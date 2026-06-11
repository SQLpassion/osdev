//! TUI application — four-tab event loop
//!
//! `TuiApp` owns all widgets and drives the blocking event loop.  On each
//! iteration it:
//!
//! 1. Draws the tab bar.
//! 2. Draws the widgets for the currently active tab.
//! 3. Draws the status bar.
//! 4. Blocks on `read_key_blocking` for the next keyboard event.
//! 5. Dispatches the event:
//!    - Left / Right → switch tabs
//!    - Up / Down    → navigate the focused widget on the current tab
//!    - q / Escape   → exit
//! 6. Repeats.
//!
//! # Tab layout
//!
//! ```
//! Row  0 : [  Info  ] [  Memory  ] [  Devices  ] [  System  ]  ← Tabs
//! Rows 1-23: content area (varies per tab)
//! Row 24 : Status bar
//! ```

use crate::drivers::keyboard::{self, Key};
use crate::drivers::screen::{Color, with_screen};
use crate::tui::{
    Gauge, Label, List, ProgressBar, Table, Tabs, TextBox, TreeView, SCREEN_COLS, SCREEN_ROWS,
};

/// Number of demo tabs (must match the tabs added in `run_demo`).
const TAB_COUNT: usize = 5;

/// Top-level TUI application.
///
/// Owns every widget used across all tabs.  The active tab determines which
/// subset is drawn on each frame.
pub struct TuiApp {
    // ── Navigation ────────────────────────────────────────────────
    tabs: Tabs,

    // ── Tab 0: Info ───────────────────────────────────────────────
    info_box: TextBox,

    // ── Tab 1: Memory ─────────────────────────────────────────────
    mem_header: Label,
    heap_gauge: Gauge,
    pmm_gauge: Gauge,
    mem_table: Table,

    // ── Tab 2: Devices ────────────────────────────────────────────
    devices_list: List,

    // ── Tab 3: System ─────────────────────────────────────────────
    sys_header:     Label,
    sys_bar_header: Label,
    cpu_gauge:      Gauge,
    heap_gauge2:    Gauge,
    pages_gauge:    Gauge,
    tasks_gauge:    Gauge,
    ata_gauge:      Gauge,
    fat_gauge:      Gauge,
    bar1:           ProgressBar,
    bar2:           ProgressBar,
    bar3:           ProgressBar,

    // ── Tab 4: Tree ───────────────────────────────────────────────
    tree_view: TreeView,
}

impl TuiApp {
    /// Construct a `TuiApp` from pre-built widgets.
    ///
    /// The large number of parameters mirrors the flat, no-heap widget storage
    /// model: every widget is moved into the app struct and lives on the task
    /// stack for the duration of the TUI session.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tabs:           Tabs,
        info_box:       TextBox,
        mem_header:     Label,
        heap_gauge:     Gauge,
        pmm_gauge:      Gauge,
        mem_table:      Table,
        devices_list:   List,
        sys_header:     Label,
        sys_bar_header: Label,
        cpu_gauge:      Gauge,
        heap_gauge2:    Gauge,
        pages_gauge:    Gauge,
        tasks_gauge:    Gauge,
        ata_gauge:      Gauge,
        fat_gauge:      Gauge,
        bar1:           ProgressBar,
        bar2:           ProgressBar,
        bar3:           ProgressBar,
        tree_view:      TreeView,
    ) -> Self {
        Self {
            tabs,
            info_box,
            mem_header,
            heap_gauge,
            pmm_gauge,
            mem_table,
            devices_list,
            sys_header,
            sys_bar_header,
            cpu_gauge,
            heap_gauge2,
            pages_gauge,
            tasks_gauge,
            ata_gauge,
            fat_gauge,
            bar1,
            bar2,
            bar3,
            tree_view,
        }
    }

    /// Enter the blocking event loop.
    ///
    /// Renders the initial frame, then processes key events until the user
    /// presses **q**, **Q**, or **Escape**.
    pub fn run(&mut self) {
        // Step 1: Suspend default flushing during TUI session to batch frame draws.
        with_screen(|screen| screen.set_suspend_flush(true));

        // Render the initial frame before waiting for any input.
        self.draw_frame();

        loop {
            let key = keyboard::read_key_blocking();

            match key {
                // Tab navigation.
                Key::ArrowLeft  => {
                    self.tabs.select_prev();
                    self.clear_content_area();
                }
                Key::ArrowRight => {
                    self.tabs.select_next();
                    self.clear_content_area();
                }

                // Widget-internal navigation (up/down arrows dispatch to the
                // focused widget on the current tab).
                Key::ArrowUp   => self.navigate_up(),
                Key::ArrowDown => self.navigate_down(),

                // Keyboard selection/expansion trigger.
                Key::Enter => {
                    // Step 1: If on the TreeView tab, toggle expansion of the node.
                    if self.tabs.active() == 4 {
                        self.tree_view.toggle_selected();
                    }
                }

                // Quit signals.
                Key::Escape | Key::Char(b'q') | Key::Char(b'Q') => break,

                // All other keys are ignored.
                _ => {}
            }

            self.draw_frame();
        }

        // Step 2: Restore physical screen flush behavior when returning to REPL.
        with_screen(|screen| screen.set_suspend_flush(false));
    }

    // -----------------------------------------------------------------------
    // Rendering
    // -----------------------------------------------------------------------

    /// Draw the tab bar, the active tab's content, and the status bar.
    fn draw_all(&self) {
        self.tabs.draw();
        self.draw_tab_content();
        self.draw_status_bar();
    }

    /// Render all layouts to backbuffer and perform a single atomic commit.
    fn draw_frame(&mut self) {
        self.draw_all();

        // Temporarily activate flush to write the complete frame.
        with_screen(|screen| {
            screen.set_suspend_flush(false);
            screen.flush();
            screen.set_suspend_flush(true);
        });
    }

    /// Draw the content widgets for the currently active tab.
    fn draw_tab_content(&self) {
        match self.tabs.active() {
            0 => {
                // Tab 0 — Info: single TextBox covering the content area.
                self.info_box.draw();
            }
            1 => {
                // Tab 1 — Memory: header label + two gauges + table.
                self.mem_header.draw();
                self.heap_gauge.draw();
                self.pmm_gauge.draw();
                self.mem_table.draw();
            }
            2 => {
                // Tab 2 — Devices: scrollable list.
                self.devices_list.draw();
            }
            3 => {
                // Tab 3 — System: gauges section + standalone progress bars.
                self.sys_header.draw();
                self.cpu_gauge.draw();
                self.heap_gauge2.draw();
                self.pages_gauge.draw();
                self.tasks_gauge.draw();
                self.ata_gauge.draw();
                self.fat_gauge.draw();
                self.sys_bar_header.draw();
                self.bar1.draw();
                self.bar2.draw();
                self.bar3.draw();
            }
            4 => {
                // Tab 4 — Tree: hierarchical TreeView control.
                self.tree_view.draw();
            }
            _ => {}
        }
    }

    /// Clear the content area (rows 1..SCREEN_ROWS-1) with black blanks.
    ///
    /// Called when switching tabs to prevent stale content from the previous
    /// tab bleeding through where the new tab has fewer or smaller widgets.
    fn clear_content_area(&self) {
        with_screen(|screen| {
            screen.fill_rect(
                1,
                0,
                SCREEN_COLS,
                SCREEN_ROWS - 2, // rows 1..23 (row 24 is the status bar)
                b' ',
                Color::White,
                Color::Black,
            );
        });
    }

    /// Dispatch an upward navigation event to the focused widget on the
    /// current tab.
    fn navigate_up(&mut self) {
        match self.tabs.active() {
            1 => self.mem_table.select_prev(),
            2 => self.devices_list.select_prev(),
            4 => self.tree_view.select_prev(),
            _ => {}
        }
    }

    /// Dispatch a downward navigation event to the focused widget on the
    /// current tab.
    fn navigate_down(&mut self) {
        match self.tabs.active() {
            1 => self.mem_table.select_next(),
            2 => self.devices_list.select_next(),
            4 => self.tree_view.select_next(),
            _ => {}
        }
    }

    // -----------------------------------------------------------------------
    // Status bar
    // -----------------------------------------------------------------------

    /// Render the status bar on the last screen row.
    ///
    /// Left side: current tab name + navigation hints.
    /// Right side: tab N/M counter.
    fn draw_status_bar(&self) {
        with_screen(|screen| {
            // Fill the entire status row.
            screen.fill_rect(
                SCREEN_ROWS - 1,
                0,
                SCREEN_COLS,
                1,
                b' ',
                Color::Black,
                Color::LightGray,
            );

            // Left hint text.
            screen.draw_at(
                SCREEN_ROWS - 1,
                1,
                "\x1B[0m\u{2190}\u{2192}: tab  \u{2191}\u{2193}: scroll  q/Esc: quit",
                Color::Black,
                Color::LightGray,
            );

            // Simpler ASCII hint (VGA only understands CP437).
            screen.draw_at(
                SCREEN_ROWS - 1,
                1,
                "Left/Right: tab    Up/Down: scroll    q / Esc: quit",
                Color::Black,
                Color::LightGray,
            );

            // Right side: "Tab N/M".
            let active = self.tabs.active() + 1; // 1-based
            let total  = TAB_COUNT;

            // Build "Tab N/M" manually (no format!, no alloc).
            // Maximum length: "Tab 4/4" = 7 chars.
            let mut buf = [b' '; 8];
            buf[0] = b'T'; buf[1] = b'a'; buf[2] = b'b'; buf[3] = b' ';
            buf[4] = b'0' + active as u8;
            buf[5] = b'/';
            buf[6] = b'0' + total as u8;

            let start = SCREEN_COLS - 9;
            for (i, &byte) in buf.iter().enumerate() {
                screen.draw_char_at(SCREEN_ROWS - 1, start + i, byte, Color::Black, Color::LightGray);
            }
        });
    }
}
