//! Test to ensure the serial writer returns within a bounded loop
//! if the UART never reports transmit-empty.

#![no_std]
#![no_main]
#![feature(custom_test_frameworks)]
#![test_runner(kaos_kernel::testing::test_runner)]
#![reexport_test_harness_main = "test_main"]

use kaos_kernel::arch::qemu::{exit_qemu, QemuExitCode};
use kaos_kernel::drivers::serial::Serial;

#[no_mangle]
#[link_section = ".text.boot"]
pub extern "C" fn KernelMain(_kernel_size: u64) -> ! {
    test_main();
    exit_qemu(QemuExitCode::Success);
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    kaos_kernel::testing::test_panic_handler(info)
}

#[test_case]
fn test_serial_timeout_on_hung_uart() {
    // Create a new serial instance
    let mut serial = Serial::new();

    // We hack the base_port to a port that is guaranteed to not have a working UART.
    // Port 0x80 is used for POST codes and reading from it on QEMU typically
    // does not yield 0x20 (THRE). If it reads 0xFF, THRE is set.
    // If we want to guarantee THRE is 0, we can use an unmapped port that reads 0x00,
    // or just rely on the timeout to break the loop regardless.
    unsafe {
        // base_port is the first and only field of Serial.
        let base_port_ptr = &mut serial as *mut _ as *mut u16;
        // Port 0x00 is DMA controller channel 0 current address, typically reads 0 if not set.
        *base_port_ptr = 0x00;
    }

    // Write a byte. If the loop is unbounded and the port reads 0, this will hang forever
    // and the test will timeout. With the bound, it will return.
    serial.write_byte(b'X');
}
