//! Kernel-side syscall dispatcher (`int 0x80` path).
//!
//! Responsibilities of this module:
//! - decode syscall number + ABI arguments,
//! - route to the corresponding kernel implementation,
//! - enforce minimal argument validation at syscall boundaries,
//! - return stable numeric result/error codes to caller context.
//!
//! ABI for `dispatch` (provided by interrupt entry glue):
//! - `RAX` -> `syscall_nr`
//! - `RDI` -> `arg0`
//! - `RSI` -> `arg1`
//! - `RDX` -> `arg2`
//! - `R10` -> `arg3`

use core::sync::atomic::{AtomicBool, Ordering};

use crate::logging;
use crate::syscall::{syscall_result_to_raw, SyscallError, SyscallId, SyscallResult};

pub mod bios;
pub mod console;
pub mod driver;
pub mod fs;
pub mod pci;
pub mod process;

/// Global switch for per-syscall trace logging (`[SYSCALL] ...` lines).
static SYSCALL_TRACE_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable/disable syscall trace logging.
#[cfg_attr(not(test), allow(dead_code))]
pub fn set_syscall_trace_enabled(enabled: bool) {
    SYSCALL_TRACE_ENABLED.store(enabled, Ordering::Relaxed);
}

/// Returns whether syscall trace logging is currently enabled.
pub fn syscall_trace_enabled() -> bool {
    SYSCALL_TRACE_ENABLED.load(Ordering::Relaxed)
}

/// Returns the stable human-readable syscall name for a raw syscall number.
///
/// Used by dispatcher logging so serial traces remain understandable without
/// requiring external number-to-name lookup tables.
pub const fn syscall_name_for_number(syscall_nr: u64) -> &'static str {
    match syscall_nr {
        SyscallId::YIELD => "Yield",
        SyscallId::WRITE_SERIAL => "WriteSerial",
        SyscallId::EXIT => "Exit",
        SyscallId::WRITE_CONSOLE => "WriteConsole",
        SyscallId::GET_CHAR => "GetChar",
        SyscallId::GET_CURSOR => "GetCursor",
        SyscallId::SET_CURSOR => "SetCursor",
        SyscallId::CLEAR_SCREEN => "ClearScreen",
        SyscallId::OPEN_FILE => "OpenFile",
        SyscallId::CLOSE_FILE => "CloseFile",
        SyscallId::READ_FILE => "ReadFile",
        SyscallId::WRITE_FILE => "WriteFile",
        SyscallId::DELETE_FILE => "DeleteFile",
        SyscallId::SEEK_FILE => "SeekFile",
        SyscallId::END_OF_FILE => "EndOfFile",
        SyscallId::PRINT_ROOT_DIRECTORY => "PrintRootDirectory",
        SyscallId::MMAP => "Mmap",
        SyscallId::EXEC => "Exec",
        SyscallId::WAIT => "Wait",
        SyscallId::SHUTDOWN => "Shutdown",
        SyscallId::WRITE_FRAMEBUFFER => "WriteFramebuffer",
        SyscallId::READ_KEY => "ReadKey",
        SyscallId::SET_VGA_MODE => "SetVgaMode",
        SyscallId::GET_PCI_DEVICE_COUNT => "GetPciDeviceCount",
        SyscallId::GET_PCI_DEVICE => "GetPciDevice",
        SyscallId::GET_BIOS_MEMORY_MAP_ENTRY_COUNT => "GetBiosMemoryMapEntryCount",
        SyscallId::GET_BIOS_MEMORY_MAP_ENTRY => "GetBiosMemoryMapEntry",
        SyscallId::GET_TIME => "GetTime",
        SyscallId::POLL_KEY => "PollKey",
        SyscallId::GET_CONSOLE_DIMENSIONS => "GetConsoleDimensions",
        SyscallId::MAP_PHYSICAL => "MapPhysical",
        SyscallId::UNMAP_PHYSICAL => "UnmapPhysical",
        SyscallId::SPAWN_DRIVER => "SpawnDriver",
        SyscallId::ALLOC_DMA => "AllocDma",
        SyscallId::FREE_DMA => "FreeDma",
        SyscallId::VIRT_TO_PHYS => "VirtToPhys",
        SyscallId::DRV_REGISTER => "DrvRegister",
        SyscallId::DRV_LOOKUP => "DrvLookup",
        SyscallId::NET_SEND => "NetSend",
        SyscallId::NET_RECV => "NetRecv",
        SyscallId::DRV_PUBLISH_STATUS => "DrvPublishStatus",
        SyscallId::DRV_QUERY => "DrvQuery",
        SyscallId::DRV_UNLOAD => "DrvUnload",
        SyscallId::DRV_LIST_COUNT => "DrvListCount",
        SyscallId::DRV_LIST_ENTRY => "DrvListEntry",
        _ => "Unknown",
    }
}

/// Resolves syscall number and dispatches to the corresponding kernel handler.
///
/// ABI contract (as set by `int 0x80` entry glue):
/// - `syscall_nr`: `RAX`
/// - `arg0..arg3`: `RDI`, `RSI`, `RDX`, `R10`
///
/// Returns kernel-internal typed results.
///
/// Raw ABI conversion to sentinel `u64` values is done at the syscall boundary.
pub fn dispatch_checked(
    syscall_nr: u64,
    arg0: u64,
    arg1: u64,
    arg2: u64,
    arg3: u64,
) -> SyscallResult<u64> {
    // Step 1: resolve the handler and compute the syscall return value.
    let result = match syscall_nr {
        SyscallId::YIELD => process::syscall_yield_impl(),
        SyscallId::WRITE_SERIAL => {
            console::syscall_write_serial_impl(arg0 as *const u8, arg1 as usize)
        }
        SyscallId::WRITE_CONSOLE => {
            console::syscall_write_console_impl(arg0 as *const u8, arg1 as usize)
        }
        SyscallId::GET_CHAR => process::syscall_getchar_impl(),
        SyscallId::GET_CURSOR => console::syscall_get_cursor_impl(),
        SyscallId::SET_CURSOR => console::syscall_set_cursor_impl(arg0 as usize, arg1 as usize),
        SyscallId::CLEAR_SCREEN => console::syscall_clear_screen_impl(),
        SyscallId::EXIT => process::syscall_exit_impl(),
        SyscallId::OPEN_FILE => fs::syscall_open_file_impl(arg0 as *const u8, arg1),
        SyscallId::CLOSE_FILE => fs::syscall_close_file_impl(arg0),
        SyscallId::READ_FILE => fs::syscall_read_file_impl(arg0, arg1 as *mut u8, arg2),
        SyscallId::WRITE_FILE => fs::syscall_write_file_impl(arg0, arg1 as *const u8, arg2),
        SyscallId::DELETE_FILE => fs::syscall_delete_file_impl(arg0 as *const u8),
        SyscallId::SEEK_FILE => fs::syscall_seek_file_impl(arg0, arg1),
        SyscallId::END_OF_FILE => fs::syscall_end_of_file_impl(arg0),
        SyscallId::PRINT_ROOT_DIRECTORY => fs::syscall_print_root_directory_impl(),
        SyscallId::MMAP => process::syscall_mmap_impl(arg0, arg1 as usize),
        SyscallId::EXEC => process::syscall_exec_impl(arg0 as *const u8, arg1),
        SyscallId::WAIT => process::syscall_wait_impl(arg0),
        SyscallId::SHUTDOWN => process::syscall_shutdown_impl(),
        SyscallId::WRITE_FRAMEBUFFER => {
            console::syscall_write_framebuffer_impl(arg0 as *const u16, arg1 as usize)
        }
        SyscallId::READ_KEY => process::syscall_read_key_impl(),
        SyscallId::SET_VGA_MODE => console::syscall_set_vga_mode_impl(arg0),
        SyscallId::GET_PCI_DEVICE_COUNT => pci::syscall_get_pci_device_count_impl(),
        SyscallId::GET_PCI_DEVICE => pci::syscall_get_pci_device_impl(
            arg0,
            arg1 as *mut crate::syscall::types::UserPciDevice,
        ),
        SyscallId::GET_BIOS_MEMORY_MAP_ENTRY_COUNT => {
            bios::syscall_get_bios_memory_map_entry_count_impl()
        }
        SyscallId::GET_BIOS_MEMORY_MAP_ENTRY => bios::syscall_get_bios_memory_map_entry_impl(
            arg0,
            arg1 as *mut crate::syscall::types::UserBiosMemoryRegion,
        ),
        SyscallId::GET_TIME => {
            bios::syscall_get_time_impl(arg0 as *mut crate::syscall::types::UserDateTime)
        }
        SyscallId::POLL_KEY => process::syscall_poll_key_impl(),
        SyscallId::GET_CONSOLE_DIMENSIONS => console::syscall_get_console_dimensions_impl(),
        SyscallId::MAP_PHYSICAL => driver::syscall_map_physical_impl(arg0, arg1 as usize, arg2),
        SyscallId::UNMAP_PHYSICAL => driver::syscall_unmap_physical_impl(arg0, arg1 as usize),
        SyscallId::SPAWN_DRIVER => driver::syscall_spawn_driver_impl(
            arg0 as *const u8,
            arg1,
            arg2 as *const crate::syscall::types::UserDriverGrants,
        ),
        SyscallId::ALLOC_DMA => driver::syscall_alloc_dma_impl(arg0 as usize, arg1 as *mut u64),
        SyscallId::FREE_DMA => driver::syscall_free_dma_impl(arg0, arg1 as usize),
        SyscallId::VIRT_TO_PHYS => driver::syscall_virt_to_phys_impl(arg0),
        SyscallId::DRV_REGISTER => driver::syscall_drv_register_impl(arg0, arg1),
        SyscallId::DRV_LOOKUP => driver::syscall_drv_lookup_impl(arg0, arg1),
        SyscallId::NET_SEND => driver::syscall_net_send_impl(arg0, arg1, arg2),
        SyscallId::NET_RECV => driver::syscall_net_recv_impl(arg0, arg1, arg2, arg3),
        SyscallId::DRV_PUBLISH_STATUS => driver::syscall_drv_publish_status_impl(arg0),
        SyscallId::DRV_QUERY => driver::syscall_drv_query_impl(arg0, arg1),
        SyscallId::DRV_UNLOAD => driver::syscall_drv_unload_impl(arg0, arg1),
        SyscallId::DRV_LIST_COUNT => driver::syscall_drv_list_count_impl(),
        SyscallId::DRV_LIST_ENTRY => {
            driver::syscall_drv_list_entry_impl(arg0, arg1 as *mut crate::syscall::UserDriverInfo)
        }
        _ => Err(SyscallError::Unsupported),
    };

    // Step 2: emit one serial trace line for every syscall dispatch.
    // This gives deterministic kernel-side visibility into syscall traffic.
    let raw_result = syscall_result_to_raw(result);
    let trace_enabled = syscall_trace_enabled();
    logging::logln_with_options(
        "syscall",
        format_args!(
            "[SYSCALL] nr={} name={} arg0={:#x} arg1={:#x} arg2={:#x} arg3={:#x} ret={:#x}",
            syscall_nr,
            syscall_name_for_number(syscall_nr),
            arg0,
            arg1,
            arg2,
            arg3,
            raw_result
        ),
        trace_enabled,
        trace_enabled,
    );

    result
}

/// ABI-compatible raw dispatcher (`Result` encoded to sentinel `u64` values).
#[cfg_attr(not(test), allow(dead_code))]
pub fn dispatch(syscall_nr: u64, arg0: u64, arg1: u64, arg2: u64, arg3: u64) -> u64 {
    syscall_result_to_raw(dispatch_checked(syscall_nr, arg0, arg1, arg2, arg3))
}
