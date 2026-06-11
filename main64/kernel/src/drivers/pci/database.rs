//! Helper database mappings to resolve IDs to readable strings.

/// Helper to map a Class and Subclass code to a human-readable name.
pub fn class_to_str(class: u8, subclass: u8) -> &'static str {
    match class {
        0x00 => "Unclassified Device",
        0x01 => match subclass {
            0x00 => "SCSI Storage Controller",
            0x01 => "IDE Interface",
            0x02 => "Floppy Disk Controller",
            0x03 => "IPI Bus Controller",
            0x04 => "RAID Controller",
            0x05 => "ATA Controller",
            0x06 => "SATA Controller",
            0x07 => "Serial Attached SCSI (SAS) Controller",
            0x08 => "Non-Volatile Memory (NVM/NVMe) Controller",
            _ => "Mass Storage Controller",
        },
        0x02 => match subclass {
            0x00 => "Ethernet Controller",
            0x01 => "Token Ring Controller",
            0x02 => "FDDI Controller",
            0x03 => "ATM Controller",
            0x04 => "ISDN Controller",
            0x05 => "WorldFip Controller",
            0x06 => "PICMG Multi-computing",
            _ => "Network Controller",
        },
        0x03 => match subclass {
            0x00 => "VGA Compatible Controller",
            0x01 => "XGA Controller",
            0x02 => "3D Controller",
            _ => "Display Controller",
        },
        0x04 => "Multimedia Device",
        0x05 => "Memory Controller",
        0x06 => match subclass {
            0x00 => "Host Bridge",
            0x01 => "ISA Bridge",
            0x02 => "EISA Bridge",
            0x03 => "MCA Bridge",
            0x04 => "PCI-to-PCI Bridge",
            0x05 => "PCMCIA Bridge",
            0x06 => "NuBus Bridge",
            0x07 => "CardBus Bridge",
            0x08 => "Semi-Transparent PCI-to-PCI Bridge",
            _ => "Bridge Device",
        },
        0x07 => "Simple Communication Controller",
        0x08 => "Base System Peripheral",
        0x09 => "Input Device Controller",
        0x0A => "Docking Station",
        0x0B => "Processor",
        0x0C => match subclass {
            0x00 => "FireWire (IEEE 1394) Controller",
            0x01 => "ACCESS.bus Controller",
            0x02 => "SSA Controller",
            0x03 => "USB Controller",
            0x04 => "Fibre Channel Controller",
            0x05 => "System Management Bus (SMBus)",
            _ => "Serial Bus Controller",
        },
        0x0D => "Wireless Controller",
        0x0E => "Intelligent Controller",
        0x0F => "Satellite Communications Controller",
        0x10 => "Encryption Controller",
        0x11 => "Signal Processing Controller",
        _ => "Unknown Class",
    }
}

/// Helper to map a Vendor ID to a human-readable name.
pub fn vendor_to_str(vendor_id: u16) -> &'static str {
    match vendor_id {
        0x8086 => "Intel Corporation",
        0x10EC => "Realtek Semiconductor Co., Ltd.",
        0x10DE => "NVIDIA Corporation",
        0x1002 => "Advanced Micro Devices, Inc. [AMD/ATI]",
        0x1234 => "QEMU/Bochs",
        0x15AD => "VMware",
        0x80EE => "Oracle Corporation (VirtualBox)",
        0x1AF4 => "Red Hat, Inc. (Virtio)",
        0x1014 => "IBM",
        0x1022 => "AMD",
        0x1106 => "VIA Technologies, Inc.",
        0x106b => "Apple Inc.",
        _ => "Unknown Vendor",
    }
}

/// Helper to map a specific Device ID (associated with a Vendor ID) to a human-readable name.
pub fn device_to_str(vendor_id: u16, device_id: u16) -> &'static str {
    match vendor_id {
        0x8086 => match device_id {
            0x1237 => "430FX - 82437FX System Controller [Triton I]",
            0x7000 => "82371SB PIIX3 ISA [Triton II]",
            0x7010 => "82371SB PIIX3 IDE [Triton II]",
            0x7113 => "82371AB/EB/MB PIIX4 ACPI",
            0x100E => "82540EM Gigabit Ethernet Controller",
            0x2922 => "82801IR/ICH9R SATA Controller [AHCI mode]",
            0x2829 => "82801HM/HEM (ICH8M/ICH8M-E) SATA Controller [AHCI mode]",
            _ => "Generic Intel Device",
        },
        0x1234 => match device_id {
            0x1111 => "Bochs/QEMU VGA Card",
            _ => "Generic QEMU/Bochs Device",
        },
        0x1AF4 => match device_id {
            0x1000 => "Virtio Network Card",
            0x1001 => "Virtio Block Device",
            0x1002 => "Virtio Balloon",
            0x1003 => "Virtio Console",
            _ => "Generic Virtio Device",
        },
        _ => "Generic PCI Device",
    }
}
