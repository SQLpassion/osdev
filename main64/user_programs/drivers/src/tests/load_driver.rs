use super::{resolve_driver_filename, ResolveError, DRIVER_TABLE};

#[test]
fn test_resolve_driver_filename_matches_when_device_present() {
    let devices = [(0x10EC, 0x8139)];
    assert_eq!(
        resolve_driver_filename("rtl8139.drv", DRIVER_TABLE, &devices),
        Ok("RTL8139.DRV")
    );
    // Case-insensitive per the FAT32 8.3 filename convention.
    assert_eq!(
        resolve_driver_filename("RTL8139.DRV", DRIVER_TABLE, &devices),
        Ok("RTL8139.DRV")
    );
}

#[test]
fn test_resolve_driver_filename_known_but_device_absent() {
    // "INTLNIC.DRV" is a known name, but the discovered device list only
    // has an unrelated device -- must be distinguished from "unknown driver".
    let devices = [(0x1234, 0x5678)];
    assert_eq!(
        resolve_driver_filename("intlnic.drv", DRIVER_TABLE, &devices),
        Err(ResolveError::DeviceNotPresent)
    );
}

#[test]
fn test_resolve_driver_filename_unknown_name() {
    let devices = [(0x10EC, 0x8139)];
    assert_eq!(
        resolve_driver_filename("does-not-exist.drv", DRIVER_TABLE, &devices),
        Err(ResolveError::UnknownDriver)
    );
}

#[test]
fn test_resolve_driver_filename_unknown_name_even_with_no_devices() {
    assert_eq!(
        resolve_driver_filename("does-not-exist.drv", DRIVER_TABLE, &[]),
        Err(ResolveError::UnknownDriver)
    );
}

#[test]
fn test_resolve_driver_filename_all_intel_ids_resolve_to_intlnic() {
    let intel_ids: [(u16, u16); 4] = [
        (0x8086, 0x10EA),
        (0x8086, 0x15B8),
        (0x8086, 0x10D3),
        (0x8086, 0x100E),
    ];
    for &id in &intel_ids {
        let devices = [id];
        assert_eq!(
            resolve_driver_filename("intlnic.drv", DRIVER_TABLE, &devices),
            Ok("INTLNIC.DRV"),
            "PCI id {:04x}:{:04x} must resolve to INTLNIC.DRV",
            id.0,
            id.1
        );
    }
}

#[test]
fn test_resolve_driver_filename_empty_table_never_matches() {
    assert_eq!(
        resolve_driver_filename("rtl8139.drv", &[], &[(0x10EC, 0x8139)]),
        Err(ResolveError::UnknownDriver)
    );
}
