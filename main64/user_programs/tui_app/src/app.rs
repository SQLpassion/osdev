//! TUI application — five-tab event loop and demo launcher.
//!
//! `TuiApp` owns all widgets and drives the blocking event loop. On each
//! iteration it:
//!
//! 1. Draws the tab bar.
//! 2. Draws the widgets for the currently active tab.
//! 3. Draws the status bar.
//! 4. Blits the back-buffer to VGA via `WriteFramebuffer` syscall.
//! 5. Blocks on `ReadKey` syscall for the next keyboard event.
//! 6. Dispatches the event and repeats.
//!
//! The `run_demo()` function is the single public entry point: it builds all
//! widgets with static demo data, enters the event loop, and restores the
//! terminal state on exit.

extern crate alloc;

use alloc::vec;
use lib_kaos::console::{self, Key};
use lib_tui::{
    Gauge, Label, List, ProgressBar, Table, Tabs, TextBox, TreeNode, TreeView,
    SCREEN_COLS, SCREEN_ROWS, Color, with_screen,
};

/// Number of demo tabs.
const TAB_COUNT: usize = 5;

/// Top-level TUI application — owns every widget across all five tabs.
pub struct TuiApp {
    /// Horizontal tab bar at the top of the screen (row 0).
    tabs:           Tabs,
    /// Tab 0: Multi-line description box detailing system specs.
    info_box:       TextBox,
    /// Tab 1: Section title label for the memory tab.
    mem_header:     Label,
    /// Tab 1: Horizontal meter visualizing kernel heap allocation status.
    heap_gauge:     Gauge,
    /// Tab 1: Horizontal meter visualizing PMM page allocation status.
    pmm_gauge:      Gauge,
    /// Tab 1: Table displaying physical & virtual memory region maps.
    mem_table:      Table,
    /// Tab 2: Scrollable menu listing detected system hardware components.
    devices_list:   List,
    /// Tab 3: Section title label for system metrics.
    sys_header:     Label,
    /// Tab 3: Section title label for standalone progress bars.
    sys_bar_header: Label,
    /// Tab 3: Gauge showing simulated CPU usage metrics.
    cpu_gauge:      Gauge,
    /// Tab 3: Gauge showing duplicate heap statistics.
    heap_gauge2:    Gauge,
    /// Tab 3: Gauge showing physical memory allocation.
    pages_gauge:    Gauge,
    /// Tab 3: Gauge showing the count of running scheduler tasks.
    tasks_gauge:    Gauge,
    /// Tab 3: Gauge showing ATA PIO read speed metrics.
    ata_gauge:      Gauge,
    /// Tab 3: Gauge showing FAT12 disk usage capacity.
    fat_gauge:      Gauge,
    /// Tab 3: Progress bar instance 1.
    bar1:           ProgressBar,
    /// Tab 3: Progress bar instance 2.
    bar2:           ProgressBar,
    /// Tab 3: Progress bar instance 3.
    bar3:           ProgressBar,
    /// Tab 4: Nested node tree illustrating device organization layout.
    tree_view:      TreeView,
}

impl TuiApp {
    /// Instantiates a new TuiApp container with all required widgets.
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
            tabs, info_box, mem_header, heap_gauge, pmm_gauge, mem_table,
            devices_list, sys_header, sys_bar_header, cpu_gauge, heap_gauge2,
            pages_gauge, tasks_gauge, ata_gauge, fat_gauge, bar1, bar2, bar3,
            tree_view,
        }
    }

    /// Enter the main blocking event loop.
    pub fn run(&mut self) {
        // Step 1: Draw the initial frame prior to waiting for keys.
        self.draw_frame();

        // Step 2: Loop indefinitely, blocking on keyboard events.
        loop {
            let key = console::read_key().unwrap_or(Key::Unknown);
            match key {
                // Left/Right: Navigate between tabs.
                Key::ArrowLeft  => { self.tabs.select_prev(); self.clear_content_area(); }
                Key::ArrowRight => { self.tabs.select_next(); self.clear_content_area(); }
                // Up/Down: Scroll current tab list/table widgets.
                Key::ArrowUp    => self.navigate_up(),
                Key::ArrowDown  => self.navigate_down(),
                // Enter: Perform actions on active tab widgets (e.g. toggle tree nodes).
                Key::Enter => {
                    if self.tabs.active() == 4 { self.tree_view.toggle_selected(); }
                }
                // Quit triggers (q, Q, or Escape).
                Key::Escape | Key::Char(b'q') | Key::Char(b'Q') => break,
                _ => {}
            }
            // Step 3: Redraw widgets with the updated state.
            self.draw_frame();
        }
    }

    /// Helper: Refreshes all drawings and blits the buffer to VGA.
    fn draw_frame(&mut self) {
        self.draw_all();
        with_screen(|screen| screen.flush());
    }

    /// Helper: Iterates through UI sections.
    fn draw_all(&self) {
        self.tabs.draw();
        self.draw_tab_content();
        self.draw_status_bar();
    }

    /// Helper: Draws the widgets that correspond to the active tab selection index.
    fn draw_tab_content(&self) {
        match self.tabs.active() {
            0 => { self.info_box.draw(); }
            1 => {
                self.mem_header.draw();
                self.heap_gauge.draw();
                self.pmm_gauge.draw();
                self.mem_table.draw();
            }
            2 => { self.devices_list.draw(); }
            3 => {
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
            4 => { self.tree_view.draw(); }
            _ => {}
        }
    }

    /// Helper: Overwrites the central screen space with black spaces before switching tabs.
    fn clear_content_area(&self) {
        with_screen(|screen| {
            screen.fill_rect(1, 0, SCREEN_COLS, SCREEN_ROWS - 2, b' ', Color::White, Color::Black);
        });
    }

    /// Helper: Dispatches scroll up requests.
    fn navigate_up(&mut self) {
        match self.tabs.active() {
            1 => self.mem_table.select_prev(),
            2 => self.devices_list.select_prev(),
            4 => self.tree_view.select_prev(),
            _ => {}
        }
    }

    /// Helper: Dispatches scroll down requests.
    fn navigate_down(&mut self) {
        match self.tabs.active() {
            1 => self.mem_table.select_next(),
            2 => self.devices_list.select_next(),
            4 => self.tree_view.select_next(),
            _ => {}
        }
    }

    /// Helper: Draws the bottom footer showing help keybindings and active page status.
    fn draw_status_bar(&self) {
        with_screen(|screen| {
            // Fill background of row 24 with LightGray.
            screen.fill_rect(SCREEN_ROWS - 1, 0, SCREEN_COLS, 1, b' ', Color::Black, Color::LightGray);
            // Print left help binding text.
            screen.draw_at(
                SCREEN_ROWS - 1, 1,
                "Left/Right: tab    Up/Down: scroll    q / Esc: quit",
                Color::Black, Color::LightGray,
            );

            // Format current tab index fraction (e.g. "Tab 3/5") and print it right-aligned.
            let active = self.tabs.active() + 1;
            let total  = TAB_COUNT;
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

/// Build and run the five-tab TUI demo with static data.
///
/// Called by `tui_app`'s `_start`. Constructs all widgets from compile-time
/// constants, runs the event loop, then restores VGA state before returning.
pub fn run_demo() {
    // Step 1: Clear screen, disable VGA blink mode and hardware cursor.
    console::clear_screen().ok();
    console::set_vga_mode(0b00).ok();

    // ------------------------------------------------------------------
    // Tab 0 — Info (TextBox)
    // ------------------------------------------------------------------
    let info_lines: &[&'static str] = &[
        " KAOS - Klaus' Amateur Operating System",
        " =========================================",
        "",
        " Architecture : x86-64, Long Mode (64-bit)",
        " Kernel Type  : Monolithic, #[no_std], Ring 0",
        " Language     : Rust (nightly toolchain)",
        " Entry Point  : KernelMain() via bootloader",
        "",
        " Subsystems:",
        "   Physical Memory Manager (PMM)  - free-list allocator",
        "   Virtual Memory Manager  (VMM)  - 4-level paging",
        "   Heap Allocator          (Heap) - slab-style, alloc crate",
        "   Cooperative Scheduler          - round-robin, per-task stack",
        "   PS/2 Keyboard Driver           - IRQ1, dual ring buffer",
        "   VGA Text Mode Driver           - direct MMIO, 80x25, 16 colors",
        "   ATA PIO Driver                 - LBA28, read/write sectors",
        "   FAT12 File System              - read-only, root dir listing",
        "   PCI Bus Scanner                - class/vendor lookup",
        "   Ring-3 Syscall Interface       - int 0x80, 23 syscalls",
        "   TUI Engine                     - this screen! (Ring-3)",
        "",
        " Navigate with arrow keys. Press q or Esc to return to shell.",
    ];
    let info_box = TextBox::new(
        1, 0, 80, 23, info_lines,
        Color::LightGray, Color::Black, Color::LightCyan,
    );

    // ------------------------------------------------------------------
    // Tab 1 — Memory (Gauge × 2 + Table)
    // ------------------------------------------------------------------
    let mem_header = Label::new(1, 0, 80, " Memory Subsystem", Color::Black, Color::LightGreen);
    let heap_gauge = Gauge::new(3, 2, 76, "Heap Memory:     ", 18, 62, Color::LightCyan, Color::LightGreen);
    let pmm_gauge  = Gauge::new(4, 2, 76, "Physical Pages:  ", 18, 44, Color::LightCyan, Color::Yellow);

    let mut mem_table = Table::new(6, 0, 80, 18, &["Region", "Address", "Size / Type"], &[22, 20, 28]);
    mem_table.add_row(&["Low Memory",         "0x00000000",         "640 KB | Usable"]);
    mem_table.add_row(&["BIOS ROM Area",       "0x000A0000",         "384 KB | Reserved"]);
    mem_table.add_row(&["VGA Buffer",          "0x000B8000",         "32 KB | MMIO"]);
    mem_table.add_row(&["Kernel Image",        "0x00100000",         "~1 MB | Kernel"]);
    mem_table.add_row(&["Extended Memory",     "0x00200000",         "~126 MB | Usable"]);
    mem_table.add_row(&["APIC MMIO",           "0xFEE00000",         "1 MB | MMIO"]);
    mem_table.add_row(&["ACPI Tables",         "0x7FC00000",         "4 MB | ACPI Reclaim"]);
    mem_table.add_row(&["Higher-Half Mapping", "0xFFFF800000000000", "Kernel VA | Virtual"]);

    // ------------------------------------------------------------------
    // Tab 2 — Devices (List)
    // ------------------------------------------------------------------
    let device_items: [&str; 12] = [
        "  PCI 00:00.0  | Intel          | Host Bridge          | Class 06:00",
        "  PCI 00:01.0  | Intel          | ISA Bridge           | Class 06:01",
        "  PCI 00:01.1  | Intel/PIIX     | IDE Controller       | Class 01:01",
        "  PCI 00:02.0  | Bochs/VBox     | VGA Compatible       | Class 03:00",
        "  PCI 00:03.0  | Intel/PIIX     | Ethernet Controller  | Class 02:00",
        "  PCI 00:04.0  | VMware/VBox    | System Peripheral    | Class 08:80",
        "  PCI 00:05.0  | Intel          | Audio Controller     | Class 04:01",
        "  PCI 00:06.0  | Intel          | USB Controller       | Class 0C:03",
        "  PCI 00:07.0  | Intel          | PCI Bridge           | Class 06:04",
        "  PCI 00:08.0  | RedHat/QEMU    | Virtio SCSI          | Class 01:00",
        "  PCI 00:09.0  | RedHat/QEMU    | Virtio Network       | Class 02:00",
        "  PCI 00:1F.0  | Intel          | LPC Controller       | Class 06:01",
    ];
    let devices_list = List::new(1, 0, 80, 23, &device_items);

    // ------------------------------------------------------------------
    // Tab 3 — System (Gauge × 6 + ProgressBar × 3)
    // ------------------------------------------------------------------
    let sys_header     = Label::new(1,  0, 80, " System Health Overview",   Color::White,    Color::Magenta);
    let sys_bar_header = Label::new(10, 0, 80, " Standalone Progress Bars", Color::Black,    Color::Brown);
    let cpu_gauge      = Gauge::new(3, 2, 76, "CPU Usage:       ", 18, 23,  Color::LightCyan, Color::LightGreen);
    let heap_gauge2    = Gauge::new(4, 2, 76, "Heap Memory:     ", 18, 62,  Color::LightCyan, Color::Yellow);
    let pages_gauge    = Gauge::new(5, 2, 76, "Physical Pages:  ", 18, 44,  Color::LightCyan, Color::LightBlue);
    let tasks_gauge    = Gauge::new(6, 2, 76, "Tasks Running:   ", 18, 37,  Color::LightCyan, Color::Pink);
    let ata_gauge      = Gauge::new(7, 2, 76, "ATA Driver:      ", 18, 100, Color::LightCyan, Color::LightGreen);
    let fat_gauge      = Gauge::new(8, 2, 76, "FAT12 Filesystem:", 18, 72,  Color::LightCyan, Color::LightCyan);
    let bar1 = ProgressBar::new(12, 4, 72, 56, Color::White, Color::Black, Color::LightGreen);
    let bar2 = ProgressBar::new(13, 4, 72, 87, Color::White, Color::Black, Color::Yellow);
    let bar3 = ProgressBar::new(14, 4, 72, 12, Color::White, Color::Black, Color::LightRed);

    // ------------------------------------------------------------------
    // Tab 4 — PCI Tree (static demo data)
    // ------------------------------------------------------------------
    let tree_nodes = vec![
        TreeNode::new("Storage Controllers", vec![
            TreeNode::leaf("00:01.1 | Intel/PIIX  | IDE Controller"),
            TreeNode::leaf("00:08.0 | RedHat/QEMU | Virtio SCSI"),
        ], true),
        TreeNode::new("Network Controllers", vec![
            TreeNode::leaf("00:03.0 | Intel/PIIX  | Ethernet Controller"),
            TreeNode::leaf("00:09.0 | RedHat/QEMU | Virtio Network"),
        ], true),
        TreeNode::new("Display Controllers", vec![
            TreeNode::leaf("00:02.0 | Bochs/VBox  | VGA Compatible"),
        ], true),
        TreeNode::new("Bridge Devices", vec![
            TreeNode::leaf("00:00.0 | Intel       | Host Bridge"),
            TreeNode::leaf("00:01.0 | Intel       | ISA Bridge"),
            TreeNode::leaf("00:07.0 | Intel       | PCI Bridge"),
            TreeNode::leaf("00:1F.0 | Intel       | LPC Controller"),
        ], false),
        TreeNode::new("Other Devices", vec![
            TreeNode::leaf("00:04.0 | VMware/VBox | System Peripheral"),
            TreeNode::leaf("00:05.0 | Intel       | Audio Controller"),
            TreeNode::leaf("00:06.0 | Intel       | USB Controller"),
        ], false),
    ];
    let tree_view = TreeView::new(1, 0, 80, 23, tree_nodes);

    // ------------------------------------------------------------------
    // Tab bar (row 0, full width)
    // ------------------------------------------------------------------
    let mut tabs = Tabs::new(0, 0, 80);
    tabs.add_tab("Info");
    tabs.add_tab("Memory");
    tabs.add_tab("Devices");
    tabs.add_tab("System");
    tabs.add_tab("PCI");

    // Assemble and run the application.
    let mut app = TuiApp::new(
        tabs, info_box,
        mem_header, heap_gauge, pmm_gauge, mem_table,
        devices_list,
        sys_header, sys_bar_header,
        cpu_gauge, heap_gauge2, pages_gauge, tasks_gauge, ata_gauge, fat_gauge,
        bar1, bar2, bar3,
        tree_view,
    );
    app.run();

    // Step 2: Restore terminal defaults (re-enable hardware cursor and VGA blink mode), clear screen.
    console::set_vga_mode(0b11).ok();
    console::clear_screen().ok();
}
