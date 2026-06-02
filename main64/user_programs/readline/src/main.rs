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

    let _ = syscall::write_console(PROMPT_MSG);

    match syscall::user_readline(&mut line) {
        Ok(line_len) => {
            let _ = syscall::write_console(OUTPUT_PREFIX);
            let _ = syscall::write_console(&line[..line_len]);
            let _ = syscall::write_console(NEWLINE);
        }
        Err(_) => {
            let _ = syscall::write_console(ERROR_MSG);
        }
    }

    syscall::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall::exit()
}
