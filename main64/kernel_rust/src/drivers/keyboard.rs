//! PS/2 keyboard driver (Rust port of the C keyboard driver)
//!
//! Handles scan code processing and stores decoded input in a ring buffer.
//! Extended PS/2 scancodes (0xE0 prefix) are decoded into the `Key` enum so
//! that TUI consumers can react to arrow keys, function keys, and similar.

use crate::arch::port::PortByte;
use crate::scheduler;
use crate::sync::ringbuffer::RingBuffer;
use crate::sync::singlewaitqueue::SingleWaitQueue;
use crate::sync::spinlock::SpinLock;
use crate::sync::waitqueue::WaitQueue;
use crate::sync::waitqueue_adapter;

/// A fully-decoded keyboard event.
///
/// `Char(byte)` carries a printable ASCII byte; all other variants represent
/// special keys that have no printable equivalent but are meaningful for TUI
/// navigation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Key {
    /// Printable ASCII character (includes Enter/Backspace for symmetry with
    /// the existing `read_char` interface).
    Char(u8),
    /// Escape key (0x1B).
    Escape,
    /// Backspace key.
    Backspace,
    /// Enter / Return key.
    Enter,
    /// ↑ arrow key.
    ArrowUp,
    /// ↓ arrow key.
    ArrowDown,
    /// ← arrow key.
    ArrowLeft,
    /// → arrow key.
    ArrowRight,
    /// Function key F1–F12.
    F(u8),
    /// Any other key whose scancode is not mapped.
    Unknown,
}

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
    // 0x00..=0x10: error, Esc, 1-9, 0, ß(mapped to s), ´(mapped to =), Backspace, Tab, q
    0, 0x1B, b'1', b'2', b'3', b'4', b'5', b'6', b'7', b'8', b'9', b'0', b's', b'=', 0x08, 0, b'q',
    // 0x11..=0x20: w, e, r, t, z, u, i, o, p, ü(mapped to [), +, Enter, LCtrl, a, s, d
    b'w', b'e', b'r', b't', b'z', b'u', b'i', b'o', b'p', b'[', b'+', b'\n', 0, b'a', b's', b'd',
    // 0x21..=0x30: f, g, h, j, k, l, ö(mapped to {), ä(mapped to ~), <, LShift, #, y, x, c, v, b
    b'f', b'g', b'h', b'j', b'k', b'l', b'{', b'~', b'<', 0, b'#', b'y', b'x', b'c', b'v', b'b',
    // 0x31..=0x3F: n, m, ',', '.', -, RShift, Keypad-*, LAlt, Space, CapsLock, F1..F10
    b'n', b'm', b',', b'.', b'-', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0,
    // 0x40..=0x4F: F6..F10, NumLock, ScrollLock, Keypad 7..9, Keypad-, Keypad 4..6, Keypad+
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0x50..=0x58: Keypad 2..3, Keypad-0, Keypad-Del, Alt-SysRq, 0x55, <(ISO key), F11, F12
    0, 0, 0, 0, 0, 0, b'<', 0, 0,
];

/// Upper-case QWERTZ scan code map (printable ASCII only; 0 == ignored)
const SCANCODES_UPPER: [u8; SCANCODE_TABLE_LEN] = [
    // 0x00..=0x10: error, Esc, !"§$%&/()=?, ?(Shift+ß), ´→backtick(Shift+´), Backspace, Tab, Q
    0, 0x1B, b'!', b'"', b'\x00', b'$', b'%', b'&', b'/', b'(', b')', b'=', b'?', b'`', 0x08, 0, b'Q',
    // 0x11..=0x20: W, E, R, T, Z, U, I, O, P, Ü(mapped to ]), *, Enter, LCtrl, A, S, D
    b'W', b'E', b'R', b'T', b'Z', b'U', b'I', b'O', b'P', b']', b'*', b'\n', 0, b'A', b'S', b'D',
    // 0x21..=0x30: F, G, H, J, K, L, Ö(mapped to }), Ä(mapped to @), >, LShift, ', Y, X, C, V, B
    b'F', b'G', b'H', b'J', b'K', b'L', b'}', b'@', b'>', 0, b'\\', b'Y', b'X', b'C', b'V', b'B',
    // 0x31..=0x3F: N, M, ;, :, _, RShift, Keypad-*, LAlt, Space, CapsLock, F1..F10
    b'N', b'M', b';', b':', b'_', 0, b'*', 0, b' ', 0, 0, 0, 0, 0, 0,
    // 0x40..=0x4F: F6..F10, NumLock, ScrollLock, Keypad 7..9, Keypad-, Keypad 4..6, Keypad+
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    // 0x50..=0x58: Keypad 2..3, Keypad-0, Keypad-Del, Alt-SysRq, 0x55, >(ISO key Shift), F11, F12
    0, 0, 0, 0, 0, 0, b'>', 0, 0,
];


#[derive(Debug, Clone, Copy)]
struct KeyboardState {
    shift: bool,
    caps_lock: bool,
    left_ctrl: bool,
    /// Set when the previous scancode was the 0xE0 extended-key prefix.
    extended: bool,
}

impl KeyboardState {
    const fn new() -> Self {
        Self {
            shift: false,
            caps_lock: false,
            left_ctrl: false,
            extended: false,
        }
    }
}

/// Capacity of the decoded `Key` event ring buffer.
const KEY_BUFFER_CAPACITY: usize = 64;

struct Keyboard {
    raw: RingBuffer<RAW_BUFFER_CAPACITY>,
    /// Decoded ASCII character ring buffer (legacy `read_char` interface).
    buffer: RingBuffer<INPUT_BUFFER_CAPACITY>,
    /// Decoded `Key` event ring buffer (TUI / extended-key interface).
    key_buffer: RingBuffer<KEY_BUFFER_CAPACITY>,
}

impl Keyboard {
    const fn new() -> Self {
        Self {
            raw: RingBuffer::new(),
            buffer: RingBuffer::new(),
            key_buffer: RingBuffer::new(),
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
    KEYBOARD.key_buffer.clear();
    let mut state = KEYBOARD_STATE.lock();
    *state = KeyboardState::new();
}

/// Clear the input and key buffers.
///
/// This drains any stale keys or characters to prevent input bleeding between
/// different modes (e.g. from TUI to REPL).
pub fn clear_buffers() {
    KEYBOARD.buffer.clear();
    KEYBOARD.key_buffer.clear();
}


/// Handle IRQ1 (keyboard) top half: enqueue raw scancode and wake the
/// keyboard worker task so it can decode the scancode into ASCII.
pub fn handle_irq() {
    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Reading keyboard controller status uses the documented PS/2 status port.
    // - Port access is valid in ring 0 IRQ context.
    let status = unsafe { PortByte::new(KYBRD_CTRL_STATS_REG).read() };
    if (status & KYBRD_CTRL_STATS_MASK_OUT_BUF) == 0 {
        return;
    }

    // SAFETY:
    // - This requires `unsafe` because hardware port I/O is inherently outside Rust's memory-safety guarantees.
    // - Output-buffer-full bit was checked above.
    // - Reading data port consumes the pending scancode.
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

/// Read a decoded `Key` event if one is available; returns `None` when the
/// key-event buffer is empty.
pub fn read_key() -> Option<Key> {
    // The key buffer stores the raw byte representation of the Key discriminant
    // packed by `encode_key` / decoded by `decode_key`.
    KEYBOARD.key_buffer.pop().map(decode_key)
}

/// Read a blocking key event and return it as an encoded byte for the `ReadKey` syscall.
///
/// Encoding:
/// - `0x01`–`0x7F` → printable ASCII character
/// - `0x80`        → Escape
/// - `0x81`        → Backspace
/// - `0x82`        → Enter
/// - `0x83`        → ArrowUp
/// - `0x84`        → ArrowDown
/// - `0x85`        → ArrowLeft
/// - `0x86`        → ArrowRight
/// - `0x90`–`0x9B` → F(1)–F(12)
pub fn read_key_blocking_encoded() -> u8 {
    encode_key(read_key_blocking())
}

/// Block the calling task until a `Key` event is available.
///
/// Mirrors `read_char_blocking` but drains from the extended key-event buffer
/// so TUI consumers receive arrow keys, function keys, etc.
pub fn read_key_blocking() -> Key {
    loop {
        if let Some(key) = read_key() {
            return key;
        }

        let task_id =
            scheduler::current_task_id().expect("read_key_blocking called outside scheduled task");

        // Re-use the same INPUT_WAITQUEUE: the keyboard worker wakes it
        // whenever any decoded input (char or key) becomes available.
        if waitqueue_adapter::sleep_if_multi(&INPUT_WAITQUEUE, task_id, || {
            KEYBOARD.key_buffer.is_empty()
        }).should_yield() {
            scheduler::yield_now();
        }
    }
}

// ---------------------------------------------------------------------------
// Key encoding helpers
// ---------------------------------------------------------------------------
// The `RingBuffer` stores `u8` values.  We pack the `Key` enum into a single
// byte using a simple tag scheme:
//   0x00        → Unknown
//   0x01–0x7F  → Char(byte)  (byte stored directly; 0x00 is not a valid char)
//   0x80        → Escape
//   0x81        → Backspace
//   0x82        → Enter
//   0x83        → ArrowUp
//   0x84        → ArrowDown
//   0x85        → ArrowLeft
//   0x86        → ArrowRight
//   0x90–0x9B  → F(1)–F(12)  (0x90 + n - 1)
// ---------------------------------------------------------------------------

pub fn encode_key(key: Key) -> u8 {
    match key {
        Key::Unknown        => 0x00,
        Key::Char(b)        => b,
        Key::Escape         => 0x80,
        Key::Backspace      => 0x81,
        Key::Enter          => 0x82,
        Key::ArrowUp        => 0x83,
        Key::ArrowDown      => 0x84,
        Key::ArrowLeft      => 0x85,
        Key::ArrowRight     => 0x86,
        Key::F(n)           => 0x8F_u8.saturating_add(n),
    }
}

fn decode_key(byte: u8) -> Key {
    match byte {
        0x00        => Key::Unknown,
        0x80        => Key::Escape,
        0x81        => Key::Backspace,
        0x82        => Key::Enter,
        0x83        => Key::ArrowUp,
        0x84        => Key::ArrowDown,
        0x85        => Key::ArrowLeft,
        0x86        => Key::ArrowRight,
        0x90..=0x9B => Key::F(byte - 0x8F),
        b           => Key::Char(b),
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

    // Wake consumers when either the ASCII buffer or the Key buffer has data,
    // so both `read_char_blocking` and `read_key_blocking` callers are woken.
    if processed_any
        && (!KEYBOARD.buffer.is_empty() || !KEYBOARD.key_buffer.is_empty())
    {
        waitqueue_adapter::wake_all_multi(&INPUT_WAITQUEUE);
    }

    processed_any
}

fn handle_scancode(state: &mut KeyboardState, code: u8) {
    // 0xE0 marks the start of a two-byte extended scancode sequence.
    // Record the flag and consume the prefix without producing any key event.
    if code == 0xE0 {
        state.extended = true;
        return;
    }

    if (code & 0x80) != 0 {
        // Break code: key released.
        handle_break(state, code & 0x7f);
    } else {
        // Make code: key pressed.
        handle_make(state, code);
    }

    // Clear the extended flag after the second byte has been processed,
    // regardless of whether it was a break or make code.
    state.extended = false;
}

fn handle_break(state: &mut KeyboardState, code: u8) {
    // Only non-extended modifier releases are meaningful here.
    if !state.extended {
        match code {
            0x1d => state.left_ctrl = false,
            0x2a | 0x36 => state.shift = false,
            _ => {}
        }
    }
}

fn handle_make(state: &mut KeyboardState, code: u8) {
    // Extended make codes (preceded by 0xE0) map to special Key variants.
    if state.extended {
        let key = match code {
            0x48 => Key::ArrowUp,
            0x50 => Key::ArrowDown,
            0x4B => Key::ArrowLeft,
            0x4D => Key::ArrowRight,
            _    => Key::Unknown,
        };
        // Push the encoded key into the extended key-event buffer.
        let _ = KEYBOARD.key_buffer.push(encode_key(key));
        return;
    }

    // Non-extended make codes: handle modifiers and function keys first.
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
        // F1–F10
        0x3B => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(1)));  return; }
        0x3C => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(2)));  return; }
        0x3D => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(3)));  return; }
        0x3E => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(4)));  return; }
        0x3F => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(5)));  return; }
        0x40 => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(6)));  return; }
        0x41 => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(7)));  return; }
        0x42 => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(8)));  return; }
        0x43 => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(9)));  return; }
        0x44 => { let _ = KEYBOARD.key_buffer.push(encode_key(Key::F(10))); return; }
        _ => {}
    }

    // Translate the scancode to an ASCII byte using the layout tables.
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

    let Some(&ascii) = table.get(code as usize) else {
        return;
    };

    if ascii == 0 {
        return;
    }

    // Push into the legacy ASCII buffer (for `read_char` consumers).
    let _ = KEYBOARD.buffer.push(ascii);

    // Also push into the extended Key buffer so `read_key` consumers receive
    // printable characters alongside special keys without a separate poll.
    let key = match ascii {
        0x1B => Key::Escape,
        0x08 => Key::Backspace,
        b'\n' | b'\r' => Key::Enter,
        b => Key::Char(b),
    };
    let _ = KEYBOARD.key_buffer.push(encode_key(key));
}

fn is_alpha(code: u8) -> bool {
    matches!(
        code,
        0x10..=0x19 // Q..P
            | 0x1e..=0x26 // A..L
            | 0x2c..=0x32 // Z..M
    )
}
