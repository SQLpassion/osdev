//! User-space TUI library for KAOS Ring-3 programs.
//!
//! Provides the same widget set as the kernel's `tui` module but operates
//! entirely in Ring-3: widgets write into a local 80×25 back-buffer, and
//! `Screen::flush` transfers it to VGA via the `WriteFramebuffer` syscall.
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
//! | `TreeView`    | `treeview.rs`   | Nested hierarchy with expand/collapse |

#![no_std]
extern crate alloc;

pub mod dialog;
pub mod gauge;
pub mod label;
pub mod list;
pub mod progressbar;
pub mod screen;
pub mod table;
pub mod tabs;
pub mod textbox;
pub mod treeview;

pub use dialog::Dialog;
pub use gauge::Gauge;
pub use label::Label;
pub use list::List;
pub use progressbar::ProgressBar;
pub use screen::{Color, Screen, with_screen, SCREEN_COLS, SCREEN_ROWS};
pub use table::Table;
pub use tabs::Tabs;
pub use textbox::TextBox;
pub use treeview::{TreeNode, TreeView};
