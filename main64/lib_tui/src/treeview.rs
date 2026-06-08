//! TreeView widget — nested tree hierarchy with expand/collapse.
//!
//! Visualizes hierarchical nodes. Supports expanding/collapsing sub-nodes,
//! tracks selection indices across the flattened visible representation,
//! and displays expansion indicators (`[+]`, `[-]`) with appropriate indentation.

extern crate alloc;
use alloc::borrow::Cow;
use alloc::vec::Vec;
use crate::screen::{Color, with_screen};
use crate::{SCREEN_COLS, SCREEN_ROWS};

/// Default foreground color of items.
const ITEM_FG:   Color = Color::White;
/// Default background color of items.
const ITEM_BG:   Color = Color::Black;
/// High-contrast selected item text color.
const SEL_FG:    Color = Color::Black;
/// High-contrast selected item background highlight color.
const SEL_BG:    Color = Color::LightCyan;
/// Outer border line foreground color.
const BORDER_FG: Color = Color::LightCyan;
/// Outer border line background color.
const BORDER_BG: Color = Color::Black;

/// Represents a single node in the hierarchy.
pub struct TreeNode {
    /// String description label of the node.
    pub label: Cow<'static, str>,
    /// Nested child nodes.
    pub children: Vec<TreeNode>,
    /// Expansion state flag (expanded = true, collapsed = false).
    pub expanded: bool,
}

impl TreeNode {
    /// Creates a new parent node.
    pub fn new<S: Into<Cow<'static, str>>>(label: S, children: Vec<TreeNode>, expanded: bool) -> Self {
        Self { label: label.into(), children, expanded }
    }

    /// Creates a leaf node (has no child sub-elements).
    pub fn leaf<S: Into<Cow<'static, str>>>(label: S) -> Self {
        Self { label: label.into(), children: Vec::new(), expanded: false }
    }
}

/// A scrollable hierarchy browser widget.
pub struct TreeView {
    /// Zero-based vertical screen row index where the top border starts.
    row: usize,
    /// Zero-based horizontal screen column index where the left border starts.
    col: usize,
    /// Total width of the tree box (including borders) in columns.
    width: usize,
    /// Total height of the tree box (including borders) in rows.
    height: usize,
    /// List of top-level root nodes.
    root_nodes: Vec<TreeNode>,
    /// Index of the currently highlighted visible node.
    selected: usize,
    /// Index of the first visible node in the scroll window viewport.
    scroll: usize,
}

/// Flat representation of a node currently visible in the tree.
struct VisibleNodeRef {
    /// Vector path of indices from root to this node (e.g. `[0, 2]` represents 1st root's 3rd child).
    path: Vec<usize>,
    /// Text label of the node.
    label: Cow<'static, str>,
    /// Depth level of nesting (used to calculate rendering indentation).
    depth: usize,
    /// Flag indicating whether the node contains any children.
    has_children: bool,
    /// Current expansion state of the node.
    expanded: bool,
}

impl TreeView {
    /// Creates a new TreeView widget.
    ///
    /// # Arguments
    /// * `row` - Starting vertical coordinate.
    /// * `col` - Starting horizontal coordinate.
    /// * `width` - Total width.
    /// * `height` - Total height.
    /// * `root_nodes` - Vector of root nodes.
    pub fn new(row: usize, col: usize, width: usize, height: usize, root_nodes: Vec<TreeNode>) -> Self {
        Self { row, col, width, height, root_nodes, selected: 0, scroll: 0 }
    }

    /// Computes the number of visible rows inside the box.
    fn visible_rows(&self) -> usize { self.height.saturating_sub(2) }

    /// Traverses the hierarchy to gather all visible nodes (respecting collapsed branches).
    fn get_visible_nodes(&self) -> Vec<VisibleNodeRef> {
        let mut visible = Vec::new();
        let mut current_path = Vec::new();
        self.collect_visible_nodes(&self.root_nodes, &mut current_path, 0, &mut visible);
        visible
    }

    /// Recursively collects visible nodes into the output list.
    fn collect_visible_nodes(&self, nodes: &[TreeNode], current_path: &mut Vec<usize>, depth: usize, out: &mut Vec<VisibleNodeRef>) {
        for (idx, node) in nodes.iter().enumerate() {
            current_path.push(idx);
            out.push(VisibleNodeRef {
                path: current_path.clone(),
                label: node.label.clone(),
                depth,
                has_children: !node.children.is_empty(),
                expanded: node.expanded,
            });
            // Recursively collect children only if the parent is expanded.
            if node.expanded && !node.children.is_empty() {
                self.collect_visible_nodes(&node.children, current_path, depth + 1, out);
            }
            current_path.pop();
        }
    }

    /// Returns the active selected index.
    #[allow(dead_code)] pub fn selected_idx(&self) -> usize { self.selected }
    /// Returns the total number of flattened visible nodes.
    #[allow(dead_code)] pub fn visible_count(&self) -> usize { self.get_visible_nodes().len() }

    /// Shifts selection up, adjusting the scroll window top if necessary.
    pub fn select_prev(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
            if self.selected < self.scroll { self.scroll = self.selected; }
        }
    }

    /// Shifts selection down, adjusting the scroll window bottom if necessary.
    pub fn select_next(&mut self) {
        let visible = self.get_visible_nodes();
        if self.selected + 1 < visible.len() {
            self.selected += 1;
            let last_visible = self.scroll + self.visible_rows().saturating_sub(1);
            if self.selected > last_visible {
                self.scroll = self.selected - self.visible_rows().saturating_sub(1);
            }
        }
    }

    /// Toggles the expansion state of the currently selected node.
    pub fn toggle_selected(&mut self) {
        let visible = self.get_visible_nodes();
        if let Some(selected_node) = visible.get(self.selected) {
            if selected_node.has_children {
                if let Some(node) = find_node_mut(&mut self.root_nodes, &selected_node.path) {
                    node.expanded = !node.expanded;
                }
            }
        }
        self.clamp_indices();
    }

    /// Returns the label and child-status of the currently selected node.
    pub fn selected_node_info(&self) -> Option<(alloc::borrow::Cow<'static, str>, bool)> {
        let visible = self.get_visible_nodes();
        let node = visible.get(self.selected)?;
        Some((node.label.clone(), node.has_children))
    }

    /// Ensures that index/scroll indices stay in bounds after nodes expand/collapse.
    fn clamp_indices(&mut self) {
        let visible = self.get_visible_nodes();
        let len = visible.len();
        if self.selected >= len { self.selected = len.saturating_sub(1); }
        let visible_rows = self.visible_rows();
        if self.scroll >= len { self.scroll = len.saturating_sub(visible_rows); }
        if self.selected < self.scroll { self.scroll = self.selected; }
        let last_visible = self.scroll + visible_rows.saturating_sub(1);
        if self.selected > last_visible { self.scroll = self.selected - visible_rows.saturating_sub(1); }
    }

    /// Renders the tree frame, node labels with indentation, and indicators to the screen.
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS { return; }
        let visible_nodes = self.get_visible_nodes();
        with_screen(|screen| {
            // Draw the outer CP437 box frame.
            screen.draw_box(self.row, self.col, self.width, self.height, BORDER_FG, BORDER_BG);

            let inner_col   = self.col + 1;
            let inner_width = self.width.saturating_sub(2);
            let visible_row_count = self.visible_rows();

            // Clear the interior area.
            screen.fill_rect(self.row + 1, inner_col, inner_width, visible_row_count, b' ', ITEM_FG, ITEM_BG);

            // Render visible nodes within the scrolling window.
            for vis_idx in 0..visible_row_count {
                let abs_idx = self.scroll + vis_idx;
                if abs_idx >= visible_nodes.len() { break; }

                let item_row = self.row + 1 + vis_idx;
                let node = &visible_nodes[abs_idx];
                let is_selected = abs_idx == self.selected;
                let (fg, bg) = if is_selected { (SEL_FG, SEL_BG) } else { (ITEM_FG, ITEM_BG) };

                // Draw item highlight row.
                screen.fill_rect(item_row, inner_col, inner_width, 1, b' ', fg, bg);

                // Compute rendering start position (each depth level shifts text right by 2 columns).
                let indent_cols = node.depth * 2;
                let start_col   = inner_col + indent_cols;

                if start_col < inner_col + inner_width {
                    // Draw expand/collapse state symbol or indentation padding.
                    let symbol = if node.has_children { if node.expanded { "[-] " } else { "[+] " } } else { "    " };
                    screen.draw_at(item_row, start_col, symbol, fg, bg);

                    let label_col = start_col + 4;
                    if label_col < inner_col + inner_width {
                        // Draw node label with hard clipping to widget boundaries.
                        let max_len = (inner_col + inner_width).saturating_sub(label_col);
                        let label_str = &node.label[..node.label.len().min(max_len)];
                        screen.draw_at(item_row, label_col, label_str, fg, bg);
                    }
                }
            }

            // Draw scroll fraction indicator at the bottom-right border frame.
            if !visible_nodes.is_empty() {
                let bottom_row    = self.row + self.height - 1;
                let indicator_col = self.col + self.width.saturating_sub(8);
                let cur   = self.selected + 1;
                let total = visible_nodes.len();

                let mut buf = [b' '; 7];
                let mut pos = 6usize;
                let mut n = total;

                // Parse total count digits.
                loop {
                    pos -= 1; buf[pos] = b'0' + (n % 10) as u8; n /= 10;
                    if n == 0 || pos == 0 { break; }
                }
                if pos > 0 { pos -= 1; buf[pos] = b'/'; }

                // Parse current selection index digits.
                let mut n = cur;
                loop {
                    if pos == 0 { break; }
                    pos -= 1; buf[pos] = b'0' + (n % 10) as u8; n /= 10;
                    if n == 0 { break; }
                }

                // Render buffer characters.
                for (i, &byte) in buf.iter().enumerate() {
                    screen.draw_char_at(bottom_row, indicator_col + i, byte, BORDER_FG, BORDER_BG);
                }
            }
        });
    }
}

/// Helper: Traverses child branches to locate a mutable node by index path.
fn find_node_mut<'a>(root_nodes: &'a mut [TreeNode], path: &[usize]) -> Option<&'a mut TreeNode> {
    if path.is_empty() { return None; }
    let mut current = root_nodes.get_mut(path[0])?;
    for &idx in &path[1..] { current = current.children.get_mut(idx)?; }
    Some(current)
}
