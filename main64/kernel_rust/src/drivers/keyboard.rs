//! PS/2 keyboard driver (Rust port of the C keyboard driver)
//!
//! Handles scan code processing and stores decoded input in a ring buffer.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::port::PortByte;
use crate::drivers::screen::Screen;

/// Keyboard controller ports
const KYBRD_CTRL_STATS_REG: u16 = 0x64;
const KYBRD_ENC_INPUT_BUF: u16 = 0x60;

/// Keyboard status mask (output buffer full)
const KYBRD_CTRL_STATS_MASK_OUT_BUF: u8 = 0x01;

/// Scan code table size (0x00..=0x58)
const SCANCODE_TABLE_LEN: usize = 0x59;

/// Ring buffer capacity (must be > 1)
const INPUT_BUFFER_CAPACITY: usize = 256;
const RAW_BUFFER_CAPACITY: usize = 64;

/// Lower-case QWERTZ scan code map (printable ASCII only; 0 == ignored)
const SCANCODES_LOWER: [u8; SCANCODE_TABLE_LEN] = [
    0, 0, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b's', b'=', 0x08, 0, b'q',
    b'w', b'e', b'r', b't', b'z', b'u', b'i', b'o', b'p', b'[', b'+', b'\n', 0, b'a', b's', b'd',
    b'f', b'g', b'h', b'j', b'k', b'l', b'{', b'~', b'<', 0, b'#', b'y', b'x', b'c', b'v', b'b',
    b'n', b'm', b',', b'.', b'-', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Upper-case QWERTZ scan code map (printable ASCII only; 0 == ignored)
const SCANCODES_UPPER: [u8; SCANCODE_TABLE_LEN] = [
    0, 0, b'!', b'"', b'0', b'$', b'%', b'&', b'/', b'(', b')', b'=', b'?', b'`', 0x08, 0, b'Q',
    b'W', b'E', b'R', b'T', b'Z', b'U', b'I', b'O', b'P', b']', b'*', b'\n', 0, b'A', b'S', b'D',
    b'F', b'G', b'H', b'J', b'K', b'L', b'}', b'@', b'>', 0, b'\\', b'Y', b'X', b'C', b'V', b'B',
    b'N', b'M', b';', b':', b'_', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

#[derive(Debug, Clone, Copy)]
struct KeyboardState {
    shift: bool,
    caps_lock: bool,
    left_ctrl: bool,
}

/// Lock-free ring buffer for keyboard input (single producer, single consumer).
struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    head: AtomicUsize,
    tail: AtomicUsize,
}

impl<const N: usize> RingBuffer<N> {
    const fn new() -> Self {
        Self {
            buf: UnsafeCell::new([0; N]),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
        }
    }

    fn clear(&self) {
        self.head.store(0, Ordering::Relaxed);
        self.tail.store(0, Ordering::Relaxed);
    }

    fn push(&self, value: u8) -> bool {
        let head = self.head.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        let tail = self.tail.load(Ordering::Acquire);

        if next == tail {
            return false;
        }

        unsafe {
            (*self.buf.get())[head] = value;
        }

        self.head.store(next, Ordering::Release);
        true
    }

    fn pop(&self) -> Option<u8> {
        let tail = self.tail.load(Ordering::Relaxed);
        let head = self.head.load(Ordering::Acquire);

        if tail == head {
            return None;
        }

        let value = unsafe { (*self.buf.get())[tail] };
        let next = (tail + 1) % N;
        self.tail.store(next, Ordering::Release);
        Some(value)
    }
}

unsafe impl<const N: usize> Sync for RingBuffer<N> {}

struct Keyboard {
    raw: RingBuffer<RAW_BUFFER_CAPACITY>,
    buffer: RingBuffer<INPUT_BUFFER_CAPACITY>,
    state: UnsafeCell<KeyboardState>,
}

impl Keyboard {
    const fn new() -> Self {
        Self {
            raw: RingBuffer::new(),
            buffer: RingBuffer::new(),
            state: UnsafeCell::new(KeyboardState {
                shift: false,
                caps_lock: false,
                left_ctrl: false,
            }),
        }
    }

    /// # Safety
    /// Caller must ensure no concurrent access to keyboard state.
    /// Safe in single-threaded kernel context with proper IRQ handling.
    #[allow(clippy::mut_from_ref)]
    unsafe fn state_mut(&self) -> &mut KeyboardState {
        &mut *self.state.get()
    }
}

unsafe impl Sync for Keyboard {}

static KEYBOARD: Keyboard = Keyboard::new();

/// Initialize the keyboard driver state.
pub fn init() {
    KEYBOARD.raw.clear();
    KEYBOARD.buffer.clear();
    let state = unsafe { KEYBOARD.state_mut() };
    state.shift = false;
    state.caps_lock = false;
    state.left_ctrl = false;
}

/// Handle IRQ1 (keyboard) top half: enqueue raw scancode only.
pub fn handle_irq() {
    let status = unsafe { PortByte::new(KYBRD_CTRL_STATS_REG).read() };
    if (status & KYBRD_CTRL_STATS_MASK_OUT_BUF) == 0 {
        return;
    }

    let code = unsafe { PortByte::new(KYBRD_ENC_INPUT_BUF).read() };
    let _ = KEYBOARD.raw.push(code);
}

/// Bottom half: drain raw scancodes and decode them. Call this regularly from
/// your main loop before consuming characters.
pub fn poll() {
    while let Some(code) = KEYBOARD.raw.pop() {
        handle_scancode(code);
    }
}

/// Read a decoded character if available; returns None when the buffer is empty.
/// Call `poll()` before this to process any pending scancodes.
pub fn read_char() -> Option<u8> {
    KEYBOARD.buffer.pop()
}

fn handle_scancode(code: u8) {
    if (code & 0x80) != 0 {
        handle_break(code & 0x7f);
    } else {
        handle_make(code);
    }
}

fn handle_break(code: u8) {
    let state = unsafe { KEYBOARD.state_mut() };
    match code {
        0x1d => state.left_ctrl = false,
        0x2a | 0x36 => state.shift = false,
        _ => {}
    }
}

fn handle_make(code: u8) {
    let state = unsafe { KEYBOARD.state_mut() };
    match code {
        0x1d => {
            state.left_ctrl = true;
            return;
        }
        0x3a => {
            state.caps_lock = !state.caps_lock;
            return;
        }
        0x2a | 0x36 => {
            state.shift = true;
            return;
        }
        _ => {}
    }

    let use_upper = if is_alpha(code) {
        state.shift ^ state.caps_lock
    } else {
        state.shift
    };

    let table = if use_upper {
        &SCANCODES_UPPER
    } else {
        &SCANCODES_LOWER
    };

    let Some(&key) = table.get(code as usize) else {
        return;
    };

    if key != 0 {
        let _ = KEYBOARD.buffer.push(key);
    }
}

fn is_alpha(code: u8) -> bool {
    matches!(
        code,
        0x10..=0x19 // Q..P
            | 0x1e..=0x26 // A..L
            | 0x2c..=0x32 // Z..M
    )
}

/// Read a line into `buf`, echoing to `screen`. Returns the number of bytes written.
/// The newline is echoed but not stored in `buf`.
pub fn read_line(screen: &mut Screen, buf: &mut [u8]) -> usize {
    let mut len = 0;

    loop {
        poll();

        if let Some(ch) = read_char() {
            match ch {
                b'\r' | b'\n' => {
                    // Echo LF; terminate input
                    screen.print_char(b'\n');
                    break;
                }
                0x08 => {
                    // Backspace: erase last character if any
                    if len > 0 {
                        len -= 1;
                        screen.print_char(0x08);
                        screen.print_char(b' ');
                        screen.print_char(0x08);
                    }
                }
                _ => {
                    if len < buf.len() {
                        buf[len] = ch;
                        len += 1;
                        screen.print_char(ch);
                    } else {
                        // Optional: bell on overflow
                        // screen.print_char(0x07);
                    }
                }
            }
        } else {
            // Sleep until the next interrupt to avoid busy-waiting
            unsafe {
                core::arch::asm!("hlt");
            }
        }
    }

    len
}
