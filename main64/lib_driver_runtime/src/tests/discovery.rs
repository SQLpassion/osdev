use super::select_mmio_bar_index;
use lib_kaos::pci::UserPciBar;

const ZERO_BAR: UserPciBar = UserPciBar {
    bar_type: 0,
    flags: 0,
    address: 0,
    size: 0,
    raw_value: 0,
    _padding: 0,
};

fn memory_bar(address: u64, size: u64) -> UserPciBar {
    UserPciBar {
        bar_type: 2, // Memory32
        flags: 0,
        address,
        size,
        raw_value: 0,
        _padding: 0,
    }
}

fn io_bar(address: u64) -> UserPciBar {
    UserPciBar {
        bar_type: 1, // Io
        flags: 0,
        address,
        size: 0,
        raw_value: 0,
        _padding: 0,
    }
}

#[test]
fn test_select_mmio_bar_index_finds_first_memory_bar() {
    let mut bars = [ZERO_BAR; 6];
    bars[0] = io_bar(0xC000);
    bars[1] = memory_bar(0xFEB0_0000, 256);
    assert_eq!(select_mmio_bar_index(&bars, None), Some(1));
}

#[test]
fn test_select_mmio_bar_index_scan_takes_priority_over_preferred() {
    let mut bars = [ZERO_BAR; 6];
    bars[2] = memory_bar(0xFEB0_0000, 256);
    // preferred_index (0) is not itself a Memory BAR, but a real Memory BAR
    // exists elsewhere -- the scan must win.
    bars[0] = io_bar(0xC000);
    assert_eq!(select_mmio_bar_index(&bars, Some(0)), Some(2));
}

#[test]
fn test_select_mmio_bar_index_falls_back_to_preferred_when_no_memory_bar_found() {
    let mut bars = [ZERO_BAR; 6];
    // A non-zero address but bar_type 0 (None) is not picked up by the
    // scan (which only matches type 2/3), so this must come from the
    // preferred-index fallback.
    bars[3] = UserPciBar {
        address: 0xFEC0_0000,
        size: 4096,
        ..ZERO_BAR
    };
    assert_eq!(select_mmio_bar_index(&bars, Some(3)), Some(3));
}

#[test]
fn test_select_mmio_bar_index_returns_none_when_nothing_usable() {
    let bars = [ZERO_BAR; 6];
    assert_eq!(select_mmio_bar_index(&bars, None), None);
    assert_eq!(select_mmio_bar_index(&bars, Some(0)), None);
}
