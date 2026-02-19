#![no_std]
#![no_main]

#[path = "../../common/syscall.rs"]
mod syscall;

const PROMPT_MSG: &[u8] = b"Enter your name: ";
const OUTPUT_PREFIX: &[u8] = b"Your name is: ";
const NEWLINE: &[u8] = b"\n";
const ERROR_MSG: &[u8] = b"READLINE.BIN: user_readline failed\n";

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    let mut line = [0u8; 128];

    // SAFETY:
    // - All message slices are valid static byte ranges.
    // - Kernel validates user pointer/length on syscall boundary.
    unsafe {
        let _ = syscall::write_console(PROMPT_MSG.as_ptr(), PROMPT_MSG.len());
    }

    match syscall::user_readline(&mut line) {
        Ok(line_len) => {
            // SAFETY:
            // - Prefix/static buffers and `line[..line_len]` are valid reads.
            // - `line_len` was produced by `user_readline` for this buffer.
            unsafe {
                let _ = syscall::write_console(OUTPUT_PREFIX.as_ptr(), OUTPUT_PREFIX.len());
                let _ = syscall::write_console(line.as_ptr(), line_len);
                let _ = syscall::write_console(NEWLINE.as_ptr(), NEWLINE.len());
            }
        }
        Err(_) => {
            // SAFETY:
            // - `ERROR_MSG` points to valid static bytes.
            unsafe {
                let _ = syscall::write_console(ERROR_MSG.as_ptr(), ERROR_MSG.len());
            }
        }
    }

    syscall::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall::exit()
}
