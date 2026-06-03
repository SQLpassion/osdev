//! Kernel TUI engine
//!
//! Provides a minimal text-user-interface toolkit that runs directly on the
//! VGA text buffer.  All widgets write at absolute (row, col) positions via
//! the `Screen::draw_at` / `Screen::fill_rect` / `Screen::draw_box`
//! primitives — the normal `print_str` / scroll path is never used inside the
//! TUI, so the screen content is fully controlled.
//!
//! # Widget inventory
//!
//! | Widget        | File            | Purpose                               |
//! |---------------|-----------------|---------------------------------------|
//! | `Label`       | `label.rs`      | Single-line colored text              |
//! | `List`        | `list.rs`       | Scrollable / selectable item list     |
//! | `ProgressBar` | `progressbar.rs`| Horizontal fill bar with pct readout  |
//! | `Gauge`       | `gauge.rs`      | Labeled progress gauge                |
//! | `TextBox`     | `textbox.rs`    | Multi-line text in a box border       |
//! | `Table`       | `table.rs`      | Scrollable data table                 |
//! | `Tabs`        | `tabs.rs`       | Horizontal tab selection bar          |
//!
//! # Demo
//!
//! `tui::run_demo()` constructs a four-tab `TuiApp` that exercises every
//! widget type and is launched from the REPL with the `tui` command.
//!
//! # Navigation
//!
//! | Key        | Action                                    |
//! |------------|-------------------------------------------|
//! | ← / →      | Switch tabs                               |
//! | ↑ / ↓      | Navigate items in the focused widget      |
//! | q / Escape | Exit the TUI and return to the REPL       |

pub mod app;
pub mod gauge;
pub mod label;
pub mod list;
pub mod progressbar;
pub mod table;
pub mod tabs;
pub mod textbox;

pub use app::TuiApp;
pub use gauge::Gauge;
pub use label::Label;
pub use list::List;
pub use progressbar::ProgressBar;
pub use table::Table;
pub use tabs::Tabs;
pub use textbox::TextBox;

/// VGA screen dimensions shared by all widgets.
pub const SCREEN_ROWS: usize = 25;
pub const SCREEN_COLS: usize = 80;

/// Launch the built-in four-tab TUI demonstration.
///
/// This is the single entry point called by the REPL `tui` command.
/// It constructs the full `TuiApp` on the stack, enters the event loop, and
/// restores a clean REPL screen state after the user exits.
pub fn run_demo() {
    use crate::drivers::screen::{Color, with_screen};

    // Step 1: blank the screen and hide the hardware cursor so the TUI
    //         starts on a clean slate without REPL text or cursor flicker.
    with_screen(|screen| {
        screen.clear();
        screen.disable_blink_mode(); // allow all 16 colors as VGA backgrounds
        screen.disable_hw_cursor();  // fully suppress the blinking hardware cursor
    });

    // Flush any stale keyboard input so key presses from before launching the
    // TUI do not trigger unintended actions inside the application.
    crate::drivers::keyboard::clear_buffers();

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
        "   Ring-3 Syscall Interface       - GetCursor / SetCursor demo",
        "   TUI Engine                     - this screen!",
        "",
        " Navigate with arrow keys. Press q or Esc to return to REPL.",
    ];

    let info_box = TextBox::new(
        1,
        0,
        80,
        23,
        info_lines,
        Color::LightGray,
        Color::Black,
        Color::LightCyan,
    );

    // ------------------------------------------------------------------
    // Tab 1 — Memory (Gauge × 2 + Table)
    // ------------------------------------------------------------------
    let mem_header = Label::new(
        1,
        0,
        80,
        " Memory Subsystem",
        Color::Black,
        Color::LightGreen,
    );
    let heap_gauge = Gauge::new(
        3,
        2,
        76,
        "Heap Memory:     ",
        18,
        62,
        Color::LightCyan,
        Color::LightGreen,
    );
    let pmm_gauge = Gauge::new(
        4,
        2,
        76,
        "Physical Pages:  ",
        18,
        44,
        Color::LightCyan,
        Color::Yellow,
    );

    // Column widths chosen so that total inner width equals 78:
    //   Σ col_widths + 3 * col_count - 1 = 78
    //   22 + 20 + 28  + 3*3 - 1 = 70+8 = 78 ✓
    let mut mem_table = Table::new(
        6,
        0,
        80,
        18,
        &["Region", "Address", "Size / Type"],
        &[22, 20, 28],
    );
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
    let sys_header = Label::new(
        1,
        0,
        80,
        " System Health Overview",
        Color::White,
        Color::Magenta,
    );
    let sys_bar_header = Label::new(
        10,
        0,
        80,
        " Standalone Progress Bars",
        Color::Black,
        Color::Brown,
    );

    let cpu_gauge = Gauge::new(
        3,
        2,
        76,
        "CPU Usage:       ",
        18,
        23,
        Color::LightCyan,
        Color::LightGreen,
    );
    let heap_gauge2 = Gauge::new(
        4,
        2,
        76,
        "Heap Memory:     ",
        18,
        62,
        Color::LightCyan,
        Color::Yellow,
    );
    let pages_gauge = Gauge::new(
        5,
        2,
        76,
        "Physical Pages:  ",
        18,
        44,
        Color::LightCyan,
        Color::LightBlue,
    );
    let tasks_gauge = Gauge::new(
        6,
        2,
        76,
        "Tasks Running:   ",
        18,
        37,
        Color::LightCyan,
        Color::Pink,
    );
    let ata_gauge = Gauge::new(
        7,
        2,
        76,
        "ATA Driver:      ",
        18,
        100,
        Color::LightCyan,
        Color::LightGreen,
    );
    let fat_gauge = Gauge::new(
        8,
        2,
        76,
        "FAT12 Filesystem:",
        18,
        72,
        Color::LightCyan,
        Color::LightCyan,
    );

    let bar1 = ProgressBar::new(12, 4, 72, 56, Color::White, Color::Black, Color::LightGreen);
    let bar2 = ProgressBar::new(13, 4, 72, 87, Color::White, Color::Black, Color::Yellow);
    let bar3 = ProgressBar::new(14, 4, 72, 12, Color::White, Color::Black, Color::LightRed);

    // ------------------------------------------------------------------
    // Tab bar (row 0, full width)
    // ------------------------------------------------------------------
    let mut tabs = Tabs::new(0, 0, 80);
    tabs.add_tab("Info");
    tabs.add_tab("Memory");
    tabs.add_tab("Devices");
    tabs.add_tab("System");

    // Step 2: assemble and run the application.
    let mut app = TuiApp::new(
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
    );

    app.run();

    // Step 3: restore a clean REPL state after the TUI exits.
    with_screen(|screen| {
        screen.enable_blink_mode();  // restore default 8-color background + blink mode
        screen.enable_hw_cursor();   // show cursor again for REPL input
        screen.clear();
        screen.set_color(Color::White);
    });

    // Clear all keyboard buffers so the key used to exit the TUI (e.g. 'q' or Esc)
    // and any keys typed during the session do not bleed into the REPL.
    crate::drivers::keyboard::clear_buffers();
}
