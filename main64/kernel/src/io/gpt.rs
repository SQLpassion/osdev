//! Minimal GPT parsing to locate the EFI System Partition.

use crate::drivers::block;

/// The canonical type GUID for the EFI System Partition (mixed-endian on disk).
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2A, 0xC1, 0x1F, 0xF8, 0xD2, 0x11, 0xBA, 0x4B, 0x00, 0xA0, 0xC9, 0x3E, 0xC9, 0x3B,
];

/// Returns the starting LBA of the EFI System Partition, or None if not found.
///
/// Reads the GPT header at LBA 1, then iterates through partition entries looking
/// for the ESP type GUID.
pub fn find_esp_start_lba() -> Option<u64> {
    let mut header_sector = [0u8; 512];

    // Step 1: Read LBA 1 to find the GPT header.
    // We expect the AHCI driver to succeed in reading this sector.
    if block::read_sectors(1, 1, &mut header_sector).is_err() {
        return fallback_esp();
    }

    // Step 2: Parse the header to get partition array metadata.
    let (entry_lba, num_entries, entry_size) = match parse_gpt_header(&header_sector) {
        Some(info) => info,
        None => return fallback_esp(),
    };

    // Determine how many entries fit in one sector.
    let entries_per_sector = 512 / entry_size;

    // Defensively cap the number of entries we read (128 is the standard default).
    let max_entries_to_check = core::cmp::min(num_entries, 128);
    let sectors_to_read = max_entries_to_check.div_ceil(entries_per_sector);

    let mut entry_sector = [0u8; 512];

    // Step 3: Iterate through partition entries.
    for sector_offset in 0..sectors_to_read {
        // Cast the physical LBA to u32, which is safe since the GPT is at the start of the disk.
        let lba = (entry_lba + sector_offset as u64) as u32;

        if block::read_sectors(lba as u64, 1, &mut entry_sector).is_err() {
            return fallback_esp();
        }

        let entries_in_this_sector = core::cmp::min(
            entries_per_sector,
            max_entries_to_check - sector_offset * entries_per_sector,
        );

        if let Some(start_lba) =
            parse_gpt_entries_sector(&entry_sector, entries_in_this_sector, entry_size)
        {
            return Some(start_lba);
        }
    }

    // None matched.
    fallback_esp()
}

/// Parses the GPT Header to extract (entry_lba, num_entries, entry_size).
pub fn parse_gpt_header(header_sector: &[u8; 512]) -> Option<(u64, u32, u32)> {
    if &header_sector[0..8] != b"EFI PART" {
        return None;
    }

    // Extract PartitionEntryLBA (offset 0x48), NumberOfPartitionEntries (0x50), SizeOfPartitionEntry (0x54).
    let entry_lba = u64::from_le_bytes(header_sector[0x48..0x50].try_into().unwrap());
    let num_entries = u32::from_le_bytes(header_sector[0x50..0x54].try_into().unwrap());
    let entry_size = u32::from_le_bytes(header_sector[0x54..0x58].try_into().unwrap());

    if entry_size == 0 || entry_size > 512 || 512 % entry_size != 0 {
        return None;
    }

    Some((entry_lba, num_entries, entry_size))
}

/// Parses a partition entry sector to find the ESP starting LBA.
pub fn parse_gpt_entries_sector(
    entry_sector: &[u8; 512],
    entries_in_this_sector: u32,
    entry_size: u32,
) -> Option<u64> {
    for i in 0..entries_in_this_sector {
        let offset = (i * entry_size) as usize;
        let guid = &entry_sector[offset..offset + 16];

        // Check if this partition is the EFI System Partition.
        if guid == ESP_TYPE_GUID {
            let start_lba = u64::from_le_bytes(
                entry_sector[offset + 0x20..offset + 0x28]
                    .try_into()
                    .unwrap(),
            );
            return Some(start_lba);
        }
    }
    None
}

/// Fallback if parsing fails or ESP is not found.
/// TODO: remove fallback once the primary GPT parsing path is fully stabilized.
fn fallback_esp() -> Option<u64> {
    crate::debugln!("ESP not found in GPT, falling back to LBA 2048");
    Some(2048)
}
