#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::arch::asm;

pub const KEY_RETURN: u8 = b'\r';
pub const KEY_BACKSPACE: u8 = b'\x08';

unsafe extern "C"
{
    unsafe fn PrintRootDirectory() -> i32;
}

unsafe extern "C"
{
    unsafe fn ClearScreen() -> i32;
}

unsafe extern "C"
{
    unsafe fn printf_syscall_wrapper(s: *const u8);
}

unsafe extern "C" {
    unsafe fn scanf_syscall_wrapper(buffer: *mut u8, buffer_size: i32);
}

pub fn printf(s: &str) {
    const MAX_LEN: usize = 1024;
    let bytes = s.as_bytes();
    let len = core::cmp::min(bytes.len(), MAX_LEN - 1);

    // SAFETY: This allocates a local buffer and ensures a null-terminated copy
    let mut buf = [0u8; MAX_LEN];
    for i in 0..len {
        buf[i] = bytes[i];
    }
    buf[len] = 0;

    unsafe {
        printf_syscall_wrapper(buf.as_ptr());
    }
}

pub fn scanf(buf: &mut [u8]) {
    unsafe {
        scanf_syscall_wrapper(buf.as_mut_ptr(), buf.len() as i32);
    }
}

/// This function is called on panic
#[panic_handler]
fn panic(_info: &PanicInfo) -> !
{
    printf_old("Panic!!!\n\0");
    loop {}
}

// Main entry point
#[unsafe(no_mangle)]
pub extern "C" fn _start()
{
    unsafe { ClearScreen(); PrintRootDirectory() };

    // Welcome message
    printf("Klaus Aschenbrenner loves low level coding!!!\n\n");

    printf("Please enter your input: ");
    const BUF_LEN: usize = 128;
    let mut buf = [0u8; BUF_LEN];

    scanf(&mut buf);

    /* // Find the null terminator
    let len = buf.iter().position(|&c| c == 0).unwrap_or(BUF_LEN);

    // SAFETY: We assume syscall returns valid UTF-8
    unsafe
    { 
        let msg = str::from_utf8_unchecked(&buf[..len]);
        printf(msg);
    } */

    /* // Read something from the keyboard
    printf("Please enter your input: \0");
    let mut buffer = [0u8; 10];
    scanf(&mut buffer); */

    /* // Print out the entered string
    let string = unsafe { core::str::from_utf8_unchecked(&buffer) };
    printf("Your entered input was: \0");
    printf(string);
    printf("\n\n\0"); */

    // End the program
    printf_old("Finished!\n\0");
    // terminate_process();
}

#[inline(never)]
#[unsafe(no_mangle)]
fn printf_old(string: &str)
{
    let ptr = string.as_ptr();

    unsafe
    {
        asm!(
            "MOV RDI, 1",
            "MOV RSI, {0}",
            "INT 0x80",
            in(reg) ptr,
            out("rdi") _,
            out("rsi") _,
            out("rax") _,
            options(nostack, nomem)
        );
    }
}

/* // ***The "no_mangle" attribute was removed "on purpose", because otherwise the program crashes - no idea why...***
// #[unsafe(no_mangle)]
#[inline(never)]
pub fn scanf(buffer1: &mut [u8])
{
    let mut buffer = [0u8; 10];
    let mut i = 0;

    while i < buffer.len()
    {
        let mut key: u8 = 0;

        // Wait for a key
        while key == 0
        {
            key = getchar();
        }

        let process_key = true;

        if key == KEY_RETURN
        {
            printf_old("\n\0");
            break;
        }

        /* if key == KEY_BACKSPACE
        {
            process_key = false;

            if i > 0
            {
                let (mut row, mut col) = get_cursor_position();

                if col > 0
                {
                    col -= 1;
                }

                set_cursor_position(row, col);
                printf(" "); // erase char
                set_cursor_position(row, col); // move back again
                i -= 1;
            }
        } */

        if process_key
        {
            if key != 0
            {
                let temp = [key, 0];
                let result = unsafe { core::str::from_utf8_unchecked(&temp) };
                printf_old(result);
            }

            buffer[i] = key;
            i += 1;
        }
    }
} */

/* #[inline(never)]
// #[unsafe(no_mangle)]
fn getchar() -> u8
{
    let result: u64;

    unsafe
    {
        asm!(
            "MOV RDI, 4",
            "INT 0x80",
            out("rax") result,
            out("rdi") _,
            options(nostack, nomem)
        );
    }

    result as u8
} */

#[inline(never)]
#[unsafe(no_mangle)]
fn terminate_process() -> !
{
    unsafe
    {
        asm!(
            "MOV RDI, 3",
            "INT 0x80",
            out("rdi") _,
            options(nostack, nomem)
        );
    }

    loop {}
}

/* // #[inline(never)]
fn get_cursor_position(row: &mut i32, col: &mut i32)
{
    let row_ptr = row as *mut i32;
    let col_ptr = col as *mut i32;

    unsafe
    {
        asm!
        (
            "MOV RDI, 5",
            "MOV RSI, {0}",
            "MOV RDX, {1}",
            "INT 0x80",
            in(reg) row_ptr,
            in(reg) col_ptr,
        );
    }
}

// #[inline(never)]
fn set_cursor_position(row: &mut i32, col: &mut i32)
{
    let row_ptr = row as *mut i32;
    let col_ptr = col as *mut i32;

    unsafe
    {
        asm!
        (
            "MOV RDI, 6",
            "MOV RSI, {0}",
            "MOV RDX, {1}",
            "INT 0x80",
            in(reg) row_ptr,
            in(reg) col_ptr,
            options(nostack, preserves_flags)
        );
    }
} */