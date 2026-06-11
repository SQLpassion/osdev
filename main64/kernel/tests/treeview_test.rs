//! TreeView Integration Test.
//!
//! Validates flattening, selection, expansion, collapse, and clamp logic
//! of the new `TreeView` and `TreeNode` controls.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

extern crate alloc;

use alloc::vec;
use core::panic::PanicInfo;
use kaos_kernel::arch::interrupts;
use kaos_kernel::memory::{heap, pmm, vmm};
use kaos_kernel::tui::{TreeNode, TreeView};

/// Entry point for the integration test kernel
#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    // Initialize serial for test output
    kaos_kernel::drivers::serial::init();

    // Step 1: Initialize IDT/interrupts and page mapping.
    interrupts::init();
    pmm::init(false);
    vmm::init(false);

    // Step 2: Initialize slab heap allocator so alloc::vec works.
    heap::init(false);

    test_main();

    loop {
        core::hint::spin_loop();
    }
}

/// Panic handler for integration tests
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

/// Contract: TreeView nodes and selection toggle expansion correctly.
/// Given: A tree structure with nested nodes.
/// When: We call toggle_selected and selection navigation.
/// Then: Visible nodes list and indices behave correctly.
/// Failure Impact: Regression in TUI TreeView logic or scrolling.
#[test_case]
fn test_treeview_nesting_and_expansion() {
    let child1 = TreeNode::leaf("child1");
    let child2 = TreeNode::leaf("child2");

    // Create root node (expanded) with two collapsed children (one of which has sub-children).
    let sub_child = TreeNode::leaf("sub_child");
    let nested_parent = TreeNode::new("parent", vec![sub_child], false);

    let root = TreeNode::new("root", vec![child1, nested_parent, child2], true);

    let mut tree = TreeView::new(0, 0, 40, 10, vec![root]);

    // Step 1: Verify the initial visible list of nodes.
    // The hierarchy has:
    // - root (depth 0, expanded)
    //   - child1 (depth 1, leaf)
    //   - parent (depth 1, collapsed)
    //   - child2 (depth 1, leaf)
    // "sub_child" should NOT be visible yet.
    assert_eq!(tree.visible_count(), 4, "Initial visible count should be 4");

    let (label, depth, expanded) = tree.get_visible_node_info(0).unwrap();
    assert_eq!(label, "root");
    assert_eq!(depth, 0);
    assert!(expanded);

    let (label, depth, expanded) = tree.get_visible_node_info(2).unwrap();
    assert_eq!(label, "parent");
    assert_eq!(depth, 1);
    assert!(!expanded);

    // Step 2: Navigate selection downwards to "parent" (index 2).
    assert_eq!(tree.selected_idx(), 0);
    tree.select_next();
    assert_eq!(tree.selected_idx(), 1);
    tree.select_next();
    assert_eq!(tree.selected_idx(), 2);

    // Step 3: Toggle expansion on the parent node (index 2).
    // This should reveal "sub_child", increasing visible count to 5.
    tree.toggle_selected();
    assert_eq!(tree.visible_count(), 5, "Visible count should increase to 5 after expansion");

    let (label, depth, expanded) = tree.get_visible_node_info(2).unwrap();
    assert_eq!(label, "parent");
    assert_eq!(depth, 1);
    assert!(expanded);

    let (label, depth, expanded) = tree.get_visible_node_info(3).unwrap();
    assert_eq!(label, "sub_child");
    assert_eq!(depth, 2);
    assert!(!expanded);

    // Step 4: Toggle expansion again to collapse the parent node.
    // Visible count should shrink back to 4.
    tree.toggle_selected();
    assert_eq!(tree.visible_count(), 4, "Visible count should shrink back to 4 after collapse");

    let (label, depth, expanded) = tree.get_visible_node_info(2).unwrap();
    assert_eq!(label, "parent");
    assert_eq!(depth, 1);
    assert!(!expanded);
}

/// Contract: TreeView selection clamping works on collapse.
/// Given: A tree structure with selected nested sub-child.
/// When: We collapse the parent node that contains the selected sub-child.
/// Then: The selection is automatically clamped to the parent node.
/// Failure Impact: Out-of-bounds selection index.
#[test_case]
fn test_treeview_selection_clamping_on_collapse() {
    let sub_child = TreeNode::leaf("sub_child");
    let nested_parent = TreeNode::new("parent", vec![sub_child], true);
    let root = TreeNode::new("root", vec![nested_parent], true);

    let mut tree = TreeView::new(0, 0, 40, 10, vec![root]);

    // Initial structure:
    // 0: root (depth 0, expanded)
    // 1: parent (depth 1, expanded)
    // 2: sub_child (depth 2, leaf)
    assert_eq!(tree.visible_count(), 3);

    // Step 1: Move selection to "sub_child" (index 2).
    tree.select_next();
    tree.select_next();
    assert_eq!(tree.selected_idx(), 2);

    // Step 2: Move selection back to "parent" (index 1) and collapse it.
    tree.select_prev();
    assert_eq!(tree.selected_idx(), 1);
    tree.toggle_selected();

    // Now structure:
    // 0: root
    // 1: parent (collapsed)
    // visible count = 2.
    assert_eq!(tree.visible_count(), 2);
    assert_eq!(tree.selected_idx(), 1);
}
