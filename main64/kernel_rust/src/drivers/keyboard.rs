//! PS/2 keyboard driver (Rust port of the C keyboard driver)
//!
//! Handles scan code processing and stores decoded input in a ring buffer.

use crate::arch::port::PortByte;
use crate::scheduler;
use crate::sync::ringbuffer::RingBuffer;
use crate::sync::singlewaitqueue::SingleWaitQueue;
use crate::sync::spinlock::SpinLock;
use crate::sync::waitqueue::WaitQueue;
use crate::sync::waitqueue_adapter;

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
    0, 0x1B, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b's', b'=', 0x08, 0, b'q',
    b'w', b'e', b'r', b't', b'z', b'u', b'i', b'o', b'p', b'[', b'+', b'\n', 0, b'a', b's', b'd',
    b'f', b'g', b'h', b'j', b'k', b'l', b'{', b'~', b'<', 0, b'#', b'y', b'x', b'c', b'v', b'b',
    b'n', b'm', b',', b'.', b'-', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
];

/// Upper-case QWERTZ scan code map (printable ASCII only; 0 == ignored)
const SCANCODES_UPPER: [u8; SCANCODE_TABLE_LEN] = [
    0, 0x1B, b'!', b'"', b'0', b'$', b'%', b'&', b'/', b'(', b')', b'=', b'?', b'`', 0x08, 0, b'Q',
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

impl KeyboardState {
    const fn new() -> Self {
        Self {
            shift: false,
            caps_lock: false,
            left_ctrl: false,
        }
    }
}

struct Keyboard {
    raw: RingBuffer<RAW_BUFFER_CAPACITY>,
    buffer: RingBuffer<INPUT_BUFFER_CAPACITY>,
}

impl Keyboard {
    const fn new() -> Self {
        Self {
            raw: RingBuffer::new(),
            buffer: RingBuffer::new(),
        }
    }
}

// Static assert: `Keyboard` must be `Sync` because it is used as a `static`.
// All fields use lock-free atomics internally (`RingBuffer` is `Sync`).
// If a non-`Sync` field is added in the future, this line will produce a
// compile error rather than silently introducing unsoundness.
const _: () = {
    const fn assert_sync<T: Sync>() {}
    assert_sync::<Keyboard>();
};

static KEYBOARD: Keyboard = Keyboard::new();
static KEYBOARD_STATE: SpinLock<KeyboardState> = SpinLock::new(KeyboardState::new());

/// Wakes the keyboard worker task when raw scancodes are available.
static RAW_WAITQUEUE: SingleWaitQueue = SingleWaitQueue::new();

/// Wakes consumer tasks when decoded characters are available.
static INPUT_WAITQUEUE: WaitQueue = WaitQueue::new();

/// Initialize the keyboard driver state.
pub fn init() {
    KEYBOARD.raw.clear();
    KEYBOARD.buffer.clear();
    let mut state = KEYBOARD_STATE.lock();
    *state = KeyboardState::new();
}

/// Handle IRQ1 (keyboard) top half: enqueue raw scancode and wake the
/// keyboard worker task so it can decode the scancode into ASCII.
pub fn handle_irq() {
    let status = unsafe { PortByte::new(KYBRD_CTRL_STATS_REG).read() };
    if (status & KYBRD_CTRL_STATS_MASK_OUT_BUF) == 0 {
        return;
    }

    let code = unsafe { PortByte::new(KYBRD_ENC_INPUT_BUF).read() };
    enqueue_raw_scancode(code);
}

/// Enqueue a raw keyboard scancode and wake the keyboard worker task.
///
/// This is used by the IRQ top-half and by integration tests that inject
/// synthetic scancodes to exercise the full decode pipeline.
pub fn enqueue_raw_scancode(code: u8) {
    let _ = KEYBOARD.raw.push(code);
    waitqueue_adapter::wake_all_single(&RAW_WAITQUEUE);
}

/// Read a decoded character if available; returns None when the buffer is empty.
pub fn read_char() -> Option<u8> {
    KEYBOARD.buffer.pop()
}

/// Block the calling task until a decoded character is available.
///
/// Uses the scheduler adapter over the input wait queue with an emptiness
/// check under disabled interrupts to prevent lost wakeups.  On wakeup the
/// task re-checks the buffer; if another consumer grabbed the character first
/// (thundering herd), it sleeps again.
pub fn read_char_blocking() -> u8 {
    loop {
        if let Some(ch) = read_char() {
            return ch;
        }

        let task_id =
            scheduler::current_task_id().expect("read_char_blocking called outside scheduled task");

        if waitqueue_adapter::sleep_if_multi(&INPUT_WAITQUEUE, task_id, || {
            KEYBOARD.buffer.is_empty()
        }).should_yield() {
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
        process_pending_scancodes();

        // Sleep until the IRQ handler enqueues the next scancode.
        // `sleep_if` checks `is_empty()` with interrupts disabled so an IRQ
        // that fires between the pop-loop above and this point does not cause
        // a lost wakeup.
        if waitqueue_adapter::sleep_if_single(&RAW_WAITQUEUE, task_id, || KEYBOARD.raw.is_empty())
            .should_yield()
        {
            scheduler::yield_now();
        }
    }
}

/// Execute one keyboard bottom-half iteration: drain raw scancodes, decode
/// into characters, and wake waiting consumers when input became available.
///
/// Returns `true` when at least one raw scancode was processed.
pub fn process_pending_scancodes() -> bool {
    let mut processed_any = false;

    while let Some(code) = KEYBOARD.raw.pop() {
        let mut state = KEYBOARD_STATE.lock();
        handle_scancode(&mut state, code);

        // Lock is released here at end of each iteration, keeping the
        // interrupt-disabled window short.
        drop(state);

        processed_any = true;
    }

    if processed_any && !KEYBOARD.buffer.is_empty() {
        waitqueue_adapter::wake_all_multi(&INPUT_WAITQUEUE);
    }

    processed_any
}

fn handle_scancode(state: &mut KeyboardState, code: u8) {
    if (code & 0x80) != 0 {
        handle_break(state, code & 0x7f);
    } else {
        handle_make(state, code);
    }
}

fn handle_break(state: &mut KeyboardState, code: u8) {
    match code {
        0x1d => state.left_ctrl = false,
        0x2a | 0x36 => state.shift = false,
        _ => {}
    }
}

fn handle_make(state: &mut KeyboardState, code: u8) {
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
