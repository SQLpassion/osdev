#![no_std]
#![no_main]
#![allow(clippy::empty_loop)]

//! KAOS UEFI Bootloader (`BOOTX64.EFI`)
//!
//! This application is loaded by the UEFI firmware (typically at path `\EFI\BOOT\BOOTX64.EFI`
//! on an EFI System Partition). It initializes the serial debugging port, queries the
//! Graphics Output Protocol (GOP) for visual framebuffer layouts, reads the kernel image
//! `KERNEL.BIN` from the boot filesystem, allocates low memory, exits UEFI boot services,
//! constructs a unified physical memory map, maps the higher-half kernel, and jumps to Ring 0.

use core::ffi::c_void;
use core::panic::PanicInfo;

mod serial;

/// UEFI status code (`EFI_STATUS`, a `UINTN`). `0` represents `EFI_SUCCESS`.
pub type Status = usize;

/// UEFI handle (`EFI_HANDLE`), an opaque pointer representing firmware objects.
pub type Handle = *mut c_void;

/// Header common to all UEFI tables (`EFI_TABLE_HEADER`).
///
/// Contains verification signatures and structure sizes to ensure compatibility.
#[repr(C)]
struct EfiTableHeader {
    /// Opaque signature identifying the table type.
    signature: u64,
    /// Revision level of the table.
    revision: u32,
    /// Size of the header table in bytes.
    header_size: u32,
    /// Calculated CRC32 checksum.
    crc32: u32,
    /// Reserved padding space.
    reserved: u32,
}

/// The UEFI Simple Text Output Protocol (`EFI_SIMPLE_TEXT_OUTPUT_PROTOCOL`).
///
/// Provides basic character console output functions to print to screen.
#[repr(C)]
struct EfiSimpleTextOutputProtocol {
    /// Resets the text output device hardware.
    reset: extern "efiapi" fn(this: *mut EfiSimpleTextOutputProtocol, extended: bool) -> Status,
    /// Prints a NUL-terminated UCS-16 wide string to the output device.
    output_string:
        extern "efiapi" fn(this: *mut EfiSimpleTextOutputProtocol, string: *const u16) -> Status,
}

/// A standard 128-bit Universally Unique Identifier (`GUID`).
///
/// Used to identify protocols, configuration tables, and device paths.
#[repr(C)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Guid {
    pub data1: u32,
    pub data2: u16,
    pub data3: u16,
    pub data4: [u8; 8],
}

/// GUID identifying the Graphics Output Protocol (GOP).
const EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID: Guid = Guid {
    data1: 0x9042a9de,
    data2: 0x23dc,
    data3: 0x4a38,
    data4: [0x96, 0xfb, 0x7a, 0xde, 0xd0, 0x80, 0x51, 0x6a],
};

/// GUID identifying the Simple File System Protocol.
const EFI_SIMPLE_FILE_SYSTEM_PROTOCOL_GUID: Guid = Guid {
    data1: 0x964e5b22,
    data2: 0x6459,
    data3: 0x11d2,
    data4: [0x8e, 0x39, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// GUID identifying the Loaded Image Protocol.
const EFI_LOADED_IMAGE_PROTOCOL_GUID: Guid = Guid {
    data1: 0x5b1b31a1,
    data2: 0x9562,
    data3: 0x11d2,
    data4: [0x8e, 0x3f, 0x00, 0xa0, 0xc9, 0x69, 0x72, 0x3b],
};

/// UEFI Loaded Image Protocol (`EFI_LOADED_IMAGE_PROTOCOL`).
///
/// Contains information about the currently executing bootloader image.
#[repr(C)]
struct EfiLoadedImageProtocol {
    revision: u32,
    /// Parent image handle that loaded this image.
    parent_handle: Handle,
    /// System table pointer associated with this image.
    system_table: *const EfiSystemTable,
    /// The physical device handle on which the image file resides.
    device_handle: Handle,
    file_path: *const c_void,
    reserved: *const c_void,
    load_options_size: u32,
    load_options: *const c_void,
    /// The base memory address where the image is loaded.
    image_base: *const c_void,
    /// Size of the loaded image in bytes.
    image_size: u64,
    image_code_type: u32,
    image_data_type: u32,
    unload: *const c_void,
}

/// UEFI Simple File System Protocol (`EFI_SIMPLE_FILE_SYSTEM_PROTOCOL`).
///
/// Used to gain access to directory volumes on disk drives.
#[repr(C)]
struct EfiSimpleFileSystemProtocol {
    revision: u64,
    /// Opens the root directory volume of the filesystem.
    open_volume: extern "efiapi" fn(
        this: *mut EfiSimpleFileSystemProtocol,
        root: *mut *mut EfiFileProtocol,
    ) -> Status,
}

/// UEFI File Protocol (`EFI_FILE_PROTOCOL`).
///
/// Represents an open file or directory on a FAT partition, supporting read, write, and seek.
#[repr(C)]
struct EfiFileProtocol {
    revision: u64,
    /// Opens a file relative to the current directory handle.
    open: extern "efiapi" fn(
        this: *mut EfiFileProtocol,
        new_handle: *mut *mut EfiFileProtocol,
        file_name: *const u16,
        open_mode: u64,
        attributes: u64,
    ) -> Status,
    /// Closes the active file handle.
    close: extern "efiapi" fn(this: *mut EfiFileProtocol) -> Status,
    delete: *const c_void,
    /// Reads data from the file into the destination buffer.
    read: extern "efiapi" fn(
        this: *mut EfiFileProtocol,
        buffer_size: *mut usize,
        buffer: *mut c_void,
    ) -> Status,
    write: *const c_void,
    /// Gets the current file offset pointer position.
    get_position: extern "efiapi" fn(this: *mut EfiFileProtocol, position: *mut u64) -> Status,
    /// Sets the current file offset pointer position (e.g. `0xFFFFFFFFFFFFFFFF` for EOF).
    set_position: extern "efiapi" fn(this: *mut EfiFileProtocol, position: u64) -> Status,
}

/// UEFI Graphics Output Protocol (`EFI_GRAPHICS_OUTPUT_PROTOCOL`).
///
/// Provides access to the linear pixel graphics framebuffer established by the firmware.
#[repr(C)]
struct EfiGraphicsOutputProtocol {
    query_mode: *const c_void,
    set_mode: *const c_void,
    blt: *const c_void,
    /// Pointer to the active mode structure.
    mode: *mut EfiGraphicsOutputProtocolMode,
}

/// Active mode information structure of the Graphics Output Protocol.
#[repr(C)]
struct EfiGraphicsOutputProtocolMode {
    max_mode: u32,
    mode: u32,
    /// Active mode metadata descriptor.
    info: *mut EfiGraphicsOutputModeInformation,
    size_of_info: usize,
    /// Physical starting address of the linear framebuffer.
    frame_buffer_start: u64,
    /// Total capacity size of the framebuffer in bytes.
    frame_buffer_size: usize,
}

/// Active display resolution and pixel format settings of the GOP mode.
#[repr(C)]
struct EfiGraphicsOutputModeInformation {
    version: u32,
    /// Horizontal resolution in pixels.
    horizontal_resolution: u32,
    /// Vertical resolution in pixels.
    vertical_resolution: u32,
    pixel_format: u32,
    pixel_information: [u32; 4],
    /// Scanline stride length in pixels (includes potential horizontal padding).
    pixels_per_scanline: u32,
}

/// UEFI Memory Descriptor representing a physical memory region.
#[repr(C)]
struct EfiMemoryDescriptor {
    /// Type of memory (e.g., EfiConventionalMemory = 7, EfiLoaderCode = 1).
    memory_type: u32,
    /// Physical starting address.
    physical_start: u64,
    /// Virtual starting address.
    virtual_start: u64,
    /// Number of 4 KiB pages spanned by this range.
    number_of_pages: u64,
    /// Memory cache and access permission attributes.
    attribute: u64,
}

/// UEFI Boot Services Table (`EFI_BOOT_SERVICES`).
///
/// Contains firmware functions available only during the boot loader phase.
#[repr(C)]
struct EfiBootServices {
    hdr: EfiTableHeader,
    raise_tpl: *const c_void,
    restore_tpl: *const c_void,
    /// Allocates physical memory pages.
    allocate_pages: extern "efiapi" fn(
        alloc_type: u32,
        memory_type: u32,
        pages: usize,
        address: *mut u64,
    ) -> Status,
    free_pages: *const c_void,
    /// Retrieves the current physical memory map layout.
    get_memory_map: extern "efiapi" fn(
        memory_map_size: *mut usize,
        memory_map: *mut u8,
        map_key: *mut usize,
        descriptor_size: *mut usize,
        descriptor_version: *mut u32,
    ) -> Status,
    allocate_pool: *const c_void,
    free_pool: *const c_void,
    create_event: *const c_void,
    set_timer: *const c_void,
    wait_for_event: *const c_void,
    signal_event: *const c_void,
    close_event: *const c_void,
    check_event: *const c_void,
    install_protocol_interface: *const c_void,
    reinstall_protocol_interface: *const c_void,
    uninstall_protocol_interface: *const c_void,
    /// Retrieves an interface implementing a specific protocol GUID on a handle.
    handle_protocol: extern "efiapi" fn(
        handle: Handle,
        protocol: *const Guid,
        interface: *mut *mut c_void,
    ) -> Status,
    reserved: *const c_void,
    register_protocol_notify: *const c_void,
    locate_handle: *const c_void,
    locate_device_path: *const c_void,
    install_configuration_table: *const c_void,
    load_image: *const c_void,
    start_image: *const c_void,
    exit: *const c_void,
    unload_image: *const c_void,
    /// Exits boot services, transitioning ownership of hardware from firmware to the kernel.
    exit_boot_services: extern "efiapi" fn(image_handle: Handle, map_key: usize) -> Status,
    get_next_monotonic_count: *const c_void,
    stall: *const c_void,
    /// Arms or disarms the firmware watchdog timer.
    set_watchdog_timer:
        extern "efiapi" fn(timeout: usize, code: u64, data_size: usize, data: *const u16) -> Status,
    connect_controller: *const c_void,
    disconnect_controller: *const c_void,
    open_protocol: *const c_void,
    close_protocol: *const c_void,
    open_protocol_information: *const c_void,
    protocols_per_handle: *const c_void,
    locate_handle_buffer: extern "efiapi" fn(
        search_type: u32,
        protocol: *const Guid,
        search_key: *const c_void,
        no_handles: *mut usize,
        buffer: *mut *mut Handle,
    ) -> Status,
    /// Locates the first handle in the system that supports a protocol GUID.
    locate_protocol: extern "efiapi" fn(
        protocol: *const Guid,
        registration: *const c_void,
        interface: *mut *mut c_void,
    ) -> Status,
}

/// UEFI System Table (`EFI_SYSTEM_TABLE`).
///
/// The master system parameters containing standard console streams and tables.
#[repr(C)]
pub struct EfiSystemTable {
    hdr: EfiTableHeader,
    firmware_vendor: *const u16,
    firmware_revision: u32,
    console_in_handle: Handle,
    con_in: *mut c_void,
    console_out_handle: Handle,
    /// Protocol instance for console output text mode.
    con_out: *mut EfiSimpleTextOutputProtocol,
    standard_error_handle: Handle,
    std_err: *mut c_void,
    runtime_services: *mut c_void,
    /// Pointer to the boot services function table.
    boot_services: *mut EfiBootServices,
}

/// Unified BootInfo video mode type selector.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoModeType {
    /// VGA Text mode (BIOS default).
    VgaText = 0,
    /// Linear Pixel graphics framebuffer (UEFI/VBE default).
    Framebuffer = 1,
}

/// Color channel layout of the pixels in the linear graphics framebuffer.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelFormat {
    Rgb = 0,
    Bgr = 1,
    Bitmask = 2,
    BltOnly = 3,
}

/// Framebuffer details passed down to the kernel.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct FramebufferInfo {
    pub base_address: u64,
    pub size: usize,
    pub width: u32,
    pub height: u32,
    pub pixels_per_scanline: u32,
    pub pixel_format: PixelFormat,
}

/// Simplified memory map entries for the kernel physical memory manager.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct UnifiedMemoryEntry {
    pub start: u64,
    pub size: u64,
    pub is_usable: bool,
}

/// Master BootInfo structure containing all system diagnostic tables.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct BootInfo {
    pub magic: u64,
    pub video_type: VideoModeType,
    pub fb_info: FramebufferInfo,
    pub memory_map_addr: u64,
    pub memory_map_len: u32,
    pub kernel_size: u64,
    pub pmm_metadata_base: u64,
    pub pmm_metadata_size: u64,
}

/// Statically allocated memory map buffer passed to the kernel.
static mut UNIFIED_MEM_MAP: [UnifiedMemoryEntry; 256] = [UnifiedMemoryEntry {
    start: 0,
    size: 0,
    is_usable: false,
}; 256];

/// Statically allocated BootInfo table initialized at boot time.
static mut BOOT_INFO: BootInfo = BootInfo {
    magic: 0x4B414F535F424F4F,
    video_type: VideoModeType::Framebuffer,
    fb_info: FramebufferInfo {
        base_address: 0,
        size: 0,
        width: 0,
        height: 0,
        pixels_per_scanline: 0,
        pixel_format: PixelFormat::Bgr,
    },
    memory_map_addr: 0,
    memory_map_len: 0,
    kernel_size: 0,
    pmm_metadata_base: 0,
    pmm_metadata_size: 0,
};

/// Entry point of the UEFI application.
///
/// # Safety
/// The `system_table` must be a valid pointer provided by the UEFI firmware.
/// The `image_handle` must be a valid image handle provided by the UEFI firmware.
#[no_mangle]
pub unsafe extern "efiapi" fn efi_main(
    image_handle: Handle,
    system_table: *const EfiSystemTable,
) -> Status {
    serial::init();

    // SAFETY:
    // - `system_table` is a valid pointer passed by the firmware.
    // - We read `con_out` and `boot_services` pointers from it.
    let con_out = unsafe { (*system_table).con_out };
    let boot_services = unsafe { (*system_table).boot_services };

    print(con_out, "KAOS UEFI loader v2: initialising loader...\r\n");

    // Step 1: Locate SimpleFileSystem on the boot device
    print(con_out, "  -> Querying LoadedImage protocol...\r\n");
    let mut loaded_image: *mut EfiLoadedImageProtocol = core::ptr::null_mut();
    // SAFETY: Retrieve Loaded Image protocol on our own image handle to get the device handle.
    let status = unsafe {
        ((*boot_services).handle_protocol)(
            image_handle,
            &EFI_LOADED_IMAGE_PROTOCOL_GUID,
            &mut loaded_image as *mut *mut EfiLoadedImageProtocol as *mut *mut c_void,
        )
    };
    if status != 0 {
        print(con_out, "ERROR: Failed to handle LoadedImage!\r\n");
        loop {}
    }

    // SAFETY: `loaded_image` contains the boot device handle.
    let device_handle = unsafe { (*loaded_image).device_handle };

    print(
        con_out,
        "  -> Querying SimpleFileSystem protocol on device...\r\n",
    );
    let mut fs: *mut EfiSimpleFileSystemProtocol = core::ptr::null_mut();
    // SAFETY: Open SimpleFileSystem on our boot device.
    let status = unsafe {
        ((*boot_services).handle_protocol)(
            device_handle,
            &EFI_SIMPLE_FILE_SYSTEM_PROTOCOL_GUID,
            &mut fs as *mut *mut EfiSimpleFileSystemProtocol as *mut *mut c_void,
        )
    };
    if status != 0 {
        print(
            con_out,
            "ERROR: Failed to handle SimpleFileSystem on device!\r\n",
        );
        loop {}
    }

    // Step 2: Query Graphics Output Protocol (GOP)
    print(con_out, "  -> Querying GOP...\r\n");

    let mut gop: *mut EfiGraphicsOutputProtocol = core::ptr::null_mut();

    // Attempt 1: Try handle_protocol on console_out_handle
    let console_out_handle = unsafe { (*system_table).console_out_handle };
    let mut status = unsafe {
        ((*boot_services).handle_protocol)(
            console_out_handle,
            &EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID,
            &mut gop as *mut *mut EfiGraphicsOutputProtocol as *mut *mut c_void,
        )
    };

    // Attempt 2: Fall back to locate_protocol system-wide
    if status != 0 {
        status = unsafe {
            ((*boot_services).locate_protocol)(
                &EFI_GRAPHICS_OUTPUT_PROTOCOL_GUID,
                core::ptr::null(),
                &mut gop as *mut *mut EfiGraphicsOutputProtocol as *mut *mut c_void,
            )
        };
    }

    if status != 0 {
        print(con_out, "ERROR: Failed to locate GOP! Status: ");
        print_hex(con_out, status);
        print(con_out, "\r\n");
        loop {}
    }

    // Read the GOP mode info
    // SAFETY:
    // - `gop` pointer was successfully located.
    // - `mode` and `info` fields are valid.
    let fb_base = unsafe { (*(*gop).mode).frame_buffer_start };
    let fb_size = unsafe { (*(*gop).mode).frame_buffer_size };
    let fb_width = unsafe { (*(*(*gop).mode).info).horizontal_resolution };
    let fb_height = unsafe { (*(*(*gop).mode).info).vertical_resolution };
    let fb_scanline = unsafe { (*(*(*gop).mode).info).pixels_per_scanline };
    let fb_pixel_format = unsafe { (*(*(*gop).mode).info).pixel_format };

    let pixel_format = match fb_pixel_format {
        0 => PixelFormat::Rgb,
        1 => PixelFormat::Bgr,
        2 => PixelFormat::Bitmask,
        3 => PixelFormat::BltOnly,
        _ => PixelFormat::Bgr,
    };

    // Fill BOOT_INFO fb_info
    // SAFETY: Modify the static BootInfo structure before ExitBootServices.
    unsafe {
        BOOT_INFO.fb_info = FramebufferInfo {
            base_address: fb_base,
            size: fb_size,
            width: fb_width,
            height: fb_height,
            pixels_per_scanline: fb_scanline,
            pixel_format,
        };
    }

    print(con_out, "  -> GOP queried successfully.\r\n");

    // Step 3: Open Volume and Load KERNEL.BIN
    print(con_out, "  -> Opening filesystem root...\r\n");
    let mut root_dir: *mut EfiFileProtocol = core::ptr::null_mut();
    // SAFETY: Open the file system root volume.
    let status = unsafe { ((*fs).open_volume)(fs, &mut root_dir) };
    if status != 0 {
        print(con_out, "ERROR: Failed to open root volume!\r\n");
        loop {}
    }

    print(con_out, "  -> Opening KERNEL.BIN...\r\n");
    let mut kernel_file: *mut EfiFileProtocol = core::ptr::null_mut();
    let kernel_name = [
        'K' as u16, 'E' as u16, 'R' as u16, 'N' as u16, 'E' as u16, 'L' as u16, '.' as u16,
        'B' as u16, 'I' as u16, 'N' as u16, 0,
    ];
    // Mode READ = 1, Attribute = 0
    // SAFETY: Open KERNEL.BIN from the root directory.
    let status =
        unsafe { ((*root_dir).open)(root_dir, &mut kernel_file, kernel_name.as_ptr(), 1, 0) };
    if status != 0 {
        print(con_out, "ERROR: Failed to open KERNEL.BIN!\r\n");
        loop {}
    }

    // Seek to end to find size
    // SAFETY: `kernel_file` is open and valid.
    let status = unsafe { ((*kernel_file).set_position)(kernel_file, 0xFFFFFFFFFFFFFFFF) };
    if status != 0 {
        print(
            con_out,
            "ERROR: Failed to set position to end of KERNEL.BIN!\r\n",
        );
        loop {}
    }

    let mut file_size: u64 = 0;
    // SAFETY: Retrieve current position (which is the file size).
    let status = unsafe { ((*kernel_file).get_position)(kernel_file, &mut file_size) };
    if status != 0 {
        print(con_out, "ERROR: Failed to get KERNEL.BIN file size!\r\n");
        loop {}
    }

    // Seek back to start
    // SAFETY: Reset file pointer to beginning for reading.
    let status = unsafe { ((*kernel_file).set_position)(kernel_file, 0) };
    if status != 0 {
        print(
            con_out,
            "ERROR: Failed to reset position to start of KERNEL.BIN!\r\n",
        );
        loop {}
    }

    // Step 4: Allocate memory at 0x100000 and read the kernel
    let mut kernel_addr: u64 = 0x100000;
    // Claim the entire low region 0x100000..0x400000 (768 pages / 3 MiB) as one block — not
    // just the ~89-page kernel image. The kernel places several things at fixed low addresses
    // beyond its image:
    //   * the PMM metadata/bitmaps, immediately after the kernel BSS, growing ~32 KiB per GiB
    //     of RAM (e.g. ~260 KiB at 8 GiB, ~2 MiB at 64 GiB), and
    //   * the bootstrap stack, top at 0x400000, growing downward.
    // On real UEFI hardware the firmware keeps critical structures (e.g. its page tables) in
    // low memory unless that memory is claimed. With only 96 pages reserved, the PMM bitmaps
    // overran the allocation and clobbered firmware page tables, triple-faulting in pmm::init
    // (QEMU/OVMF hid this by keeping its tables high in RAM). Reserving the whole 3 MiB keeps
    // the kernel, bitmaps and stack inside one block that stays within the kernel's 4 MiB
    // identity map; EfiLoaderCode marks it reserved in the UEFI memory map.
    let pages = 768;
    // AllocateType: AllocateAddress = 2 (allocate at the exact address in `kernel_addr`).
    // NOTE: 1 is AllocateMaxAddress, which treats `kernel_addr` as a *ceiling* and places the
    // allocation anywhere below it — that silently loaded the kernel at 0x40000 instead of
    // 0x100000. MemoryType: EfiLoaderCode = 1.
    print(
        con_out,
        "  -> Allocating memory for Kernel at 0x100000...\r\n",
    );
    // SAFETY: Allocate pages at address 0x100000 for the kernel memory layout (code + data + BSS).
    let status = unsafe { ((*boot_services).allocate_pages)(2, 1, pages, &mut kernel_addr) };
    if status != 0 {
        print(
            con_out,
            "ERROR: Failed to allocate memory at 0x100000 for kernel!\r\n",
        );
        loop {}
    }

    print(con_out, "  -> Reading KERNEL.BIN into memory...\r\n");
    let mut read_size = file_size as usize;
    // SAFETY: Read the kernel binary payload into the newly allocated buffer.
    let status =
        unsafe { ((*kernel_file).read)(kernel_file, &mut read_size, kernel_addr as *mut c_void) };
    if status != 0 || read_size != file_size as usize {
        print(con_out, "ERROR: Failed to read KERNEL.BIN!\r\n");
        loop {}
    }

    // Close files
    // SAFETY: Close open file handles.
    unsafe {
        ((*kernel_file).close)(kernel_file);
        ((*root_dir).close)(root_dir);
    }

    // Disable the UEFI watchdog timer. The firmware arms a ~5-minute watchdog before
    // launching a boot application; if it is left running it resets the machine. A
    // timeout of 0 disables it. This must be done while boot services are still alive.
    // SAFETY: standard boot-service call; null data pointer is valid for timeout 0.
    unsafe {
        ((*boot_services).set_watchdog_timer)(0, 0, 0, core::ptr::null());
    }

    // Step 5: Translate UEFI memory map & Exit Boot Services
    print(con_out, "  -> Exiting Boot Services...\r\n");
    let mut map_buf = [0u8; 32768];
    let mut memory_map_size = map_buf.len();
    let mut map_key: usize = 0;
    let mut descriptor_size: usize = 0;
    let mut descriptor_version: u32 = 0;

    // Reserve a PMM metadata region sized for the machine's RAM, BEFORE exiting boot
    // services (AllocatePages needs boot services). The kernel's PMM places its
    // bitmaps here instead of in fixed low memory; on large-RAM systems (e.g. 128 GiB
    // -> ~4 MiB of bitmaps) the old fixed placement overran into firmware memory and
    // triple-faulted in pmm::init. We size it from the usable RAM reported by an
    // initial get_memory_map (the ExitBootServices loop below re-fetches the map).
    {
        let mut probe_size = map_buf.len();
        let mut probe_key: usize = 0;
        let mut probe_desc_size: usize = 0;
        let mut probe_desc_ver: u32 = 0;
        // SAFETY: standard get_memory_map call into our stack buffer.
        let st = unsafe {
            ((*boot_services).get_memory_map)(
                &mut probe_size,
                map_buf.as_mut_ptr(),
                &mut probe_key,
                &mut probe_desc_size,
                &mut probe_desc_ver,
            )
        };
        if st == 0 && probe_desc_size > 0 {
            let n = probe_size / probe_desc_size;
            let mut total_frames: u64 = 0;
            let mut region_count: u64 = 0;
            for i in 0..n {
                // SAFETY: `i < n`, each descriptor occupies `probe_desc_size` bytes.
                let desc = unsafe {
                    &*(map_buf.as_ptr().add(i * probe_desc_size) as *const EfiMemoryDescriptor)
                };
                // EfiConventionalMemory = 7; mirror the kernel PMM's usable filter.
                if desc.memory_type == 7 && desc.physical_start >= 0x100000 {
                    total_frames += desc.number_of_pages;
                    region_count += 1;
                }
            }
            // header + region array + 1 bit per frame, plus generous slack.
            let meta_bytes = 0x4000 + region_count * 0x40 + (total_frames / 8) + 0x4000;

            // Step 5b: Calculate how many pages we need to hold the PMM metadata,
            // rounding up the byte size to the nearest page boundary.
            let meta_pages = meta_bytes.div_ceil(0x1000) + 8;
            let mut meta_addr: u64 = 0;
            // AllocateAnyPages = 0, EfiLoaderData = 2.
            let alloc_st = unsafe {
                ((*boot_services).allocate_pages)(0, 2, meta_pages as usize, &mut meta_addr)
            };
            if alloc_st == 0 {
                // SAFETY: write the static BootInfo before the kernel jump.
                unsafe {
                    BOOT_INFO.pmm_metadata_base = meta_addr;
                    BOOT_INFO.pmm_metadata_size = meta_pages * 0x1000;
                }
            }
        }
    }

    // Retry loop for ExitBootServices
    let mut exited = false;
    for _ in 0..5 {
        // SAFETY: Retrieve UEFI memory map.
        let status = unsafe {
            ((*boot_services).get_memory_map)(
                &mut memory_map_size,
                map_buf.as_mut_ptr(),
                &mut map_key,
                &mut descriptor_size,
                &mut descriptor_version,
            )
        };
        if status == 0 {
            // SAFETY: Call exit_boot_services to hand over hardware ownership.
            let exit_status =
                unsafe { ((*boot_services).exit_boot_services)(image_handle, map_key) };
            if exit_status == 0 {
                exited = true;
                break;
            }
        }
        memory_map_size = map_buf.len();
    }

    if !exited {
        print(con_out, "ERROR: Failed to Exit Boot Services!\r\n");
        loop {}
    }

    // Step 6: Build Unified Memory Map
    // Now UEFI services are dead, we work directly on memory.
    let num_descriptors = memory_map_size / descriptor_size;
    let mut usable_entry_count = 0u32;

    // SAFETY:
    // - Parse the memory map retrieved just before ExitBootServices.
    // - Fill the static mutable array `UNIFIED_MEM_MAP` for the kernel.
    unsafe {
        for i in 0..num_descriptors {
            let desc_ptr = map_buf.as_ptr().add(i * descriptor_size) as *const EfiMemoryDescriptor;
            let desc = &*desc_ptr;

            // EfiConventionalMemory = 7
            let is_usable = desc.memory_type == 7;

            if usable_entry_count < 256 {
                UNIFIED_MEM_MAP[usable_entry_count as usize] = UnifiedMemoryEntry {
                    start: desc.physical_start,
                    size: desc.number_of_pages * 4096,
                    is_usable,
                };
                usable_entry_count += 1;
            }
        }

        BOOT_INFO.memory_map_addr = &raw const UNIFIED_MEM_MAP[0] as u64;
        BOOT_INFO.memory_map_len = usable_entry_count;
        BOOT_INFO.kernel_size = file_size;
    }
    // Step 7: Map higher-half kernel to UEFI page tables by mirroring PML4 entry 0 to entry 256.
    // SAFETY:
    // - CR3 contains the physical address of the active PML4 table.
    // - Under UEFI, physical memory is identity mapped, so the physical address of PML4 is also its virtual address.
    // - PML4 index 256 covers virtual addresses starting at 0xFFFF800000000000. Copying entry 0 to entry 256 mirrors the lower 512GB identity mappings into the higher-half.
    // - We temporarily disable the WP (Write Protect) bit in CR0 to allow modifying the write-protected page table page.
    // - We reload CR3 with its current value to flush the CPU's TLB/page-walk cache, ensuring the new virtual mapping is active.
    unsafe {
        let mut cr0: u64;
        let mut cr3: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0, options(nomem, nostack, preserves_flags));
        core::arch::asm!("mov {}, cr3", out(reg) cr3, options(nomem, nostack, preserves_flags));

        // Clear WP bit (bit 16) in CR0
        let cr0_new = cr0 & !(1 << 16);
        core::arch::asm!("mov cr0, {}", in(reg) cr0_new, options(nomem, nostack, preserves_flags));

        let pml4_addr = cr3 & 0x000F_FFFF_FFFF_F000;
        let pml4 = pml4_addr as *mut u64;
        *pml4.add(256) = *pml4.add(0);

        // Restore CR0 (re-enabling WP)
        core::arch::asm!("mov cr0, {}", in(reg) cr0, options(nomem, nostack, preserves_flags));

        // Flush TLB page walk cache
        core::arch::asm!("mov cr3, {}", in(reg) cr3, options(nomem, nostack, preserves_flags));
    }

    // Step 8: Jump to Kernel
    // Kernel entry point is expected at virtual 0xFFFF800000100000.
    // SAFETY:
    // - The kernel binary is fully loaded at physical 0x100000, which is mapped to 0xFFFF800000100000 via our PML4 mirroring.
    // - We set RSP to physical address 0x400000 (which is identity mapped and serves as the kernel's bootstrap stack).
    // - Pass a pointer to the prepared BootInfo structure in RDI (SysV x86_64 calling convention).
    // - We use inline assembly to set the stack, clear RBP, and jump to the kernel entry point.
    unsafe {
        let entry_point: usize = 0xFFFF800000100000;
        core::arch::asm!(
            "mov rsp, 0x400000",
            "xor rbp, rbp",
            "jmp {entry}",
            entry = in(reg) entry_point,
            in("rdi") &raw const BOOT_INFO,
            options(noreturn)
        );
    }
}

/// Writes an ASCII string to the UEFI console via `ConOut->OutputString`.
fn print(con_out: *mut EfiSimpleTextOutputProtocol, s: &str) {
    serial::write_str(s);

    const CHUNK: usize = 64;
    let mut buffer = [0u16; CHUNK + 1];
    let mut len = 0;

    for byte in s.bytes() {
        buffer[len] = byte as u16;
        len += 1;

        if len == CHUNK {
            buffer[len] = 0;
            flush(con_out, &buffer);
            len = 0;
        }
    }

    if len > 0 {
        buffer[len] = 0;
        flush(con_out, &buffer);
    }
}

/// Hands a single NUL-terminated UCS-2 buffer to the firmware to print.
fn flush(con_out: *mut EfiSimpleTextOutputProtocol, buffer: &[u16]) {
    // SAFETY: `con_out` is a valid protocol pointer, and `buffer` is NUL-terminated.
    unsafe {
        ((*con_out).output_string)(con_out, buffer.as_ptr());
    }
}

/// Prints a hex representation of a `usize` value to the screen.
fn print_hex(con_out: *mut EfiSimpleTextOutputProtocol, val: usize) {
    let mut buf = [0u16; 19];
    buf[0] = '0' as u16;
    buf[1] = 'x' as u16;
    let chars = b"0123456789ABCDEF";
    for i in 0..16 {
        let shift = (15 - i) * 4;
        let digit = (val >> shift) & 0xF;
        buf[2 + i] = chars[digit] as u16;
    }
    buf[18] = 0;
    flush(con_out, &buf);
}

/// Panic handler.
#[panic_handler]
fn panic(_info: &PanicInfo) -> ! {
    loop {}
}
