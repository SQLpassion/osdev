#![no_std]
#![no_main]

use lib_kaos as syscall;

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    let _ = syscall::write_console(b"=== FILEDEMO [Ring 3] ===\n");

    let filename = b"test.txt";

    // Check if the file already exists
    if syscall::file_exists(filename) {
        let _ = syscall::write_console(b"test.txt already exists. Deleting it to write new content.\n");
        if syscall::delete_file(filename).is_err() {
            let _ = syscall::write_console(b"Error deleting existing file.\n");
            syscall::exit();
        }
    } else {
        let _ = syscall::write_console(b"test.txt does not exist. Creating new file.\n");
    }

    let _ = syscall::write_console(b"Enter file content: ");
    let mut content_buf = [0u8; 128];
    let content_len = match syscall::user_readline(&mut content_buf) {
        Ok(len) => len,
        Err(_) => {
            let _ = syscall::write_console(b"Error reading content.\n");
            syscall::exit();
        }
    };

    {
        let mut file = match syscall::File::open(filename, syscall::FileMode::Write) {
            Ok(f) => f,
            Err(_) => {
                let _ = syscall::write_console(b"Error: Could not open file in write mode.\n");
                syscall::exit();
            }
        };

        let _ = syscall::write_console(b"Writing content...\n");
        match file.write(&content_buf[..content_len]) {
            Ok(written) if written == content_len => {}
            _ => {
                let _ = syscall::write_console(b"Error: Could not write data to file.\n");
                syscall::exit();
            }
        }
    }
    let _ = syscall::write_console(b"File successfully saved and closed.\n\n");
    let _ = syscall::write_console(b"FILEDEMO complete. Exiting task.\n");
    syscall::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    syscall::exit()
}
