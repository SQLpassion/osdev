# KAOS Text User Interface (TUI) Framework Architecture Guide

This document provides a comprehensive, deep-dive technical reference for the Text User Interface (TUI) framework built into the KAOS kernel. Running entirely in Ring 0 under `#![no_std]` and backed by the kernel's heap allocator, the TUI provides a responsive, dynamically allocated, multi-tab windowing experience directly on bare-metal VGA text-mode hardware.

---

## 1. Architectural Overview & Hardware Foundations

Unlike modern user-space TUI frameworks (such as `ncurses` or `ratatui`) which run inside a virtual terminal emulator and communicate via standard streams (stdin/stdout) and terminal escape codes, the KAOS TUI operates directly on bare-metal hardware. It interfaces directly with the Video Graphics Array (VGA) controller and the PS/2 keyboard controller via Memory-Mapped I/O (MMIO) and Port I/O (PIO).

### 1.1 TUI Infrastructure Dataflow

```text
               ┌─────────────────────────────────────────┐
               │          TuiApp Event Loop              │◄────────────────────────┐
               └────────────────────┬────────────────────┘                         │
                                    │                                              │
                                    │ draws                                        │
                                    ▼                                              │
               ┌─────────────────────────────────────────┐                         │
               │        Active Tab Content Widgets       │                         │
               │ (Label, TextBox, Gauge, Table, List)    │                         │
               └────────────────────┬────────────────────┘                         │
                                    │                                              │
                                    │ calls draw_at / fill_rect                    │
                                    ▼                                              │
               ┌─────────────────────────────────────────┐                         │ reads key
               │      Screen Driver (SpinLock Guard)     │                         │ event
               └────────────────────┬────────────────────┘                         │
                                    │                                              │
                                    │ writes (volatile MMIO)                       │
                                    ▼                                              │
               ┌─────────────────────────────────────────┐                         │
               │   VGA Framebuffer [0xFFFF8000000B8000]  │                         │
               └────────────────────┬────────────────────┘                         │
                                    │                                              │
                                    │ hardware displays                            │
                                    ▼                                              │
               ┌─────────────────────────────────────────┐                         │
               │         Physical 80x25 Monitor          │                         │
               └─────────────────────────────────────────┘                         │
                                                                                   │
                                                                                   │
               ┌─────────────────────────────────────────┐                         │
               │      PS/2 Keyboard ISR (IRQ 1)          │                         │
               └────────────────────┬────────────────────┘                         │
                                    │                                              │
                                    │ pushes raw scancodes                         │
                                    ▼                                              │
               ┌─────────────────────────────────────────┐                         │
               │          Raw Scancode Buffer            │                         │
               └────────────────────┬────────────────────┘                         │
                                    │                                              │
                                    │ processed by worker                          │
                                    ▼                                              │
               ┌─────────────────────────────────────────┐                         │
               │          Extended Key Buffer            │─────────────────────────┘
               │          (Decoded Key Events)           │
               └─────────────────────────────────────────┘
```

### 1.1 VGA Memory-Mapped I/O (MMIO)
In standard VGA text-mode 3 (80 columns by 25 rows, 16 colors), the video controller maps its character frame memory to the physical address range `0x000B8000` to `0x000BFFFF`. 

During boot, the KAOS page tables map this physical range into the higher-half virtual address space:
* **VGA Base Physical Address**: `0x000B8000`
* **VGA Base Virtual Address**: `0xFFFF8000000B8000`
* **Layout**: 25 rows by 80 columns of 16-bit character cells.
* **Cell Footprint**: Each cell requires 2 bytes (16 bits) of space.
* **Total Buffer Footprint**: $80 \times 25 \times 2 \text{ bytes} = 4000 \text{ bytes}$.

### 1.2 Character Cell Memory Layout
Each 16-bit cell in the VGA buffer represents a character and its visual styling attributes:

```rust
#[derive(Copy, Clone, Debug)]
#[repr(C)]
pub struct VgaChar {
    pub character: u8,
    pub attribute: u8,
}
```

The `attribute` byte is structured to determine the foreground color (lower nibble) and background color (upper nibble):

```
 Bit:   7     6     5     4      3     2     1     0
      +-----+-----+-----+-----+  +-----+-----+-----+-----+
      |  B  |  R  |  G  |  B  |  |  I  |  R  |  G  |  B  |
      +-----+-----+-----+-----+  +-----+-----+-----+-----+
      \___________ ___________/  \___________ ___________/
                  V                          V
           Background Color           Foreground Color
```

* **B (Blink / Background Intensity)**: By default, bit 7 of the attribute byte toggles blinking. 
* **I (Foreground Intensity / Brightness)**: Bit 3 toggles brightness for the foreground color.
* **R, G, B**: Red, Green, and Blue color channels.

#### VGA Color Palette
Combining these channels yields 16 colors:

| Value (Hex) | Binary | Color | Bright/Intensity Variant (Hex) | Bright/Intensity Color |
| :---: | :---: | :--- | :---: | :--- |
| `0x0` | `0000` | Black | `0x8` | Dark Gray |
| `0x1` | `0001` | Blue | `0x9` | Light Blue |
| `0x2` | `0010` | Green | `0xA` | Light Green |
| `0x3` | `0011` | Cyan | `0xB` | Light Cyan |
| `0x4` | `1000` | Red | `0xC` | Light Red |
| `0x5` | `1001` | Magenta | `0xD` | Pink |
| `0x6` | `0110` | Brown | `0xE` | Yellow |
| `0x7` | `0111` | Light Gray | `0xF` | White |

> [!IMPORTANT]
> To enable all 16 background colors (allowing vibrant UI designs instead of blinking text), the KAOS kernel disables the hardware blinking feature by talking to the VGA Attribute Controller.

### 1.3 Disabling Hardware Blinking
To disable blinking, the CPU writes to the VGA Attribute Controller (ATC) registers. The ATC is indexed via port `0x3C0` and behaves as a state machine toggling between index and data modes:

```rust
// Port definitions for VGA Attribute Controller
const ATC_INDEX_WRITE: u16 = 0x3C0;
const ATC_DATA_READ: u16 = 0x3C1;
const STATUS_REGISTER_1: u16 = 0x3DA;

pub unsafe fn disable_blink_mode() {
    let isr1 = crate::arch::port::PortByte::new(STATUS_REGISTER_1);
    let ac_addr = crate::arch::port::PortByte::new(ATC_INDEX_WRITE);
    let ac_data_r = crate::arch::port::PortByte::new(ATC_DATA_READ);

    // Step 1: Reset the ATC state machine to "Index Mode" by reading Status Register 1
    let _ = isr1.read();

    // Step 2: Select Register 0x10 (Mode Control Register) and prevent screen blanking (set bit 5)
    ac_addr.write(0x10 | 0x20);

    // Step 3: Read the current configuration value
    let val = ac_data_r.read();

    // Step 4: Reset the ATC state machine again
    let _ = isr1.read();

    // Step 5: Write the updated register value back.
    // Clear Bit 3 (Blink Enable) to convert Bit 7 of cell attributes into background intensity.
    ac_addr.write(0x10 | 0x20);
    ac_addr.write(val & !0x08);

    // Step 6: Enable screen output
    ac_addr.write(0x20);
}
```

### 1.4 Code Page 437 (CP437) Fills and Borders
Because the VGA card is initialized to Code Page 437, we can render complex widgets using box-drawing bytes:
* **Single Borders**: Top-Left `0xDA` (`┌`), Top-Right `0xBF` (`┐`), Bottom-Left `0xC0` (`└`), Bottom-Right `0xD9` (`┘`), Horizontal `0xC4` (`─`), Vertical `0xB3` (`│`).
* **Double Borders**: Top-Left `0xC9` (`╔`), Top-Right `0xBB` (`╗`), Bottom-Left `0xC8` (`╚`), Bottom-Right `0xBC` (`╝`), Horizontal `0xCD` (`═`), Vertical `0xBA` (`║`).
* **Blocks**: Block Fills (`0xDB` for solid `█`, `0xB2` for dark shade `▓`, `0xB1` for medium shade `▒`, `0xB0` for light shade `░`).
* **Arrows**: Up `0x18` (`↑`), Down `0x19` (`↓`), Right `0x1A` (`→`), Left `0x1B` (`←`).

---

## 2. Core Rendering Driver

At the center of rendering is the `Screen` driver. It coordinates layout placement, locking, and raw volatile buffer operations.

### 2.1 The Screen Struct
The screen driver tracks cursor coordinates, dimensions, and active text attributes:

```rust
pub struct Screen {
    row: usize,
    col: usize,
    foreground: Color,
    background: Color,
    num_cols: usize,
    num_rows: usize,
}
```

Every access to the global VGA frame requires a volatile raw pointer access. This guarantees that the compiler will not elide consecutive writes to the same video memory cell (which could happen if it incorrectly assumed the operations were redundant).

```rust
impl Screen {
    /// Computes the raw memory-mapped address of a specific row and column.
    #[inline]
    fn vga_ptr(&self, row: usize, col: usize) -> *mut VgaChar {
        let offset = row * self.num_cols + col;
        (VGA_BUFFER + offset * 2) as *mut VgaChar
    }

    /// Set an individual character cell using raw volatile writes.
    pub fn draw_char_at(&self, row: usize, col: usize, byte: u8, fg: Color, bg: Color) {
        if row >= self.num_rows || col >= self.num_cols {
            return;
        }
        let attr = ((bg as u8) << 4) | (fg as u8);
        let cell = VgaChar { character: byte, attribute: attr };
        
        // SAFETY:
        // - Coordinate checks ensure the calculated offset remains within the 4000-byte frame.
        // - Volatile writes ensure hardware registers reflect changes instantly.
        unsafe {
            core::ptr::write_volatile(self.vga_ptr(row, col), cell);
        }
    }
}
```

### 2.2 Global Locking vs. Panic Paths
Multi-core SMP systems present a concurrency issue: if two CPU cores write to the VGA frame concurrently, it causes race conditions and visual corruption. To prevent this, the primary `Screen` driver is wrapped in a `SpinLock`.

However, if the kernel panics while a core holds this lock, any attempt to acquire the lock to print the panic traceback would lead to a system deadlock. To prevent this, the kernel implements a separate, lock-free `PanicScreenWriter` that writes directly to the framebuffer, guaranteeing that crash telemetry is always visible:

```rust
pub struct PanicScreenWriter {
    row: usize,
    col: usize,
    attribute: u8,
}

impl PanicScreenWriter {
    pub const fn new(foreground: Color, background: Color) -> Self {
        Self {
            row: 0,
            col: 0,
            attribute: ((background as u8) << 4) | (foreground as u8),
        }
    }
    
    // Writes a byte without locking, ensuring panic logs are written immediately.
    pub fn put_byte(&mut self, byte: u8) {
        // ... raw pointer writes directly to VGA_BUFFER bypassing SpinLock
    }
}
```

### 2.3 Hardware Cursor Controls
The hardware cursor is controlled by writing index values to the CRT Controller (CRTC). This is done via I/O Ports `0x3D4` and `0x3D5`:

```rust
pub fn update_cursor(&self) {
    let position = (self.row * self.num_cols + self.col) as u16;
    unsafe {
        // Send index 0x0F (Cursor Location Low Register) to CRT Controller Index Port
        PortByte::new(0x3D4).write(0x0F);
        // Write lower 8 bits of position to Data Port
        PortByte::new(0x3D5).write((position & 0xFF) as u8);

        // Send index 0x0E (Cursor Location High Register)
        PortByte::new(0x3D4).write(0x0E);
        // Write upper 8 bits of position to Data Port
        PortByte::new(0x3D5).write(((position >> 8) & 0xFF) as u8);
    }
}
```

---

## 3. Keyboard Pipeline & Asynchronous Event Loop

To enable fluid user interaction without polling (which consumes 100% CPU time), the KAOS TUI relies on cooperative scheduling, waitqueues, and decoded keyboard interrupts.

### 3.1 SPSC Lock-Free Ring Buffers
The keyboard driver manages key queues using a Single-Producer Single-Consumer (SPSC) ring buffer layout. Since the producer (the keyboard worker) and the consumer (the TUI app) operate on different task threads, this SPSC structure avoids heavy lock contention by utilizing atomic indices:

```rust
pub struct RingBuffer<const N: usize> {
    buf: UnsafeCell<[u8; N]>,
    head_producer: AtomicUsize,
    tail_consumer: AtomicUsize,
}

impl<const N: usize> RingBuffer<N> {
    pub fn push(&self, value: u8) -> bool {
        let head = self.head_producer.load(Ordering::Relaxed);
        let next = (head + 1) % N;
        let tail = self.tail_consumer.load(Ordering::Acquire);

        if next == tail {
            return false; // Buffer is full
        }

        // SAFETY: Only the producer modifies the slot at index 'head'
        unsafe {
            (*self.buf.get())[head] = value;
        }
        self.head_producer.store(next, Ordering::Release);
        true
    }

    pub fn pop(&self) -> Option<u8> {
        let mut tail = self.tail_consumer.load(Ordering::Relaxed);
        loop {
            let head = self.head_producer.load(Ordering::Acquire);
            if tail == head {
                return None; // Buffer is empty
            }
            
            // Speculatively read the byte
            let val = unsafe { (*self.buf.get())[tail] };
            let next = (tail + 1) % N;

            // Attempt CAS operation to support multiple concurrent consumers safely
            match self.tail_consumer.compare_exchange_weak(
                tail, next, Ordering::Acquire, Ordering::Relaxed
            ) {
                Ok(_) => return Some(val),
                Err(actual) => tail = actual, // CAS failed, retry
            }
        }
    }
}
```

### 3.2 Dual Input Models Rationale and the "Bleed" Issue
The keyboard driver exposes two input models because the kernel serves two distinct classes of input consumers:

1. **Legacy Character Stream Buffer (`KEYBOARD.buffer`)**:
   * **Exposed API**: `read_char()` / `read_char_blocking()`
   * **Data Type**: Raw `u8` ASCII character bytes.
   * **Designed For**: Line-oriented command entry (such as the standard **REPL shell** or serial terminal redirection).
   * **Rationale**: Simple shell prompts only need standard printable characters, backspaces, and newlines. They do not want the complexity or memory footprint of matching over nested `Key` enum types. Leaving this interface as raw `u8` characters keeps command-parsing loops simple.
   
2. **Structured Key Event Buffer (`KEYBOARD.key_buffer`)**:
   * **Exposed API**: `read_key()` / `read_key_blocking()`
   * **Data Type**: A structured `Key` enum wrapping printable characters alongside non-character controller keys (such as `ArrowLeft`, `ArrowRight`, `ArrowUp`, `ArrowDown`, `Escape`, `Backspace`, and `F1`-`F12`).
   * **Designed For**: Interactive UI layouts like the **TUI application** or menu systems.
   * **Rationale**: Complex visual applications rely heavily on navigation inputs that have no ASCII representation. The structured `Key` enum allows the keyboard driver's decode routine to package scancode sequences out-of-band so that applications do not have to parse multi-byte ANSI escape codes manually.

#### The "Bleed" Issue
When you press a key such as `'q'` to exit the TUI, the driver enqueues the input in both buffers to satisfy both potential readers. Because the TUI runs on `read_key_blocking`, it reads the event from `KEYBOARD.key_buffer` and terminates. The matching ASCII byte in `KEYBOARD.buffer` is never consumed by the TUI. When control returns to the REPL shell, the shell reads the legacy buffer, receives the stale `'q'`, and prints it on the command prompt.

To resolve this context leakage, the TUI flushes both queues using `clear_buffers()` on TUI startup and teardown:

```rust
pub fn clear_buffers() {
    KEYBOARD.buffer.clear();
    KEYBOARD.key_buffer.clear();
}
```

### 3.3 The Asynchronous Event Loop
The `TuiApp` loop operates cooperatively. It renders the widgets, then yields CPU execution if no key events are pending:

```rust
pub fn run(&mut self) {
    self.draw_all(); // Draw initial frame

    loop {
        // read_key_blocking() sleeps the task on INPUT_WAITQUEUE if key_buffer is empty
        let key = keyboard::read_key_blocking();

        match key {
            Key::ArrowLeft => {
                self.tabs.select_prev();
                self.clear_content_area();
            }
            Key::ArrowRight => {
                self.tabs.select_next();
                self.clear_content_area();
            }
            Key::ArrowUp => self.navigate_up(),
            Key::ArrowDown => self.navigate_down(),
            Key::Escape | Key::Char(b'q') | Key::Char(b'Q') => break, // Exit loop
            _ => {}
        }
        self.draw_all(); // Redraw with updated widget states
    }
}
```

---

## 4. Widget Tree Design & Layout System

The TUI framework utilizes dynamic memory allocations via the registered global heap allocator (`GLOBAL_ALLOCATOR`). This allows widgets to be configured with dynamic collection structures (specifically `alloc::vec::Vec`) rather than being constrained by fixed-capacity arrays or compile-time limits.

### 4.1 Memory Layout of Dynamically Allocated Widgets
By using `Vec`, widgets do not need to pre-allocate maximum buffers. Instead, they dynamically expand to fit the size of their data.

For example, the `TextBox` widget uses a `Vec` to store an arbitrary number of lines:

```rust
extern crate alloc;

use alloc::vec::Vec;

pub struct TextBox {
    row: usize,
    col: usize,
    width: usize,
    height: usize,
    lines: Vec<&'static str>,
    fg: Color,
    bg: Color,
    border_fg: Color,
}
```

Similarly, the `Tabs` selector uses `Vec<&'static str>` for dynamic labels, the `List` widget uses `Vec<&'static str>` for dynamic entries, and the `Table` widget uses `Vec<Vec<&'static str>>` to represent dynamically sized 2D data grids.

When instantiating the widgets, they are created on the stack of the running task, while their underlying collections dynamically allocate memory from the kernel heap.

### 4.2 Drawing Primitives and Clipping Rules
Every widget receives layout bounds (`row`, `col`, `width`, `height`) during construction. To ensure widgets do not overwrite adjacent borders, all strings are clipped at render time:

```rust
// Truncates text exceeding columns limit
let visible_width = self.width.saturating_sub(2);
let clipped_text = if line.len() > visible_width {
    &line[..visible_width]
} else {
    line
};
screen.draw_at(start_row, start_col, clipped_text, self.fg, self.bg);
```

### 4.3 Widget Portfolio Details
The framework provides several core components:

* **Label**: A lightweight single-line text block used for headers.
* **TextBox**: A static text panel surrounded by a single-line border. Used for help text or informational displays.
* **Gauge**: A horizontal bar containing descriptive text, a percentage label, and a color-filled indicator segment.
* **ProgressBar**: A raw progress meter formatted as `[████▒▒▒▒    ]`.
* **Table**: A selectable data grid featuring headers, column separators, and scrollable row bounds.
* **List**: A vertical collection of items supporting selection scrolling.
* **Tabs**: A horizontal tab-header strip that manages the active view state.

---

## 5. Blueprint for Implementing a TUI in Your Own Kernel

If you are developing a new OS kernel and want to implement a custom TUI, follow this structural blueprint:

```
[ Step 1: MMIO Setup ] ---> [ Step 2: Keyboard Driver ] ---> [ Step 3: WaitQueue Scheduler ] ---> [ Step 4: TUI Loop ]
```

### Step 1: Set Up Virtual Memory Mapping
Add a page entry for the VGA MMIO block in your kernel initialization assembly or paging setup. Ensure that this page is mapped as a kernel-only read/write page, and set the Cache Disable (CD) or Write-Through (WT) bits in the page tables to prevent CPU cache incoherency.

### Step 2: Write a Keyboard Interrupt Decoder
Map the keyboard interrupt vector (IRQ 1) in your Interrupt Descriptor Table (IDT). In the ISR handler, read the raw scancodes from port `0x60`, determine key press/release states, and write them to an input queue.

### Step 3: Implement WaitQueue / Scheduler Yielding
Ensure your kernel scheduler supports thread sleep states. When an application requests keyboard input, search the queue. If it is empty, add the calling thread ID to a `WaitQueue` and call `yield_now()` to transition the task's state from `Running` to `Blocked`, preventing CPU core stalling.

### Step 4: Implement drawing helpers
Provide basic drawing functions (`draw_char_at`, `draw_str_at`) using direct volatile writes to the mapped virtual address of the VGA buffer.


