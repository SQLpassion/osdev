//! Panic handler for the kernel
//!
//! Required for `no_std` environments.

use crate::boot_info::{BootInfo, PixelFormat, VideoModeType, BOOT_INFO_PTR};
use crate::console::FramebufferConsole;
use crate::drivers::screen::Color;
use core::fmt::Write;
use core::panic::PanicInfo;
use core::sync::atomic::Ordering;

/// Pixel width of a single text glyph (matches the framebuffer console font).
const GLYPH_W: u32 = 8;
/// Pixel height of a single text glyph.
const GLYPH_H: u32 = 16;

/// Lock-free, heap-free framebuffer text writer for the panic path.
///
/// On a UEFI / linear-framebuffer boot there is no VGA text buffer at `0xB8000`,
/// so the legacy [`crate::drivers::screen::PanicScreenWriter`] produces no visible
/// output — a panic (or any fault that ends in one) would leave the screen frozen
/// with no diagnostic.  This writer renders panic text directly into the linear
/// framebuffer's VRAM, glyph by glyph, with:
/// - **no locks** — a panic may occur while `GLOBAL_CONSOLE` is already held, and
///   re-locking it would deadlock the panic path, and
/// - **no heap / backbuffer** — a panic may occur mid-heap-operation, so it writes
///   straight to VRAM rather than allocating a [`FramebufferConsole`].
struct PanicFramebufferWriter {
    /// Base of the linear framebuffer (identity-mapped 32-bit pixels).
    base: *mut u32,
    /// Pixels per scanline (stride), which may exceed `width`.
    stride: u32,
    /// Visible width in pixels.
    width: u32,
    /// Visible height in pixels.
    height: u32,
    /// On-the-wire pixel byte order, used to encode colors correctly.
    format: PixelFormat,
    /// Current cursor X in pixels.
    x: u32,
    /// Current cursor Y in pixels.
    y: u32,
}

impl PanicFramebufferWriter {
    /// Builds a writer from the published boot info, or `None` when the active
    /// boot is not a usable linear framebuffer (e.g. legacy VGA text mode).
    fn from_boot_info() -> Option<Self> {
        let raw = BOOT_INFO_PTR.load(Ordering::Relaxed);
        if raw == 0 {
            return None;
        }
        // SAFETY: `raw` is the boot-info pointer published in `KernelMain` after a
        // magic check; the structure lives in identity-mapped low memory.
        let bi = unsafe { &*(raw as *const BootInfo) };
        if bi.video_type != VideoModeType::Framebuffer || bi.fb_info.base_address == 0 {
            return None;
        }
        let fb = bi.fb_info;
        Some(Self {
            base: fb.base_address as *mut u32,
            stride: fb.pixels_per_scanline,
            width: fb.width,
            height: fb.height,
            format: fb.pixel_format,
            x: 0,
            y: 0,
        })
    }

    /// Fills the entire visible framebuffer with a single color.
    fn clear(&mut self, color: u32) {
        for y in 0..self.height {
            for x in 0..self.width {
                // SAFETY: `(y, x)` are within `height`/`width`, so the offset is
                // inside the framebuffer; the framebuffer is identity-mapped writable.
                unsafe {
                    self.base
                        .add((y * self.stride + x) as usize)
                        .write_volatile(color);
                }
            }
        }
        self.x = 0;
        self.y = 0;
    }

    /// Renders one glyph at the cursor, leaving the cursor unchanged.
    fn put_glyph(&self, ch: u8, fg: u32, bg: u32) {
        let glyph = FramebufferConsole::glyph_for_byte(ch);
        for (row, &byte) in glyph.iter().enumerate() {
            for col in 0..GLYPH_W {
                let color = if byte & (1 << (7 - col)) != 0 { fg } else { bg };
                let px = self.x + col;
                let py = self.y + row as u32;
                if px < self.width && py < self.height {
                    // SAFETY: bounds-checked against `width`/`height`; identity-mapped writable.
                    unsafe {
                        self.base
                            .add((py * self.stride + px) as usize)
                            .write_volatile(color);
                    }
                }
            }
        }
    }

    /// Advances the cursor to the start of the next text line.
    fn newline(&mut self) {
        self.x = 0;
        self.y = self.y.saturating_add(GLYPH_H);
    }
}

impl Write for PanicFramebufferWriter {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let fg = Color::White.to_rgb(self.format);
        let bg = Color::Red.to_rgb(self.format);
        for b in s.bytes() {
            match b {
                b'\n' => self.newline(),
                _ => {
                    if self.x + GLYPH_W > self.width {
                        self.newline();
                    }
                    self.put_glyph(b, fg, bg);
                    self.x += GLYPH_W;
                }
            }
        }
        Ok(())
    }
}

/// Writes the panic banner and details through `writer`.
fn render_panic(writer: &mut dyn Write, info: &PanicInfo) {
    let _ = writeln!(writer, "!!! KERNEL PANIC !!!");
    if let Some(location) = info.location() {
        let _ = writeln!(writer, "Location: {}:{}", location.file(), location.line());
        let _ = writeln!(writer);
    }
    let _ = writeln!(writer, "Message: {}", info.message());
}

/// Panic handler - called when the kernel panics.
///
/// Renders the panic message lock-free so it is safe even when a console lock is
/// already held.  On a framebuffer boot it draws straight to VRAM; otherwise it
/// falls back to the VGA text-mode writer.
#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    // Prefer the framebuffer on a graphics boot (UEFI / VBE); fall back to VGA
    // text mode when no linear framebuffer is active.
    if let Some(mut fb) = PanicFramebufferWriter::from_boot_info() {
        let bg = Color::Red.to_rgb(fb.format);
        fb.clear(bg);
        render_panic(&mut fb, info);
    } else {
        let mut screen = crate::drivers::screen::PanicScreenWriter::new(Color::White, Color::Blue);
        screen.clear();
        render_panic(&mut screen, info);
    }

    // Halt the CPU.
    loop {
        // SAFETY:
        // - This requires `unsafe` because inline assembly and privileged CPU instructions are outside Rust's static safety model.
        // - Panic path intentionally stops all forward progress.
        // - `cli`/`hlt` are privileged instructions and valid in ring 0.
        unsafe {
            core::arch::asm!("cli"); // Disable interrupts
            core::arch::asm!("hlt"); // Halt
        }
    }
}
