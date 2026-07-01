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
    screen_cols, screen_rows, with_screen, Color, Gauge, Label, ProgressBar, Table, Tabs, TextBox,
    TreeNode, TreeView,
};

#[path = "../../../kernel/src/drivers/pci/database.rs"]
mod pci_database;

/// Top-level TUI application — owns every widget across all four tabs.
pub struct TuiApp {
    /// Horizontal tab bar at the top of the screen (row 0).
    tabs: Tabs,
    /// Tab 0: Multi-line description box detailing system specs.
    info_box: TextBox,
    /// Tab 1: Section title label for the memory tab.
    mem_header: Label,
    /// Tab 1: Table displaying physical & virtual memory region maps.
    mem_table: Table,
    /// Tab 2: Section title label for system metrics.
    sys_header: Label,
    /// Tab 3: Section title label for standalone progress bars.
    sys_bar_header: Label,
    /// Tab 3: Gauge showing simulated CPU usage metrics.
    cpu_gauge: Gauge,
    /// Tab 3: Gauge showing duplicate heap statistics.
    heap_gauge2: Gauge,
    /// Tab 3: Gauge showing physical memory allocation.
    pages_gauge: Gauge,
    /// Tab 3: Gauge showing the count of running scheduler tasks.
    tasks_gauge: Gauge,
    /// Tab 3: Gauge showing ATA PIO read speed metrics.
    ata_gauge: Gauge,
    /// Tab 3: Gauge showing FAT32 disk usage capacity.
    fat_gauge: Gauge,
    /// Tab 3: Progress bar instance 1.
    bar1: ProgressBar,
    /// Tab 3: Progress bar instance 2.
    bar2: ProgressBar,
    /// Tab 3: Progress bar instance 3.
    bar3: ProgressBar,
    /// Tab 4: Nested node tree illustrating device organization layout.
    tree_view: TreeView,
}

impl TuiApp {
    /// Instantiates a new TuiApp container with all required widgets.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        tabs: Tabs,
        info_box: TextBox,
        mem_header: Label,
        mem_table: Table,
        sys_header: Label,
        sys_bar_header: Label,
        cpu_gauge: Gauge,
        heap_gauge2: Gauge,
        pages_gauge: Gauge,
        tasks_gauge: Gauge,
        ata_gauge: Gauge,
        fat_gauge: Gauge,
        bar1: ProgressBar,
        bar2: ProgressBar,
        bar3: ProgressBar,
        tree_view: TreeView,
    ) -> Self {
        Self {
            tabs,
            info_box,
            mem_header,
            mem_table,
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

    /// Enter the main event loop.
    pub fn run(&mut self) {
        // Step 1: Draw the initial frame prior to waiting for keys.
        self.draw_frame();

        let mut last_second = 99;

        // Step 2: Loop indefinitely, checking keys and clock.
        loop {
            // Check if a key is pressed (non-blocking).
            let key = console::poll_key().unwrap_or(Key::Unknown);

            // If no key is pressed, check if the second has changed to update the status bar clock.
            if key == Key::Unknown {
                let mut udt = lib_kaos::time::UserDateTime {
                    year: 0,
                    month: 0,
                    day: 0,
                    hour: 0,
                    minute: 0,
                    second: 0,
                    _padding: [0; 7],
                };
                if lib_kaos::time::get_time(&mut udt).is_ok() && udt.second != last_second {
                    last_second = udt.second;
                    self.draw_frame();
                }

                // Yield CPU to prevent a 100% busy loop.
                lib_kaos::process::yield_now();
                continue;
            }

            match key {
                // Left/Right: Navigate between tabs.
                Key::ArrowLeft => {
                    self.tabs.select_prev();
                    self.clear_content_area();
                }
                Key::ArrowRight => {
                    self.tabs.select_next();
                    self.clear_content_area();
                }
                // Up/Down: Scroll current tab list/table widgets.
                Key::ArrowUp => self.navigate_up(),
                Key::ArrowDown => self.navigate_down(),
                Key::Enter => {
                    if self.tabs.active() == 1 {
                        self.show_mem_detail_dialog();
                    } else if self.tabs.active() == 3 {
                        if let Some((label, has_children)) = self.tree_view.selected_node_info() {
                            if has_children {
                                self.tree_view.toggle_selected();
                            } else {
                                self.show_pci_detail_dialog(&label);
                            }
                        }
                    }
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
            0 => {
                self.info_box.draw();
            }
            1 => {
                self.mem_header.draw();
                self.mem_table.draw();
            }
            2 => {
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
            3 => {
                self.tree_view.draw();
            }
            _ => {}
        }
    }

    /// Helper: Overwrites the central screen space with black spaces before switching tabs.
    fn clear_content_area(&self) {
        with_screen(|screen| {
            let cols = screen.cols();
            let rows = screen.rows();
            screen.fill_rect(1, 0, cols, rows - 2, b' ', Color::White, Color::Black);
        });
    }

    /// Helper: Dispatches scroll up requests.
    fn navigate_up(&mut self) {
        match self.tabs.active() {
            1 => self.mem_table.select_prev(),
            3 => self.tree_view.select_prev(),
            _ => {}
        }
    }

    /// Helper: Dispatches scroll down requests.
    fn navigate_down(&mut self) {
        match self.tabs.active() {
            1 => self.mem_table.select_next(),
            3 => self.tree_view.select_next(),
            _ => {}
        }
    }

    /// Helper: Draws the bottom footer showing help keybindings and active page status.
    fn draw_status_bar(&self) {
        with_screen(|screen| {
            let cols = screen.cols();
            let rows = screen.rows();
            // Fill background of row rows - 1 with LightGray.
            screen.fill_rect(rows - 1, 0, cols, 1, b' ', Color::Black, Color::LightGray);
            // Print left help binding text.
            screen.draw_at(
                rows - 1,
                1,
                "Left/Right: tab    Up/Down: scroll    q / Esc: quit",
                Color::Black,
                Color::LightGray,
            );

            // Print date and time if available
            let mut udt = lib_kaos::time::UserDateTime {
                year: 0,
                month: 0,
                day: 0,
                hour: 0,
                minute: 0,
                second: 0,
                _padding: [0; 7],
            };
            if lib_kaos::time::get_time(&mut udt).is_ok() {
                // Format: "2026-06-08 19:30:13" (19 chars)
                let mut time_buf = [0u8; 19];

                // Format Year
                let y = udt.year;
                time_buf[0] = b'0' + ((y / 1000) % 10) as u8;
                time_buf[1] = b'0' + ((y / 100) % 10) as u8;
                time_buf[2] = b'0' + ((y / 10) % 10) as u8;
                time_buf[3] = b'0' + (y % 10) as u8;
                time_buf[4] = b'-';

                time_buf[5] = b'0' + (udt.month / 10);
                time_buf[6] = b'0' + (udt.month % 10);
                time_buf[7] = b'-';

                time_buf[8] = b'0' + (udt.day / 10);
                time_buf[9] = b'0' + (udt.day % 10);
                time_buf[10] = b' ';

                time_buf[11] = b'0' + (udt.hour / 10);
                time_buf[12] = b'0' + (udt.hour % 10);
                time_buf[13] = b':';

                time_buf[14] = b'0' + (udt.minute / 10);
                time_buf[15] = b'0' + (udt.minute % 10);
                time_buf[16] = b':';

                time_buf[17] = b'0' + (udt.second / 10);
                time_buf[18] = b'0' + (udt.second % 10);

                let start_time = cols - 21;
                for (i, &byte) in time_buf.iter().enumerate() {
                    screen.draw_char_at(
                        rows - 1,
                        start_time + i,
                        byte,
                        Color::Black,
                        Color::LightGray,
                    );
                }
            }
        });
    }

    /// Parses the BDF from a leaf node's label and displays a modal dialog with PCI details.
    fn show_pci_detail_dialog(&self, label: &str) {
        if label.len() < 7 || &label[2..3] != ":" || &label[5..6] != "." {
            return;
        }

        let bus = match u8::from_str_radix(&label[0..2], 16) {
            Ok(b) => b,
            Err(_) => return,
        };
        let device = match u8::from_str_radix(&label[3..5], 16) {
            Ok(d) => d,
            Err(_) => return,
        };
        let function = match label[6..7].parse::<u8>() {
            Ok(f) => f,
            Err(_) => return,
        };

        if let Ok(count) = lib_kaos::pci::get_pci_device_count() {
            for idx in 0..count {
                let mut dev = lib_kaos::pci::UserPciDevice {
                    bus: 0,
                    device: 0,
                    function: 0,
                    class_code: 0,
                    subclass: 0,
                    prog_if: 0,
                    revision_id: 0,
                    header_type: 0,
                    vendor_id: 0,
                    device_id: 0,
                    interrupt_line: 0,
                    interrupt_pin: 0,
                    _padding: [0; 2],
                    bars: [lib_kaos::pci::UserPciBar {
                        bar_type: 0,
                        flags: 0,
                        address: 0,
                        size: 0,
                        raw_value: 0,
                        _padding: 0,
                    }; 6],
                };

                if lib_kaos::pci::get_pci_device(idx, &mut dev).is_ok()
                    && dev.bus == bus
                    && dev.device == device
                    && dev.function == function
                {
                    use alloc::format;
                    use alloc::string::String;
                    use alloc::vec;

                    let vendor_name = pci_database::vendor_to_str(dev.vendor_id);
                    let device_name = pci_database::device_to_str(dev.vendor_id, dev.device_id);
                    let class_name = pci_database::class_to_str(dev.class_code, dev.subclass);

                    let mut lines = vec![
                        format!(
                            "BDF Address:  {:02x}:{:02x}.{}",
                            dev.bus, dev.device, dev.function
                        ),
                        format!("Vendor Name:  {}", vendor_name),
                        format!("Device Name:  {}", device_name),
                        format!(
                            "IDs:          Vendor {:04x}, Device {:04x}",
                            dev.vendor_id, dev.device_id
                        ),
                        format!(
                            "Class Code:   {} ({:02x}:{:02x})",
                            class_name, dev.class_code, dev.subclass
                        ),
                        format!("Revision ID:  {:#04x}", dev.revision_id),
                        format!("Prog IF:      {:#04x}", dev.prog_if),
                        format!(
                            "Interrupts:   Line {}, Pin {}",
                            dev.interrupt_line, dev.interrupt_pin
                        ),
                        String::from("Base Address Registers:"),
                    ];

                    let mut has_bars = false;
                    for (i, bar) in dev.bars.iter().enumerate() {
                        if bar.bar_type != 0 {
                            has_bars = true;
                            let type_str = match bar.bar_type {
                                1 => "I/O Port",
                                2 => "Mem32",
                                3 => "Mem64",
                                _ => "Unknown",
                            };
                            let pref_str = if bar.flags == 1 {
                                "prefetchable"
                            } else {
                                "non-pref"
                            };
                            lines.push(format!(
                                "  BAR {}: {} {:#x} (size {}, {})",
                                i, type_str, bar.address, bar.size, pref_str
                            ));
                        }
                    }

                    if !has_bars {
                        lines.push(String::from("  No active BARs configured."));
                    }

                    let dialog = lib_tui::Dialog::new(2, 5, 70, 20, "PCI Device Details", lines);
                    dialog.draw();
                    with_screen(|screen| screen.flush());

                    loop {
                        let k = console::read_key().unwrap_or(Key::Unknown);
                        match k {
                            Key::Enter | Key::Escape | Key::Char(b'q') | Key::Char(b'Q') => break,
                            _ => {}
                        }
                    }
                    break;
                }
            }
        }
    }

    /// Displays a modal dialog with details for the selected memory region.
    fn show_mem_detail_dialog(&self) {
        let selected_idx = self.mem_table.selected();

        // Query the BIOS memory map using our new system call
        if let Ok(count) = lib_kaos::bios::get_bios_memory_map_entry_count() {
            if selected_idx < count {
                let mut region = lib_kaos::bios::UserBiosMemoryRegion {
                    start: 0,
                    size: 0,
                    region_type: 0,
                    _padding: 0,
                };
                if lib_kaos::bios::get_bios_memory_map_entry(selected_idx, &mut region).is_ok() {
                    use alloc::format;
                    use alloc::vec;

                    let type_str = match region.region_type {
                        1 => "Usable (available RAM for OS)",
                        2 => "Reserved (reserved by BIOS/hardware)",
                        3 => "ACPI Reclaimable (tables/data)",
                        4 => "ACPI NVS (non-volatile storage)",
                        _ => "Unknown (unclassified or reserved)",
                    };

                    let end_addr = region.start.wrapping_add(region.size).wrapping_sub(1);
                    let page_size = 4096u64;
                    let pages = region.size / page_size;
                    #[allow(clippy::manual_is_multiple_of)]
                    let is_aligned =
                        (region.start % page_size == 0) && (region.size % page_size == 0);

                    let lines = vec![
                        format!("Index:             {}", selected_idx),
                        format!("Start Address:     {:#018x}", region.start),
                        format!("End Address:       {:#018x}", end_addr),
                        format!("Size in Bytes:     {} Bytes", region.size),
                        format!("Size in KB:        {} KB", region.size / 1024),
                        format!("Size in MB:        {} MB", region.size / 1024 / 1024),
                        format!("Physical Pages:    {} (4 KiB frames)", pages),
                        format!(
                            "Page-Aligned:      {}",
                            if is_aligned { "Yes" } else { "No" }
                        ),
                        format!(
                            "Region Type:       {} (Raw: {})",
                            type_str, region.region_type
                        ),
                    ];

                    let dialog =
                        lib_tui::Dialog::new(4, 10, 60, 16, "Memory Region Details", lines);
                    dialog.draw();
                    with_screen(|screen| screen.flush());

                    loop {
                        let k = console::read_key().unwrap_or(Key::Unknown);
                        match k {
                            Key::Enter | Key::Escape | Key::Char(b'q') | Key::Char(b'Q') => break,
                            _ => {}
                        }
                    }
                }
            }
        }
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

    let cols = screen_cols();
    let rows = screen_rows();

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
        "   FAT32 File System              - read-only, root dir listing",
        "   PCI Bus Scanner                - class/vendor lookup",
        "   Ring-3 Syscall Interface       - int 0x80, 23 syscalls",
        "   TUI Engine                     - this screen! (Ring-3)",
        "",
        " Navigate with arrow keys. Press q or Esc to return to shell.",
    ];
    let info_box = TextBox::new(
        1,
        0,
        cols,
        rows - 2,
        info_lines,
        Color::LightGray,
        Color::Black,
        Color::LightCyan,
    );

    // ------------------------------------------------------------------
    // Tab 1 — Memory (Table)
    // ------------------------------------------------------------------
    let mem_header = Label::new(
        1,
        0,
        cols,
        " Memory Subsystem",
        Color::Black,
        Color::LightGreen,
    );

    let mut mem_table = Table::new(
        3,
        0,
        cols,
        rows - 4,
        &["Region", "Address", "Size / Type"],
        &[22, 20, cols.saturating_sub(44)],
    );
    let leak_str =
        |s: alloc::string::String| -> &'static str { alloc::boxed::Box::leak(s.into_boxed_str()) };

    use alloc::format;

    if let Ok(count) = lib_kaos::bios::get_bios_memory_map_entry_count() {
        for idx in 0..count {
            let mut region = lib_kaos::bios::UserBiosMemoryRegion {
                start: 0,
                size: 0,
                region_type: 0,
                _padding: 0,
            };
            if lib_kaos::bios::get_bios_memory_map_entry(idx, &mut region).is_ok() {
                let name = leak_str(format!("BIOS Region #{}", idx));
                let addr_str = leak_str(format!("{:#010x}", region.start));
                let type_str = match region.region_type {
                    1 => "Usable",
                    2 => "Reserved",
                    3 => "ACPI Reclaim",
                    4 => "ACPI NVS",
                    _ => "Unknown",
                };
                let size_mb = region.size / (1024 * 1024);
                let size_kb = region.size / 1024;
                let size_str = if size_mb > 0 {
                    leak_str(format!("{} MB | {}", size_mb, type_str))
                } else {
                    leak_str(format!("{} KB | {}", size_kb, type_str))
                };
                mem_table.add_row(&[name, addr_str, size_str]);
            }
        }
    } else {
        mem_table.add_row(&["Low Memory", "0x00000000", "640 KB | Usable"]);
        mem_table.add_row(&["BIOS ROM Area", "0x000A0000", "384 KB | Reserved"]);
        mem_table.add_row(&["VGA Buffer", "0x000B8000", "32 KB | MMIO"]);
        mem_table.add_row(&["Kernel Image", "0x00100000", "~1 MB | Kernel"]);
        mem_table.add_row(&["Extended Memory", "0x00200000", "~126 MB | Usable"]);
        mem_table.add_row(&["APIC MMIO", "0xFEE00000", "1 MB | MMIO"]);
        mem_table.add_row(&["ACPI Tables", "0x7FC00000", "4 MB | ACPI Reclaim"]);
        mem_table.add_row(&[
            "Higher-Half Mapping",
            "0xFFFF800000000000",
            "Kernel VA | Virtual",
        ]);
    }

    // ------------------------------------------------------------------
    // Tab 3 — System (Gauge × 6 + ProgressBar × 3)
    // ------------------------------------------------------------------
    let sys_header = Label::new(
        1,
        0,
        cols,
        " System Health Overview",
        Color::White,
        Color::Magenta,
    );
    let sys_bar_header = Label::new(
        10,
        0,
        cols,
        " Standalone Progress Bars",
        Color::Black,
        Color::Brown,
    );
    let cpu_gauge = Gauge::new(
        3,
        2,
        cols - 4,
        "CPU Usage:       ",
        18,
        23,
        Color::LightCyan,
        Color::LightGreen,
    );
    let heap_gauge2 = Gauge::new(
        4,
        2,
        cols - 4,
        "Heap Memory:     ",
        18,
        62,
        Color::LightCyan,
        Color::Yellow,
    );
    let pages_gauge = Gauge::new(
        5,
        2,
        cols - 4,
        "Physical Pages:  ",
        18,
        44,
        Color::LightCyan,
        Color::LightBlue,
    );
    let tasks_gauge = Gauge::new(
        6,
        2,
        cols - 4,
        "Tasks Running:   ",
        18,
        37,
        Color::LightCyan,
        Color::Pink,
    );
    let ata_gauge = Gauge::new(
        7,
        2,
        cols - 4,
        "ATA Driver:      ",
        18,
        100,
        Color::LightCyan,
        Color::LightGreen,
    );
    let fat_gauge = Gauge::new(
        8,
        2,
        cols - 4,
        "FAT32 Filesystem:",
        18,
        72,
        Color::LightCyan,
        Color::LightCyan,
    );
    let bar1 = ProgressBar::new(
        12,
        4,
        cols - 8,
        56,
        Color::White,
        Color::Black,
        Color::LightGreen,
    );
    let bar2 = ProgressBar::new(
        13,
        4,
        cols - 8,
        87,
        Color::White,
        Color::Black,
        Color::Yellow,
    );
    let bar3 = ProgressBar::new(
        14,
        4,
        cols - 8,
        12,
        Color::White,
        Color::Black,
        Color::LightRed,
    );

    // ------------------------------------------------------------------
    // Tab 4 — PCI Tree (dynamic hardware data)
    // ------------------------------------------------------------------

    let mut storage_devices = vec![];
    let mut network_devices = vec![];
    let mut display_devices = vec![];
    let mut bridge_devices = vec![];
    let mut other_devices = vec![];

    if let Ok(count) = lib_kaos::pci::get_pci_device_count() {
        for idx in 0..count {
            let mut dev = lib_kaos::pci::UserPciDevice {
                bus: 0,
                device: 0,
                function: 0,
                class_code: 0,
                subclass: 0,
                prog_if: 0,
                revision_id: 0,
                header_type: 0,
                vendor_id: 0,
                device_id: 0,
                interrupt_line: 0,
                interrupt_pin: 0,
                _padding: [0; 2],
                bars: [lib_kaos::pci::UserPciBar {
                    bar_type: 0,
                    flags: 0,
                    address: 0,
                    size: 0,
                    raw_value: 0,
                    _padding: 0,
                }; 6],
            };

            if lib_kaos::pci::get_pci_device(idx, &mut dev).is_ok() {
                let vendor_name = pci_database::vendor_to_str(dev.vendor_id);
                let device_name = pci_database::device_to_str(dev.vendor_id, dev.device_id);

                // Format a compact descriptive string to fit on the 80-column screen: "00:01.1 | Intel | PIIX IDE"
                let dev_desc = format!(
                    "{:02x}:{:02x}.{} | {} | {}",
                    dev.bus, dev.device, dev.function, vendor_name, device_name
                );

                let leaf = TreeNode::leaf(dev_desc);

                match dev.class_code {
                    0x01 => storage_devices.push(leaf),
                    0x02 => network_devices.push(leaf),
                    0x03 => display_devices.push(leaf),
                    0x06 => bridge_devices.push(leaf),
                    _ => other_devices.push(leaf),
                }
            }
        }
    }

    let tree_nodes = vec![
        TreeNode::new("Storage Controllers", storage_devices, true),
        TreeNode::new("Network Controllers", network_devices, true),
        TreeNode::new("Display Controllers", display_devices, true),
        TreeNode::new("Bridge Devices", bridge_devices, false),
        TreeNode::new("Other Devices", other_devices, false),
    ];
    let tree_view = TreeView::new(1, 0, cols, rows - 2, tree_nodes);

    // ------------------------------------------------------------------
    // Tab bar (row 0, full width)
    // ------------------------------------------------------------------
    let mut tabs = Tabs::new(0, 0, cols);
    tabs.add_tab("Info");
    tabs.add_tab("Memory");
    tabs.add_tab("System");
    tabs.add_tab("PCI Devices");

    // Assemble and run the application.
    let mut app = TuiApp::new(
        tabs,
        info_box,
        mem_header,
        mem_table,
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
    );
    app.run();

    // Step 2: Restore terminal defaults (re-enable hardware cursor and VGA blink mode), clear screen.
    console::set_vga_mode(0b11).ok();
    console::clear_screen().ok();
}
