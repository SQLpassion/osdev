#![no_std]
#![no_main]

use lib_kaos::{console, fs, process, print, println};

#[no_mangle]
#[link_section = ".ltext._start"]
pub extern "C" fn _start() -> ! {
    println!("=== FILEDEMO [Ring 3] ===");

    let filename = "test.txt";

    // Check if the file already exists
    if fs::file_exists(filename) {
        println!("test.txt already exists. Deleting it to write new content.");
        if fs::delete_file(filename).is_err() {
            println!("Error deleting existing file.");
            process::exit();
        }
    } else {
        println!("test.txt does not exist. Creating new file.");
    }

    print!("Enter file content: ");
    let mut content_buf = [0u8; 128];
    let content_len = match console::readline(&mut content_buf) {
        Ok(len) => len,
        Err(_) => {
            println!("Error reading content.");
            process::exit();
        }
    };

    {
        let mut file = match fs::File::open(filename, fs::FileMode::Write) {
            Ok(f) => f,
            Err(_) => {
                println!("Error: Could not open file in write mode.");
                process::exit();
            }
        };

        println!("Writing content...");
        match file.write(&content_buf[..content_len]) {
            Ok(written) if written == content_len => {}
            _ => {
                println!("Error: Could not write data to file.");
                process::exit();
            }
        }
    }
    println!("File successfully saved and closed.\n");
    println!("FILEDEMO complete. Exiting task.");
    process::exit()
}

#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    process::exit()
}
