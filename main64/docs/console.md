# KAOS Rust Kernel: Console Subsystem

This document explains the kernel console subsystem found in `kernel/src/console/`.
It covers the runtime-polymorphic abstraction that lets the kernel print text
identically on a legacy **VGA text-mode** display and on a modern **linear
graphics framebuffer**, the hardware that sits behind each backend, and the
performance techniques (dirty-line tracking, shadow buffers, batched cursor
updates) that make graphics-mode text output usable on real hardware.

A developer who has never touched this code should, after reading, understand
*both* the Rust architecture and the underlying x86 display hardware well enough
to extend or debug it.

---

## 1. The Two Display Worlds

A PC can present text to the user through two fundamentally different hardware
paths, and which one is live is decided at boot time by the bootloader (BIOS vs.
UEFI, see `docs/boot_bios.md` / `docs/boot_uefi.md`).

### 1.1 VGA Text Mode (the legacy path)

In classic VGA text mode the video card does the character rendering for you.
The screen is an 80×25 grid of *character cells*. Each cell is **2 bytes** of
memory-mapped I/O:

```
 byte 0: ASCII / CP437 character code
 byte 1: attribute = (background_color << 4) | foreground_color
         bit:  7   6 5 4   3 2 1 0
               |   \___/   \_____/
            blink   bg(3b)   fg(4b)
```

The cell array begins at **physical address `0xB8000`**. The hardware's built-in
character generator looks up each byte in a ROM font and paints the corresponding
8×16 glyph with the colors from the attribute byte — the CPU never touches a
single pixel. Writing the byte pair `('A', 0x0F)` to `0xB8000` instantly shows a
white-on-black "A" in the top-left corner.

Hardware sub-details that matter for this subsystem:

- **Higher-half mapping.** The kernel runs higher-half, so `0xB8000` is reached
  through the virtual alias `0xFFFF_8000_000B_8000` (`VGA_BUFFER` in
  `drivers/screen.rs`).
- **The hardware cursor** (the blinking underline) is a *separate* piece of state
  from the character data. Its position is programmed through the CRT Controller
  (CRTC) index/data port pair `0x3D4`/`0x3D5`, registers `14`/`15` (high/low byte
  of a linear cell offset).
- **The blink bit.** Attribute bit 7 defaults to meaning "blink this cell." That
  steals the high bit of the background color field, so backgrounds are limited to
  colors 0–7 unless blink mode is turned off via the Attribute Controller (see
  §6.3).

### 1.2 Linear Framebuffer (the modern path)

On a UEFI boot (GOP) or a BIOS VBE graphics mode there is **no character
generator and no `0xB8000` text buffer**. The firmware hands the kernel a flat
array of *pixels* — a linear framebuffer — and the CPU must rasterize every glyph
itself. The geometry is described by `FramebufferInfo` (`boot_info.rs`):

| Field                 | Meaning |
|-----------------------|---------|
| `base_address`        | Physical base of the pixel array (identity-mapped after `map_framebuffer`). |
| `width` / `height`    | Visible resolution in pixels. |
| `pixels_per_scanline` | **Stride**: pixels from the start of one row to the next. May exceed `width` because of hardware row-padding/alignment. |
| `pixel_format`        | Channel order — `Rgb` (`0x00BBGGRR`) or `Bgr` (`0x00RRGGBB`). |

Each pixel is a **32-bit word**. The address of pixel `(x, y)` is:

```
pixel_addr = base_address + (y * pixels_per_scanline + x) * 4
```

Note the use of `pixels_per_scanline` (the stride), **not** `width`. Using
`width` here is the classic framebuffer bug that produces a skewed, diagonally
sheared image whenever the hardware pads its rows.

To show text on this device the kernel needs its own font and its own glyph
rasterizer — which is exactly what `framebuffer.rs` + `font_basic.rs` provide.

### 1.3 The design problem

The rest of the kernel (logger, panic handler, PMM/heap diagnostics, the
user-space shell via syscalls) wants to "print a string" without caring which of
these two worlds is active. The console subsystem exists to hide that difference
behind one trait, chosen once at boot.

---

## 2. Module Layout

```
kernel/src/console/
├── mod.rs           Re-exports the public surface.
├── interface.rs     The `KernelConsole` trait + global routing (init / with_console).
├── dispatch.rs      `ConsoleImpl` enum that statically dispatches to the active backend.
├── vga.rs           `VgaConsole`  — thin adapter onto drivers/screen.rs (legacy path).
├── framebuffer.rs   `FramebufferConsole` — full software text renderer (modern path).
└── font_basic.rs    8×16 bitmap font: [[u8; 16]; 256], one entry per CP437 byte.
```

The relationship between the pieces:

```
   callers (logger, panic, syscalls, PMM/heap, shell)
                       │
                       │  with_console(|c| ...)
                       ▼
        ┌───────────────────────────────┐
        │  GLOBAL_CONSOLE: SpinLock<    │    interface.rs
        │     Option<ConsoleImpl>>      │
        └───────────────┬───────────────┘
                        │ &mut dyn KernelConsole
                        ▼
        ┌────────────────────────────────┐
        │  enum ConsoleImpl              │   dispatch.rs
        │   ├─ Vga(VgaConsole)           │
        │   └─ Framebuffer(Framebuffer…) │
        └───────┬───────────────┬────────┘
                │               │
     drivers/screen.rs   framebuffer.rs + font_basic.rs
     (0xB8000 MMIO)      (linear VRAM, software rasterizer)
```

---

## 3. The `KernelConsole` Trait (`interface.rs`)

`KernelConsole` is the single abstraction every backend implements. It extends
`core::fmt::Write`, so the standard `write!`/`writeln!` macros work against any
console for free, and it is `Send` so it can live inside a `SpinLock`.

```rust
pub trait KernelConsole: core::fmt::Write + Send {
    fn clear(&mut self);
    fn print_char(&mut self, c: u8);
    fn print_str(&mut self, s: &str);
    fn set_color(&mut self, color: Color);
    fn set_cursor(&mut self, row: usize, col: usize);
    fn get_cursor(&self) -> (usize, usize);
    fn draw_box(&mut self, row, col, width, height, fg, bg);
    fn draw_at(&mut self, row, col, text, fg, bg);
    fn fill_rect(&mut self, row, col, width, height, ch, fg, bg);
    fn draw_char_at(&mut self, row, col, ch, fg, bg);
    fn blit_framebuffer(&mut self, cells: &[u16]);
    fn get_dimensions(&self) -> (usize, usize);
    fn disable_hw_cursor(&mut self);  fn enable_hw_cursor(&mut self);
    fn disable_blink_mode(&mut self); fn enable_blink_mode(&mut self);
}
```

The methods fall into three groups:

1. **Stream output** — `print_char`, `print_str`, `write_str`, `clear`,
   `set_color`. These advance a logical cursor and scroll when the bottom is
   reached. This is what the logger and `writeln!` use.
2. **Absolute / TUI drawing** — `draw_at`, `draw_char_at`, `draw_box`,
   `fill_rect`, `blit_framebuffer`. These write to explicit `(row, col)`
   coordinates, never scroll, and never move the stream cursor. The user-space
   TUI (`docs/tui.md`) is built entirely on these plus `blit_framebuffer`.
3. **Hardware/mode control** — cursor visibility and VGA blink mode. These map to
   real register writes on VGA and are no-ops on the framebuffer.

**Why a `u8`-oriented, not `char`-oriented, API?** The console speaks **CP437**,
the IBM PC code page, not Unicode. A single byte indexes directly into the
256-entry font table and into the VGA cell. Box-drawing characters such as
`0xDA` (┌) and `0xB3` (│) are CP437 glyphs that have no clean 1-byte ASCII
equivalent; keeping the API byte-based lets `draw_box` emit them directly.

### 3.1 The colour model — `Color`

`Color` (defined in `drivers/screen.rs`) is the 16-colour VGA palette as a
`#[repr(u8)]` enum (`Black=0 … White=15`). It is the *lingua franca* of both
backends:

- On **VGA** a `Color` is packed straight into the 4-bit fields of the attribute
  byte: `attr = (bg << 4) | fg`.
- On the **framebuffer** there is no palette hardware, so `Color::to_rgb(format)`
  converts the palette index into a concrete 32-bit pixel. The canonical values
  are stored in BGR layout (e.g. `LightBlue = 0x005555FF`); when the active
  `PixelFormat` is `Rgb`, `to_rgb` swaps the R and B bytes so the same logical
  colour looks correct regardless of the firmware's channel order.
- `Color::from_nibble(n)` is the inverse used when *decoding* a stored attribute
  byte back into a `Color` (needed when the framebuffer replays its shadow cells).

This dual interpretation is the key trick that lets the framebuffer backend store
its character grid in the *exact same* `(attr << 8) | char` `u16` format as VGA,
so `blit_framebuffer` and the syscall ABI are identical across both backends.

---

## 4. Global Routing & Backend Selection

### 4.1 The global instance

```rust
pub(crate) static GLOBAL_CONSOLE: SpinLock<Option<ConsoleImpl>> =
    SpinLock::new(Some(ConsoleImpl::Vga(VgaConsole)));
```

The console is a single global guarded by the kernel `SpinLock` (`docs/sync.md`).
Two deliberate choices:

- **It defaults to VGA text mode**, *before* `init` runs. This guarantees that
  any code path executing during very early boot — or inside an integration test
  that never parses a `BootInfo` — has a working console immediately, with no
  initialization order hazard. `VgaConsole` is a zero-sized type, so this default
  costs nothing and needs no allocator.
- **It is `Option<…>`** only so the slot can be re-published by `init`; it is
  never actually `None` in practice (hence the `expect` in `with_console`).

### 4.2 `init` — committing to a backend

```rust
pub fn init(video_type: VideoModeType) {
    let console = match video_type {
        VideoModeType::VgaText     => ConsoleImpl::Vga(VgaConsole),
        VideoModeType::Framebuffer => ConsoleImpl::Framebuffer(FramebufferConsole::new()),
    };
    *GLOBAL_CONSOLE.lock() = Some(console);
}
```

`KernelMain` reads `BootInfo.video_type` and calls `console::init(video_type)`
once, right after the heap comes up (the framebuffer backend allocates its
buffers, so the heap must exist first — see `main.rs:170-185`). After this call
the global slot holds the concrete backend for the rest of the kernel's life.

**Ordering subtlety on the framebuffer path.** `init` builds the
`FramebufferConsole` (allocating its shadow + backbuffer) but the *VRAM itself*
is not yet reachable: the linear framebuffer sits at a high physical address the
early identity map does not cover. `KernelMain` therefore calls
`map_framebuffer` only after the VMM is live, and only then performs the first
`clear()`/`writeln!`. Drawing before that mapping would fault, which is precisely
why the first framebuffer output is deferred (`main.rs:187-209`).

### 4.3 `with_console` — the access pattern

Every caller in the kernel reaches the console through one function:

```rust
pub fn with_console<R>(f: impl FnOnce(&mut dyn KernelConsole) -> R) -> R {
    let mut guard = GLOBAL_CONSOLE.lock();          // disables interrupts
    let console = guard.as_mut().expect("…");
    f(console)                                       // runs with exclusive access
}
```

This mirrors the `with_pmm` / `with_screen` convention used elsewhere in the
kernel: no caller ever holds a raw reference to global state. Because the kernel
`SpinLock` **disables interrupts for the duration of the guard**, the closure
runs atomically with respect to the timer ISR and preemption. That is essential:
a half-written escape of characters or a torn cursor update during a context
switch would corrupt the display. The cost is that the closure must stay short —
console writes are not a place to do long-running work.

Note the lock is handed out as `&mut dyn KernelConsole` (a trait object) purely
for API ergonomics, even though the underlying dispatch is a static enum match
(§5). The `dyn` indirection here is one virtual call per `with_console`, not per
character.

---

## 5. Static Dispatch — `ConsoleImpl` (`dispatch.rs`)

```rust
pub enum ConsoleImpl {
    Vga(VgaConsole),
    Framebuffer(FramebufferConsole),
}
impl KernelConsole for ConsoleImpl { /* match self { … } for every method */ }
```

`ConsoleImpl` is an **enum-dispatch** wrapper. Each trait method is implemented by
matching on the active variant and forwarding to the inner backend.

Why not just store `Box<dyn KernelConsole>` in the global? Because:

- A `Box` requires the **heap**, but the console must exist before the heap is
  initialized (the early-boot VGA default). An enum lives in static storage with
  zero allocation.
- The set of backends is **closed and known at compile time** (there are exactly
  two display worlds). An enum models a closed set more honestly than a trait
  object, and the compiler can inline through the `match`.

So the design is: *closed-set static dispatch internally* (`ConsoleImpl`), exposed
through a *single dynamic boundary* (`with_console` → `&mut dyn`) for caller
convenience. Best of both.

---

## 6. The VGA Backend (`vga.rs` + `drivers/screen.rs`)

`VgaConsole` is a **zero-sized struct**. It owns no state of its own; every method
delegates to the global VGA driver via `with_screen(|screen| …)`. All the real
work — and all the hardware contact — lives in `drivers/screen.rs`.

```rust
impl KernelConsole for VgaConsole {
    fn print_str(&mut self, s: &str) { with_screen(|screen| screen.print_str(s)); }
    fn get_dimensions(&self) -> (usize, usize) { (25, 80) }   // VGA text is fixed 80×25
    // … every other method delegates likewise …
}
```

A subtle point: `with_console` already holds `GLOBAL_CONSOLE`, and the VGA methods
then acquire `GLOBAL_SCREEN` (a *second*, independent `SpinLock` inside
`drivers/screen.rs`). These are different locks guarding different data, so there
is no self-deadlock; but it does mean a VGA console write briefly holds two locks.

### 6.1 Double-buffering in `drivers/screen.rs`

The `Screen` struct keeps **two** in-RAM `[VgaChar; 2000]` arrays:

- `back_buffer` — where all drawing operations land (plain memory writes, fast).
- `front_buffer` — a cache of what is currently believed to be on screen.

`flush()` walks the 2000 cells and writes **only the cells that differ** to the
`0xB8000` MMIO region with `ptr::write_volatile`, then updates `front_buffer`.
This matters because MMIO writes to video memory are dramatically slower than RAM
writes; diffing means a one-character change costs one volatile write, not 2000.
`with_screen` calls `flush()` automatically after every closure (`screen.rs:244-249`).

`write_volatile` is mandatory for MMIO: it forbids the compiler from coalescing,
reordering, or eliding the stores, all of which would be legal for ordinary
memory but would corrupt deterministic device output.

### 6.2 The hardware cursor

`update_cursor()` programs the blinking cursor by writing a linear cell offset
(`row * cols + col`) to CRTC registers 14 (high byte) and 15 (low byte) via the
`0x3D4`/`0x3D5` index/data port pair. The stream methods (`print_str`) update the
cursor **once** at the end of a whole string rather than per character, because
each cursor update is two pairs of port-I/O writes — expensive relative to a RAM
store.

`disable_hw_cursor()` sets **bit 5 of CRTC register `0x0A`** (Cursor Start), which
the VGA spec defines as "cursor off." Merely parking the cursor off-screen is not
reliable on real hardware, so the subsystem disables it properly.
`enable_hw_cursor()` reprograms registers `0x0A`/`0x0B` to draw a two-scanline
underline at scanlines 14–15 of the cell.

### 6.3 Blink mode vs. 16 background colours

By default, attribute bit 7 means "blink," limiting backgrounds to colours 0–7.
`disable_blink_mode()` clears **bit 3 of Attribute Controller register `0x10`**,
switching the hardware to *background-intensity* mode where bit 7 becomes the
high bit of a 4-bit background field — enabling all 16 colours as backgrounds
(needed by the TUI). The Attribute Controller uses an awkward shared
address/data port at `0x3C0` gated by a flip-flop that must be reset by reading
Input Status Register 1 (`0x3DA`) before each access; `screen.rs` documents and
follows that exact protocol. On the framebuffer backend these two methods are
no-ops — there is no blink hardware, and all 16 colours are always available.

---

## 7. The Framebuffer Backend (`framebuffer.rs`)

This is where most of the subsystem's complexity lives, because the CPU must do
everything the VGA character generator did for free. `FramebufferConsole` holds a
substantial amount of state:

```rust
pub struct FramebufferConsole {
    cells: Vec<u16>,        // shadow text grid: (attr << 8) | char, one per cell
    cols: usize, rows: usize,
    cursor_row: usize, cursor_col: usize,
    fg: Color, bg: Color,
    cursor_enabled: bool,
    fb_info: Option<FramebufferInfo>,

    backbuffer: Vec<u32>,   // full-resolution RAM copy of the screen's pixels
    dirty_y_min: u32,       // smallest modified scanline since last flush
    dirty_y_max: u32,       // largest  modified scanline since last flush
    deferred_redraw: bool,
}
```

Two layers of buffering, each solving a different problem:

- **`cells` (the shadow text grid).** A logical 80×25-style character grid in the
  *same* `u16` format as VGA. It lets the console answer "what character/colour is
  at this cell?" without reading pixels back, which is required for erasing the
  cursor (§7.4), for scrolling, and for `blit_framebuffer`. It is also what makes
  the framebuffer ABI-compatible with the VGA path.
- **`backbuffer` (the pixel shadow).** A full `width*stride*height` `u32` copy of
  the screen in plain RAM. **All glyph rasterization writes here, never directly
  to VRAM.** This is critical: reads from VRAM are extremely slow (often
  uncached/write-combining memory), and partial pixel writes can flicker. Drawing
  into RAM and flushing in bulk is an order of magnitude faster.

### 7.1 Construction and geometry

`FramebufferConsole::new()` reads the published `BOOT_INFO_PTR`. If it points to a
valid `BootInfo` whose `video_type == Framebuffer` and whose `base_address != 0`,
it derives the text grid from the pixel geometry and a fixed **8×16** glyph cell:

```
cols = width  / 8        rows = height / 16
backbuffer length = pixels_per_scanline * height   (u32 elements)
```

If no valid framebuffer is present (e.g. running under a test harness) it falls
back to an 80×25 grid with no `fb_info`, so all the drawing routines degrade to
harmless no-ops instead of dereferencing a null base.

`cells` is initialized to `0x0720` for every cell — attribute `0x07` (light-gray
on black) plus character `0x20` (space) — i.e. a cleared screen.

### 7.2 Glyph rasterization — `draw_char_pixel`

The font (`font_basic.rs`) is a `[[u8; 16]; 256]`: 256 glyphs, one per CP437 byte,
each 16 rows of 8 bits. A set bit is a foreground pixel; a clear bit is
background. Bit 7 (`0x80`) is the leftmost pixel, so the column test is
`byte & (1 << (7 - col))`.

`draw_char_pixel(x, y, ch, fg, bg)`:

1. Looks up the 16-byte glyph and converts `fg`/`bg` to 32-bit pixels via
   `to_rgb(pixel_format)` **once** (not per pixel).
2. **Fast path** (the glyph fits entirely within the screen): for each of the 16
   rows it builds an 8-pixel `[u32; 8]` scanline on the stack, then
   `copy_nonoverlapping`s those 8 words into the backbuffer at
   `(y+row) * pixels_per_scanline + x`. Building the row in a local array and
   doing one bulk copy is faster than 8 individual indexed stores.
3. **Slow path** (glyph straddles the screen edge): falls back to per-pixel
   `write_pixel`, which bounds-checks each pixel individually.
4. Marks scanlines `y .. y+15` dirty.

`write_pixel(x, y, color)` is the bounds-checked primitive: it validates
`x < width && y < height`, writes one `u32` into the backbuffer at
`y * pixels_per_scanline + x`, and marks that scanline dirty.

### 7.3 Dirty-line tracking and `flush_to_vram`

The console never flushes the whole screen if it can help it. Every write records
the affected scanline range via `mark_dirty` / `mark_dirty_range`, narrowing
`[dirty_y_min, dirty_y_max]`. `flush_to_vram()` then copies **only** the rows in
that band from the backbuffer to VRAM in a single `copy_nonoverlapping`:

```
start_offset   = dirty_y_min * pixels_per_scanline
pixels_to_copy = (dirty_y_max - dirty_y_min + 1) * pixels_per_scanline
copy_nonoverlapping(backbuffer + start_offset, vram + start_offset, pixels_to_copy)
```

After flushing it resets the band to "empty" (`dirty_y_min = u32::MAX`,
`dirty_y_max = 0`). Printing one character therefore uploads ~16 scanlines, not
the entire screen. The flush is the *only* place that touches VRAM, and it does so
with one bulk memcpy of contiguous scanlines — the cheapest possible VRAM access
pattern.

`flush_redraw()` is the public-facing flush wrapper; it clears the
`deferred_redraw` flag (set by scrolling) and calls `flush_to_vram`.

### 7.4 The software cursor

There is no hardware cursor on a framebuffer, so the console draws one itself: a
2-pixel-tall bar across the bottom of the current cell (scanlines 14–15),
rendered by `draw_cursor(visible)` in the current foreground colour (or the
background colour to "hide" it).

The tricky part is **erasing** it without leaving a gap in the glyph underneath.
`erase_cursor()` does not just paint background over the bar — it looks up the
*actual character currently stored* in `cells` at the cursor position, decodes its
attribute back into `fg`/`bg` via `Color::from_nibble`, and re-renders just the
bottom two rows (rows 14–15, `glyph.iter().skip(14)`) of that glyph. This restores
exactly the pixels the cursor bar covered, including the descenders of letters
like 'g' or 'y'.

The cursor lifecycle in stream output is: `put_char` erases the cursor first, does
its work, and the batch method (`print_str` / `write_str`) re-draws the cursor and
flushes once at the very end — never per character.

### 7.5 Character output and control codes — `put_char`

`put_char(c)` is the framebuffer equivalent of the VGA `put_char`. It handles the
same control codes:

| Byte        | Behaviour |
|-------------|-----------|
| `\n` (0x0A) | `cursor_row += 1; cursor_col = 0;` then `scroll()`. |
| `\r` (0x0D) | `cursor_col = 0`. |
| `\t` (0x09) | advance column to next multiple of 8 (`(col + 8) & !7`); wrap to next line if past the edge. |
| `0x08` (BS) | move back one cell (wrapping to the previous line's end), then blank that cell. |
| other       | if at end of line, wrap; if past the last row, scroll; draw the glyph via `draw_char_at_cell`, then `cursor_col += 1`. |

`draw_char_at_cell(row, col, ch, fg, bg)` is the bridge between the two buffers: it
updates the `cells[idx]` shadow entry **and** rasterizes the glyph into the
backbuffer at `col*8, row*16`. Keeping both in lock-step is what allows the cursor
erase and scroll logic to work purely from `cells`.

### 7.6 Scrolling — `scroll`

When `cursor_row` reaches `rows`, `scroll()` shifts everything up by one text line
in **both** buffers:

1. **Shadow grid:** copy `cells[cols..]` down to `cells[0..]` (drop the top row),
   then fill the new bottom row with blanks in the current attribute.
2. **Pixel backbuffer:** `copy_within` the pixel rows up by exactly
   `16 * pixels_per_scanline` (one text row's worth of scanlines). A RAM-to-RAM
   `memmove` of the whole screen is still very fast compared to touching VRAM.
3. Clear the freed bottom band of pixels to the background colour.
4. Mark the **entire** screen dirty and set `deferred_redraw`, so the next flush
   uploads the scrolled result in one shot.

This is the one operation that legitimately needs a full-screen VRAM upload —
every visible scanline genuinely changed.

### 7.7 Absolute drawing & `blit_framebuffer`

`draw_at`, `draw_char_at`, `draw_box`, and `fill_rect` mirror the VGA versions but
render through `draw_char_at_cell` into the backbuffer. They do **not** move the
stream cursor and do **not** scroll — they are the primitives the TUI builds on.
`draw_box` uses the CP437 single-line box-drawing bytes
(`0xDA 0xBF 0xC0 0xD9 0xC4 0xB3` = ┌ ┐ └ ┘ ─ │).

`blit_framebuffer(cells)` is the high-throughput path used by the user-space TUI
via the `WriteFramebuffer` syscall (§8). The application composes an entire frame
as a flat `[u16]` of `(attr << 8) | char` cells in its own memory and hands it
over in one call; the console decodes each cell's character and colours, updates
its shadow grid, rasterizes every glyph into the backbuffer, then flushes once.
This is how the TUI repaints a full screen without thousands of individual
syscalls. Crucially, the `u16` cell format is *identical* on both backends, so the
exact same user-space frame buffer blits correctly whether the kernel booted into
VGA text mode or a linear framebuffer.

---

## 8. Integration With the Rest of the Kernel

### 8.1 The logger (`logging.rs`)

The kernel logger writes to the serial port and, on demand, replays a captured
log buffer to the console through `print_captured_target(screen: &mut dyn
KernelConsole, …)`. Because it takes the trait object, the same dump code colours
and prints lines identically on either backend (it uses `set_color` +
`writeln!`).

### 8.2 The panic handler (`panic.rs`)

The panic path **deliberately bypasses** `GLOBAL_CONSOLE` and the
`FramebufferConsole`. A panic can occur while the console lock is already held, or
mid-heap-operation, so re-locking or allocating would deadlock or fault. Instead
`panic.rs` carries two standalone, lock-free, heap-free writers:

- `PanicFramebufferWriter` (in `panic.rs`) — rebuilds the framebuffer geometry
  straight from `BOOT_INFO_PTR` and renders glyphs directly to VRAM with
  `write_volatile`, reusing only the *stateless* helper
  `FramebufferConsole::glyph_for_byte`.
- `PanicScreenWriter` (in `drivers/screen.rs`) — the equivalent for VGA text mode,
  writing cells straight to `0xB8000`.

It picks the framebuffer writer if a linear framebuffer is active, otherwise the
VGA writer, paints a red panic banner, and halts (`cli; hlt`). This guarantees a
visible diagnostic on both display worlds even when the normal console is unusable.

### 8.3 User-space syscalls (`syscall/dispatch/console.rs`)

The user-space shell and TUI reach the console through capability-checked
syscalls, every one of which funnels through `with_console`:

| Syscall                 | Console method(s)                  | Notes |
|-------------------------|------------------------------------|-------|
| `WriteConsole(ptr,len)` | `print_char` per byte              | `len` clamped to `MAX_CONSOLE_WRITE_LEN` (4096) to bound ISR-disabled time. |
| `GetCursor()`           | `get_cursor`                       | returns `(row<<32)|col`. |
| `SetCursor(row,col)`    | `set_cursor`                       | out-of-range clamped by backend. |
| `ClearScreen()`         | `clear`                            | |
| `WriteFramebuffer(p,l)` | `blit_framebuffer`                 | `len` must equal `rows*cols`; ptr must be 2-byte aligned and fully in user space. |
| `GetConsoleDimensions()`| `get_dimensions`                   | returns `(rows<<32)|cols`. |
| `SetVgaMode(flags)`     | cursor + blink mode toggles        | bit 0 = hw cursor, bit 1 = blink. |

Every pointer argument is validated with `is_valid_user_buffer` before any
dereference, and lengths are clamped to prevent a malicious or buggy task from
holding interrupts disabled (inside the console `SpinLock`) for an unbounded time.
`GetConsoleDimensions` is what lets a single TUI binary adapt to either an 80×25
VGA screen or a much larger framebuffer text grid without recompilation.

---

## 9. Performance Summary (Why It's Built This Way)

| Technique | Where | Problem it solves |
|-----------|-------|-------------------|
| Diff-flush (back vs. front buffer) | `screen.rs::flush` | VGA MMIO writes are slow → write only changed cells. |
| Batched cursor update | both backends | Cursor reprogramming is port-I/O → update once per string, not per char. |
| RAM backbuffer | `framebuffer.rs` | VRAM reads/partial writes are very slow → rasterize in RAM. |
| Dirty-line band | `framebuffer.rs::flush_to_vram` | Avoid uploading the whole screen → copy only touched scanlines. |
| Per-scanline bulk copy | `draw_char_pixel` | 8-word `copy_nonoverlapping` beats 8 indexed stores. |
| `copy_within` scroll | `framebuffer.rs::scroll` | RAM memmove ≫ VRAM scroll. |
| Enum dispatch, no `Box` | `dispatch.rs` | Console must work before the heap; closed backend set. |
| Interrupt-disabling lock | `interface.rs::with_console` | Atomic screen updates vs. preemption / ISR re-entry. |

---

## 10. Extending the Subsystem

To add a new console backend (e.g. a serial-only console, or a double-resolution
framebuffer):

1. Implement `KernelConsole` (and therefore `core::fmt::Write`) for the new type.
2. Add a variant to `ConsoleImpl` in `dispatch.rs` and forward every method in the
   `match` arms.
3. Extend `VideoModeType` (in `boot_info.rs`) and the `match` in
   `interface.rs::init` so the bootloader's mode selection reaches the new backend.

No caller changes are needed: everything in the kernel already goes through
`with_console(|c| …)` and the trait, so a new backend is picked up transparently
once `init` can construct it.

---

## 11. File Reference

| File | Responsibility |
|------|----------------|
| `console/mod.rs` | Public re-exports (`init`, `with_console`, `KernelConsole`, the two backends, `ConsoleImpl`). |
| `console/interface.rs` | `KernelConsole` trait, `GLOBAL_CONSOLE`, `init`, `with_console`. |
| `console/dispatch.rs` | `ConsoleImpl` enum-dispatch wrapper. |
| `console/vga.rs` | `VgaConsole` ZST adapter onto `drivers/screen.rs`. |
| `console/framebuffer.rs` | `FramebufferConsole`: shadow grid, pixel backbuffer, dirty tracking, software cursor, scrolling, rasterizer. |
| `console/font_basic.rs` | `FONT_BASIC: [[u8;16];256]` and `glyph_for_byte`. |
| `drivers/screen.rs` | VGA hardware driver: `0xB8000` MMIO, CRTC cursor, Attribute Controller blink mode, double-buffering, `Color`. |
| `boot_info.rs` | `VideoModeType`, `PixelFormat`, `FramebufferInfo`, `BOOT_INFO_PTR`. |
| `panic.rs` | Lock-free panic writers for both display worlds. |
| `syscall/dispatch/console.rs` | User-space console syscall implementations. |
| `logging.rs` | Captured-log replay onto a `&mut dyn KernelConsole`. |

---

## See Also

- `docs/boot_bios.md`, `docs/boot_uefi.md` — how `BootInfo.video_type` and the
  framebuffer geometry are produced.
- `docs/tui.md` — the user-space TUI that drives `blit_framebuffer` and
  `SetVgaMode`.
- `docs/sync.md` — the `SpinLock` semantics that make `with_console` atomic.
- `docs/syscall.md` — the syscall dispatch path the console syscalls plug into.
