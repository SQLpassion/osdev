use crate::vga::{inb, inw, outb, outl};

const STATUS_BSY: u8 = 0x80;
const STATUS_DRQ: u8 = 0x08;

/// Waits until the BSY flag is cleared.
unsafe fn wait_for_bsy() {
    // SAFETY:
    // - Port 0x1F7 is the status register of the primary IDE channel.
    while (unsafe { inb(0x1F7) } & STATUS_BSY) != 0 {}
}

/// Waits until the DRQ flag is set.
unsafe fn wait_for_drq() {
    // SAFETY:
    // - Port 0x1F7 is the status register of the primary IDE channel.
    while (unsafe { inb(0x1F7) } & STATUS_DRQ) == 0 {}
}

/// Reads a given number of disk sectors (512 bytes) from the starting LBA address into the target memory address.
///
/// # Safety
/// The caller must ensure that `target_address` is a valid pointer pointing to a buffer
/// large enough to hold `sector_count * 512` bytes.
pub unsafe fn read_sectors(mut target_address: *mut u8, lba: u32, sector_count: u8) {
    // Wait for the drive to be ready.
    wait_for_bsy();

    // Send the parameters to the IDE controller.
    outb(0x1F2, sector_count);
    outb(0x1F3, lba as u8);
    outb(0x1F4, (lba >> 8) as u8);
    outb(0x1F5, (lba >> 16) as u8);
    outb(0x1F6, 0xE0 | (((lba >> 24) & 0xF) as u8));
    outb(0x1F7, 0x20); // 0x20 is the READ SECTORS command

    // Read the sectors.
    for _ in 0..sector_count {
        wait_for_bsy();
        wait_for_drq();

        for _ in 0..256 {
            // Retrieve a single 16-bit value from the input port.
            let read_buffer = inw(0x1F0);

            // Write the 2 bytes to consecutive addresses.
            *target_address = (read_buffer & 0xFF) as u8;
            target_address = target_address.add(1);
            *target_address = ((read_buffer >> 8) & 0xFF) as u8;
            target_address = target_address.add(1);
        }
    }
}

/// Writes a given number of disk sectors (512 bytes) to the starting LBA address of the disk from the source memory address.
///
/// # Safety
/// The caller must ensure that `source_address` is a valid pointer pointing to a buffer
/// containing at least `sector_count * 512` bytes.
#[allow(dead_code)]
pub unsafe fn write_sectors(source_address: *const u32, lba: u32, sector_count: u8) {
    // Wait for the drive to be ready.
    wait_for_bsy();

    // Send the parameters to the IDE controller.
    outb(0x1F2, sector_count);
    outb(0x1F3, lba as u8);
    outb(0x1F4, (lba >> 8) as u8);
    outb(0x1F5, (lba >> 16) as u8);
    outb(0x1F6, 0xE0 | (((lba >> 24) & 0xF) as u8));
    outb(0x1F7, 0x30); // 0x30 is the WRITE SECTORS command

    // Write the sectors.
    for _ in 0..sector_count {
        wait_for_bsy();
        wait_for_drq();

        for i in 0..256 {
            outl(0x1F0, *source_address.add(i));
        }
    }
}
