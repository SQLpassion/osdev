//! PS/2 keyboard driver (Rust port of the C keyboard driver)
//!
//! Handles scan code processing and stores decoded input in a ring buffer.

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicUsize, Ordering};

use crate::arch::port::PortByte;
use crate::drivers::screen::Screen;
use crate::scheduler;
use crate::sync::waitqueue::WaitQueue;

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

/// Lock-free ring buffer for keyboard input.
///
/// The buffer supports **single producer, multiple consumer (SPMC)** access:
///
/// - `push()` is safe for exactly **one** producer (no CAS needed because only
///   one writer ever advances `head_producer`).
/// - `pop()` is safe for **multiple** concurrent consumers.  It uses a
///   compare-and-swap (CAS) loop on `tail_consumer` so that two tasks calling
///   `pop()` at the same time never receive the same element.
///
/// This matters because the keyboard architecture uses wake-all semantics:
/// when decoded characters become available, *all* waiting consumer tasks are
/// woken and race to call `pop()`.  Without CAS the plain `load → read →
/// store` sequence in `pop()` would allow two consumers to read the same
/// `tail_consumer` index and both obtain the same character (duplicate
/// delivery).
///
/// ## Layout
///
/// Both `head_producer` and `tail_consumer` advance with **modular
/// arithmetic** (`% N`), wrapping from the last slot back to index 0.  This
/// makes the fixed-size array reusable in a cycle — hence "ring" buffer:
///
/// ```text
///    tail_consumer      head_producer
///         ▼                   ▼
///   ┌───┬───┬───┬───┬───┬───┬───┬───┐
///   │   │ A │ B │ C │ D │   │   │   │   N = 8
///   └───┴───┴───┴───┴───┴───┴───┴───┘
///         ▲               ▲
///       pop()           push()
///    (consumers)      (producer)
/// ```
///
/// After enough push/pop operations both indices can wrap around so that
/// `head_producer` is at a lower array index than `tail_consumer`:
///
/// ```text
///          head_producer   tail_consumer
///               ▼             ▼
///   ┌───┬───┬───┬───┬───┬───┬───┬───┐
///   │ X │ Y │   │   │   │   │ W │   │   N = 8
///   └───┴───┴───┴───┴───┴───┴───┴───┘
///   ← wrapped                  oldest
/// ```
///
/// - `head_producer` — **write index** (producer side).  Points to the next
///   free slot where `push()` will store a byte.  Advanced by the single
///   producer after writing.
/// - `tail_consumer` — **read index** (consumer side).  Points to the oldest
///   unread byte that `pop()` will return.  Advanced atomically (CAS) by
///   whichever consumer successfully claims the slot.
/// - The buffer is **empty** when `tail_consumer == head_producer` and **full**
///   when `(head_producer + 1) % N == tail_consumer` (one slot is always left
///   unused to distinguish full from empty).
struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    /// Write index (producer side): points to the next free slot.
    head_producer: AtomicUsize,
    /// Read index (consumer side): points to the oldest unread byte.
    tail_consumer: AtomicUsize,
}

impl<const N: usize> RingBuffer<N> {
    const fn new() -> Self {
        Self {
            buf: UnsafeCell::new([0; N]),
            head_producer: AtomicUsize::new(0),
            tail_consumer: AtomicUsize::new(0),
        }
    }

    fn is_empty(&self) -> bool {
        self.tail_consumer.load(Ordering::Acquire) == self.head_producer.load(Ordering::Acquire)
    }

    fn clear(&self) {
        self.head_producer.store(0, Ordering::Relaxed);
        self.tail_consumer.store(0, Ordering::Relaxed);
    }

    /// Append a byte to the buffer.  Returns `true` on success, `false` if
    /// the buffer is full (the byte is silently dropped in that case).
    ///
    /// Only safe for a **single producer** — no CAS is needed because exactly
    /// one writer ever advances `head_producer`.  The `Release` store on
    /// `head_producer` ensures that the byte written to `buf[head_producer]` is
    /// visible to any consumer that later observes the new `head_producer`
    /// value via an `Acquire` load.
    fn push(&self, value: u8) -> bool {
        let head = self.head_producer.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        let tail = self.tail_consumer.load(Ordering::Acquire);

        // Buffer full: the next slot would collide with tail_consumer.
        // One slot is intentionally wasted so that full != empty.
        if next == tail {
            return false;
        }

        // Write the byte *before* publishing the new head_producer.
        unsafe {
            (*self.buf.get())[head] = value;
        }

        // Publish: consumers can now see this slot.
        self.head_producer.store(next, Ordering::Release);
        true
    }

    /// Remove and return the next element, or `None` if the buffer is empty.
    ///
    /// Uses a **CAS loop** to safely support multiple concurrent consumers:
    ///
    /// 1. Load the current `tail_consumer` index.
    /// 2. If `tail_consumer == head_producer` the buffer is empty → return
    ///    `None`.
    /// 3. Speculatively read the byte at `buf[tail_consumer]`.
    /// 4. Attempt to atomically advance `tail_consumer` via
    ///    `compare_exchange_weak`.
    ///    - **Success**: no other consumer changed `tail_consumer` between
    ///      step 1 and 4, so our read is valid → return the byte.
    ///    - **Failure**: another consumer already advanced `tail_consumer` (it
    ///      consumed the same slot first) → loop back to step 1 and retry with
    ///      the updated `tail_consumer` value.
    ///
    /// The speculative read in step 3 is safe because the producer only writes
    /// to `buf[head_producer]` *before* advancing `head_producer` with
    /// `Release` ordering, and we read `head_producer` with `Acquire` in
    /// step 2.  A slot between `tail_consumer` and `head_producer` is therefore
    /// always initialised and stable.
    fn pop(&self) -> Option<u8> {
        loop {
            let tail = self.tail_consumer.load(Ordering::Acquire);
            let head = self.head_producer.load(Ordering::Acquire);

            if tail == head {
                return None;
            }

            // Speculatively read the value — valid because the producer has
            // already published this slot (head_producer is past tail_consumer).
            let value = unsafe { (*self.buf.get())[tail] };
            let next = (tail + 1) % N;

            // Try to claim this slot by advancing tail_consumer.  If another
            // consumer beat us to it, `tail_consumer` has already moved and the
            // CAS fails — we simply retry with the new tail_consumer value.
            match self.tail_consumer.compare_exchange_weak(
                tail,
                next,
                Ordering::AcqRel,
                Ordering::Acquire,
            ) {
                Ok(_) => return Some(value),
                Err(_) => continue,
            }
        }
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

/// Wakes the keyboard worker task when raw scancodes are available.
static RAW_WAITQUEUE: WaitQueue = WaitQueue::new();

/// Wakes consumer tasks when decoded characters are available.
static INPUT_WAITQUEUE: WaitQueue = WaitQueue::new();

/// Initialize the keyboard driver state.
pub fn init() {
    KEYBOARD.raw.clear();
    KEYBOARD.buffer.clear();
    let state = unsafe { KEYBOARD.state_mut() };
    state.shift = false;
    state.caps_lock = false;
    state.left_ctrl = false;
}

/// Handle IRQ1 (keyboard) top half: enqueue raw scancode and wake the
/// keyboard worker task so it can decode the scancode into ASCII.
pub fn handle_irq() {
    let status = unsafe { PortByte::new(KYBRD_CTRL_STATS_REG).read() };
    if (status & KYBRD_CTRL_STATS_MASK_OUT_BUF) == 0 {
        return;
    }

    let code = unsafe { PortByte::new(KYBRD_ENC_INPUT_BUF).read() };
    let _ = KEYBOARD.raw.push(code);
    RAW_WAITQUEUE.wake_all();
}

/// Bottom half: drain raw scancodes and decode them. Call this regularly from
/// your main loop before consuming characters.
pub fn poll() {
    while let Some(code) = KEYBOARD.raw.pop() {
        handle_scancode(code);
    }
}

/// Read a decoded character if available; returns None when the buffer is empty.
pub fn read_char() -> Option<u8> {
    KEYBOARD.buffer.pop()
}

/// Block the calling task until a decoded character is available.
///
/// Uses [`WaitQueue::sleep_if`] with an emptiness check under disabled
/// interrupts to prevent lost wakeups.  On wakeup the task re-checks the
/// buffer; if another consumer grabbed the character first (thundering herd),
/// it sleeps again.
pub fn read_char_blocking() -> u8 {
    loop {
        if let Some(ch) = read_char() {
            return ch;
        }

        let task_id = scheduler::current_task_id()
            .expect("read_char_blocking called outside scheduled task");

        if INPUT_WAITQUEUE.sleep_if(task_id, || KEYBOARD.buffer.is_empty()) {
            scheduler::yield_now();
        }
    }
}

/// Keyboard worker task (bottom-half): drains raw scancodes, decodes them
/// into ASCII characters, and wakes any tasks waiting for input.
///
/// This task is spawned once during boot and runs for the lifetime of the
/// kernel.
pub extern "C" fn keyboard_worker_task() -> ! {
    // Obtain our own task ID (the scheduler must be running at this point).
    let task_id = loop {
        if let Some(id) = scheduler::current_task_id() {
            break id;
        }
        scheduler::yield_now();
    };

    loop {
        // Drain all available raw scancodes.
        let mut decoded_any = false;
        while let Some(code) = KEYBOARD.raw.pop() {
            handle_scancode(code);
            decoded_any = true;
        }

        // If we decoded at least one character, wake consumer tasks.
        if decoded_any && !KEYBOARD.buffer.is_empty() {
            INPUT_WAITQUEUE.wake_all();
        }

        // Sleep until the IRQ handler enqueues the next scancode.
        // `sleep_if` checks `is_empty()` with interrupts disabled so an IRQ
        // that fires between the pop-loop above and this point does not cause
        // a lost wakeup.
        if RAW_WAITQUEUE.sleep_if(task_id, || KEYBOARD.raw.is_empty()) {
            scheduler::yield_now();
        }
    }
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
///
/// Each character is obtained via [`read_char_blocking`], which puts the
/// calling task to sleep until the keyboard worker has decoded input.
pub fn read_line(screen: &mut Screen, buf: &mut [u8]) -> usize {
    let mut len = 0;

    loop {
        let ch = read_char_blocking();

        match ch {
            b'\r' | b'\n' => {
                screen.print_char(b'\n');
                break;
            }
            0x08 => {
                if len > 0 {
                    len -= 1;
                    screen.print_char(0x08);
                }
            }
            _ => {
                if len < buf.len() {
                    buf[len] = ch;
                    len += 1;
                    screen.print_char(ch);
                }
            }
        }
    }

    len
}
