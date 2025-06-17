#![no_std]
#![no_main]

use core::panic::PanicInfo;
use core::arch::asm;

pub const KEY_RETURN: u8 = b'\r';
pub const KEY_BACKSPACE: u8 = b'\r';

/// This function is called on panic
#[panic_handler]
fn panic(_info: &PanicInfo) -> !
{
    loop {}
}

// Main entry point
#[unsafe(no_mangle)]
pub extern "C" fn _start()
{
    // Welcome message
    printf("Klaus Aschenbrenner loves low level coding!\n\n\0");

    // Read something from the keyboard
    printf("Please enter your input: \0");
    let mut buffer = [0u8; 10];
    scanf(&mut buffer);

    // Print out the entered string
    let string = unsafe { core::str::from_utf8_unchecked(&buffer) };
    printf("Your entered input was: \0");
    printf(string);
    printf("\n\n\0");
    
    // End the program
    printf("Finished!\n\0");
    terminate_process();
}

#[inline(never)]
#[unsafe(no_mangle)]
fn printf(string: &str)
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
        );
    }
}

// ***The "no_mangle" attribute was removed "on purpose", because otherwise the prgram crashes - no idea why...***
// #[unsafe(no_mangle)]
#[inline(never)]
pub fn scanf(buffer: &mut [u8])
{
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
            printf("\n\0");
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
                let buffer = [key, 0];
                let result = unsafe { core::str::from_utf8_unchecked(&buffer) };
                printf(result);
            }

            buffer[i] = key;
            i += 1;
        }
    }
}

#[inline(never)]
#[unsafe(no_mangle)]
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
            out("rsi") _,
        );
    }

    result as u8
}

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
}

// #[inline(never)]
fn clear_screen()
{
    unsafe
    {
        asm!(
            "MOV RDI, 9",
            "INT 0x80",
            options(nostack, preserves_flags)
        );
    }
}

// #[inline(never)]
fn print_root_directory()
{
    unsafe
    {
        asm!(
            "MOV RDI, 8",
            "INT 0x80",
            options(nostack, preserves_flags)
        );
    }
} */