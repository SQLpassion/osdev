//! TreeView control for displaying nested hierarchies.
//!
//! A `TreeView` displays a tree structure of nodes that can be expanded or
//! collapsed by pressing `<Enter>`. It supports selection and scrolling via the
//! arrow keys, drawing proper borders, indentations, and status indicators.

extern crate alloc;

use alloc::borrow::Cow;
use alloc::vec::Vec;
use crate::drivers::screen::{Color, with_screen};
use crate::tui::{SCREEN_COLS, SCREEN_ROWS};

/// Normal (unselected) item foreground color.
const ITEM_FG: Color = Color::White;

/// Normal (unselected) item background color.
const ITEM_BG: Color = Color::Black;

/// Selected item foreground color (inverted).
const SEL_FG: Color = Color::Black;

/// Selected item background color (inverted).
const SEL_BG: Color = Color::LightCyan;

/// Box border foreground color.
const BORDER_FG: Color = Color::LightCyan;

/// Box border background color.
const BORDER_BG: Color = Color::Black;

/// Stack-allocated fixed-capacity path representation to avoid heap allocations.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct NodePath {
    indices: [usize; 8],
    len: usize,
}

impl NodePath {
    /// Creates a new empty node path.
    pub fn new() -> Self {
        Self {
            indices: [0; 8],
            len: 0,
        }
    }

    /// Pushes a child index onto the path.
    pub fn push(&mut self, idx: usize) {
        if self.len < 8 {
            self.indices[self.len] = idx;
            self.len += 1;
        }
    }

    /// Pops the last child index from the path.
    pub fn pop(&mut self) {
        if self.len > 0 {
            self.len -= 1;
        }
    }

    /// Returns the path as a slice of indices.
    pub fn as_slice(&self) -> &[usize] {
        &self.indices[..self.len]
    }
}

impl Default for NodePath {
    fn default() -> Self {
        Self::new()
    }
}

/// A node within the `TreeView` control.
pub struct TreeNode {
    /// The label text of this node.
    pub label: Cow<'static, str>,
    /// The child nodes of this node.
    pub children: Vec<TreeNode>,
    /// Whether this node is expanded or collapsed.
    pub expanded: bool,
}

impl TreeNode {
    /// Construct a new leaf or parent node.
    pub fn new<S: Into<Cow<'static, str>>>(label: S, children: Vec<TreeNode>, expanded: bool) -> Self {
        Self {
            label: label.into(),
            children,
            expanded,
        }
    }

    /// Helper to create a leaf node with no children.
    pub fn leaf<S: Into<Cow<'static, str>>>(label: S) -> Self {
        Self {
            label: label.into(),
            children: Vec::new(),
            expanded: false,
        }
    }
}

/// Flat representation of a node currently visible in the tree (zero-allocation view).
struct VisibleNodeRef<'a> {
    /// Path of indices from root to this node.
    path: NodePath,
    /// Reference to the text label of this node.
    label: &'a Cow<'static, str>,
    /// Nesting depth (0 for root).
    depth: usize,
    /// Whether the node has children.
    has_children: bool,
    /// Whether the node is currently expanded.
    expanded: bool,
}

/// Flat representation of a node stored in the cache.
struct CachedVisibleNode {
    /// Path of indices from root to this node.
    path: NodePath,
    /// Text label of this node.
    label: Cow<'static, str>,
    /// Nesting depth (0 for root).
    depth: usize,
    /// Whether the node has children.
    has_children: bool,
    /// Whether the node is currently expanded.
    expanded: bool,
}

/// A dynamic treeview control that renders nested hierarchy of nodes.
pub struct TreeView {
    /// Top-left row of the outer box border.
    row: usize,
    /// Top-left column of the outer box border.
    col: usize,
    /// Total outer width (including borders).
    width: usize,
    /// Total outer height (including borders).
    height: usize,
    /// Top-level root nodes of the tree.
    root_nodes: Vec<TreeNode>,
    /// Index of the currently highlighted visible node.
    selected: usize,
    /// First visible item index (scroll offset).
    scroll: usize,
    /// Cached list of visible nodes.
    visible_cache: Vec<CachedVisibleNode>,
}

impl TreeView {
    /// Construct a new `TreeView` control.
    pub fn new(
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        root_nodes: Vec<TreeNode>,
    ) -> Self {
        let mut view = Self {
            row,
            col,
            width,
            height,
            root_nodes,
            selected: 0,
            scroll: 0,
            visible_cache: Vec::new(),
        };
        // Step 1: Initialize the cache.
        view.update_cache();
        view
    }

    /// Number of item rows visible inside the box border.
    fn visible_rows(&self) -> usize {
        self.height.saturating_sub(2)
    }

    /// Rebuilds the cached list of visible nodes from the current tree state.
    fn update_cache(&mut self) {
        let mut visible = Vec::new();
        let mut current_path = NodePath::new();
        
        // Step 1: Run zero-allocation tree traversal to collect visible nodes.
        self.collect_visible_nodes(&self.root_nodes, &mut current_path, 0, &mut visible);

        // Step 2: Clear cache and copy data from temporary list.
        self.visible_cache.clear();
        for node in visible {
            self.visible_cache.push(CachedVisibleNode {
                path: node.path,
                label: node.label.clone(),
                depth: node.depth,
                has_children: node.has_children,
                expanded: node.expanded,
            });
        }
    }

    /// Recursively collect all visible nodes from the current node level.
    fn collect_visible_nodes<'a>(
        &self,
        nodes: &'a [TreeNode],
        current_path: &mut NodePath,
        depth: usize,
        out: &mut Vec<VisibleNodeRef<'a>>,
    ) {
        // Step 1: Iterate over all sibling nodes at this level.
        for (idx, node) in nodes.iter().enumerate() {
            current_path.push(idx);

            out.push(VisibleNodeRef {
                path: *current_path,
                label: &node.label,
                depth,
                has_children: !node.children.is_empty(),
                expanded: node.expanded,
            });

            // Step 2: If the node is expanded and contains children, recursively
            //         traverse down to collect nested sub-nodes.
            if node.expanded && !node.children.is_empty() {
                self.collect_visible_nodes(&node.children, current_path, depth + 1, out);
            }

            current_path.pop();
        }
    }

    /// Return the index of the currently highlighted visible node.
    #[allow(dead_code)]
    pub fn selected_idx(&self) -> usize {
        self.selected
    }

    /// Return the total number of currently visible nodes.
    #[allow(dead_code)]
    pub fn visible_count(&self) -> usize {
        self.visible_cache.len()
    }

    /// Return the label, depth, and expanded state of the visible node at the given index.
    #[allow(dead_code)]
    pub fn get_visible_node_info(&self, index: usize) -> Option<(Cow<'static, str>, usize, bool)> {
        // Step 1: Get the node from cache without traversing the tree.
        let node = self.visible_cache.get(index)?;
        Some((node.label.clone(), node.depth, node.expanded))
    }

    /// Navigate the selection up by one visible item, adjusting scroll.
    pub fn select_prev(&mut self) {
        // Step 1: Shift selection up if possible.
        if self.selected > 0 {
            self.selected -= 1;

            // Step 2: Adjust scroll offset upwards if the selection moves
            //         above the currently visible viewport boundary.
            if self.selected < self.scroll {
                self.scroll = self.selected;
            }
        }
    }

    /// Navigate the selection down by one visible item, adjusting scroll.
    pub fn select_next(&mut self) {
        // Step 1: Check bounds against the cached visible list.
        if self.selected + 1 < self.visible_cache.len() {
            self.selected += 1;

            // Step 2: Adjust scroll offset downwards if the selection moves
            //         below the currently visible viewport boundary.
            let last_visible = self.scroll + self.visible_rows().saturating_sub(1);
            if self.selected > last_visible {
                self.scroll = self.selected - self.visible_rows().saturating_sub(1);
            }
        }
    }

    /// Toggle the expansion state of the currently highlighted node.
    pub fn toggle_selected(&mut self) {
        // Step 1: Locate the highlighted node in the cache.
        if let Some(selected_node) = self.visible_cache.get(self.selected) {
            // Step 2: If the node has children, find it mutably in the tree and flip state.
            if selected_node.has_children {
                if let Some(node) = find_node_mut(&mut self.root_nodes, selected_node.path.as_slice()) {
                    node.expanded = !node.expanded;
                    // Step 3: Rebuild the visible node cache to reflect structure change.
                    self.update_cache();
                }
            }
        }

        // Step 4: Keep indices valid as the number of visible items might have changed.
        self.clamp_indices();
    }

    /// Ensure selection and scroll offsets remain in valid bounds after changes.
    fn clamp_indices(&mut self) {
        let len = self.visible_cache.len();

        // Step 1: Clamp selected index to the maximum available visible elements.
        if self.selected >= len {
            self.selected = len.saturating_sub(1);
        }

        // Step 2: Readjust scroll window to ensure selection remains in view.
        let visible_rows = self.visible_rows();
        if self.scroll >= len {
            self.scroll = len.saturating_sub(visible_rows);
        }

        if self.selected < self.scroll {
            self.scroll = self.selected;
        }

        let last_visible = self.scroll + visible_rows.saturating_sub(1);
        if self.selected > last_visible {
            self.scroll = self.selected - visible_rows.saturating_sub(1);
        }
    }

    /// Render the TreeView widget (borders, visible nodes, scroll stats).
    pub fn draw(&self) {
        if self.row >= SCREEN_ROWS || self.col >= SCREEN_COLS {
            return;
        }

        // Step 1: Read visible nodes from the cache.
        let visible_nodes = &self.visible_cache;

        with_screen(|screen| {
            // Step 2: Draw the outer border frame.
            screen.draw_box(self.row, self.col, self.width, self.height, BORDER_FG, BORDER_BG);

            // Step 3: Fill the interior background to erase previous state.
            let inner_col = self.col + 1;
            let inner_width = self.width.saturating_sub(2);
            let visible_row_count = self.visible_rows();

            screen.fill_rect(
                self.row + 1,
                inner_col,
                inner_width,
                visible_row_count,
                b' ',
                ITEM_FG,
                ITEM_BG,
            );

            // Step 4: Draw each visible node in the viewable viewport window.
            for vis_idx in 0..visible_row_count {
                let abs_idx = self.scroll + vis_idx;
                if abs_idx >= visible_nodes.len() {
                    break;
                }

                let item_row = self.row + 1 + vis_idx;
                let node = &visible_nodes[abs_idx];
                let is_selected = abs_idx == self.selected;

                let (fg, bg) = if is_selected {
                    (SEL_FG, SEL_BG)
                } else {
                    (ITEM_FG, ITEM_BG)
                };

                // Fill current row background so highlighting spans the full width.
                screen.fill_rect(item_row, inner_col, inner_width, 1, b' ', fg, bg);

                // Calculate horizontal indentation offset based on hierarchy depth.
                let indent_cols = node.depth * 2;
                let start_col = inner_col + indent_cols;

                if start_col < inner_col + inner_width {
                    // Render expand/collapse symbol for parent nodes, or spacer for leaves.
                    let symbol = if node.has_children {
                        if node.expanded {
                            "[-] "
                        } else {
                            "[+] "
                        }
                    } else {
                        "    "
                    };

                    screen.draw_at(item_row, start_col, symbol, fg, bg);

                    // Render the node label.
                    let label_col = start_col + 4;
                    if label_col < inner_col + inner_width {
                        screen.draw_at(item_row, label_col, &node.label, fg, bg);
                    }
                }
            }

            // Step 5: Render a minimal "X/Y" scroll status indicator on the bottom border.
            if !visible_nodes.is_empty() {
                let bottom_row = self.row + self.height - 1;
                let indicator_col = self.col + self.width.saturating_sub(8);

                let cur = self.selected + 1;
                let total = visible_nodes.len();

                // Format " CUR/TOT " manually to avoid dynamic allocation.
                let mut buf = [b' '; 7];
                let mut pos = 6usize;

                let mut n = total;
                loop {
                    pos -= 1;
                    buf[pos] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 || pos == 0 {
                        break;
                    }
                }

                if pos > 0 {
                    pos -= 1;
                    buf[pos] = b'/';
                }

                let mut n = cur;
                loop {
                    if pos == 0 {
                        break;
                    }
                    pos -= 1;
                    buf[pos] = b'0' + (n % 10) as u8;
                    n /= 10;
                    if n == 0 {
                        break;
                    }
                }

                for (i, &byte) in buf.iter().enumerate() {
                    screen.draw_char_at(
                        bottom_row,
                        indicator_col + i,
                        byte,
                        BORDER_FG,
                        BORDER_BG,
                    );
                }
            }
        });
    }
}

/// Helper function to traverse the tree nodes mutably and find a node by its path.
fn find_node_mut<'a>(root_nodes: &'a mut [TreeNode], path: &[usize]) -> Option<&'a mut TreeNode> {
    if path.is_empty() {
        return None;
    }

    let mut current = root_nodes.get_mut(path[0])?;
    for &idx in &path[1..] {
        current = current.children.get_mut(idx)?;
    }

    Some(current)
}
