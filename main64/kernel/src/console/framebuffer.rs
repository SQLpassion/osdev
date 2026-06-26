//! Graphics Framebuffer Console Implementation.
//!
//! Implements a console backend that renders text into the graphics
//! framebuffer using a static 8x16 bitmap font.

use crate::drivers::screen::Color;
use crate::boot_info::PixelFormat;
use super::KernelConsole;

const GLYPH_W: u32 = 8;
const GLYPH_H: u32 = 16;


/// Represents the graphical console state, housing a character grid in system memory (cells),
/// cursor state, formatting metadata, and cached physical framebuffer details.
pub struct FramebufferConsole {
    /// In-memory character shadow buffer, storing `(attribute << 8) | character`.
    cells: alloc::vec::Vec<u16>,
    /// Number of text columns available on screen.
    cols: usize,
    /// Number of text rows available on screen.
    rows: usize,
    /// Current logical cursor row position (0-indexed).
    cursor_row: usize,
    /// Current logical cursor column position (0-indexed).
    cursor_col: usize,
    /// Current foreground color for text output.
    fg: Color,
    /// Current background color for text output.
    bg: Color,
    /// Indicates whether the logical cursor should be rendered on screen.
    cursor_enabled: bool,
    /// Cached configuration parameters of the active linear graphics framebuffer.
    fb_info: Option<crate::boot_info::FramebufferInfo>,
    
    /// RAM Backbuffer for pixel data to avoid slow VRAM reads and partial writes.
    backbuffer: alloc::vec::Vec<u32>,
    /// Minimum y-coordinate (scanline) that has been modified since last VRAM flush.
    dirty_y_min: u32,
    /// Maximum y-coordinate (scanline) that has been modified since last VRAM flush.
    dirty_y_max: u32,
    
    /// Defers redrawing the screen until a bulk operation is finished.
    deferred_redraw: bool,
}

impl Default for FramebufferConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl FramebufferConsole {
    /// Creates and initializes a new FramebufferConsole instance.
    pub fn new() -> Self {
        let raw = crate::boot_info::BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Relaxed);
        
        let (cols, rows, fb_info, bb_size) = if raw == 0 {
            (80, 25, None, 0)
        } else {
            // SAFETY: Checked pointer.
            let bi = unsafe { &*(raw as *const crate::boot_info::BootInfo) };
            if bi.video_type == crate::boot_info::VideoModeType::Framebuffer && bi.fb_info.base_address != 0 {
                let fb = bi.fb_info;
                let bb_size = (fb.pixels_per_scanline * fb.height) as usize;
                ((fb.width / GLYPH_W) as usize, (fb.height / GLYPH_H) as usize, Some(fb), bb_size)
            } else {
                (80, 25, None, 0)
            }
        };

        let cells = alloc::vec![0x0720; cols * rows];
        
        let mut backbuffer = alloc::vec::Vec::new();
        if bb_size > 0 {
            backbuffer.resize(bb_size, 0);
        }

        Self {
            cells,
            cols,
            rows,
            cursor_row: 0,
            cursor_col: 0,
            fg: Color::White,
            bg: Color::Black,
            cursor_enabled: true,
            fb_info,
            backbuffer,
            dirty_y_min: u32::MAX,
            dirty_y_max: 0,
            deferred_redraw: false,
        }
    }

    /// Computes the active 8-bit attribute byte from the current background and foreground colors.
    fn attribute(&self) -> u8 {
        ((self.bg as u8) << 4) | (self.fg as u8)
    }

    /// Retrieves the cached hardware framebuffer details.
    fn get_fb_info(&self) -> Option<crate::boot_info::FramebufferInfo> {
        self.fb_info
    }

    /// Selects the bitmap glyph for an ASCII or explicitly supported CP437 byte.
    pub fn glyph_for_byte(ch: u8) -> &'static [u8; 16] {
        super::font_basic::glyph_for_byte(ch)
    }

    /// Marks a specific scanline as dirty.
    fn mark_dirty(&mut self, y: u32) {
        if y < self.dirty_y_min {
            self.dirty_y_min = y;
        }
        if y > self.dirty_y_max {
            self.dirty_y_max = y;
        }
    }

    /// Marks a range of scanlines as dirty.
    fn mark_dirty_range(&mut self, y_start: u32, y_end: u32) {
        if y_start < self.dirty_y_min {
            self.dirty_y_min = y_start;
        }
        if y_end > self.dirty_y_max {
            self.dirty_y_max = y_end;
        }
    }

    /// Writes a single 32-bit pixel value into the RAM backbuffer at coordinates (x, y).
    fn write_pixel(&mut self, x: u32, y: u32, color: u32) {
        if let Some(fb) = self.fb_info {
            if x < fb.width && y < fb.height {
                let offset = (y * fb.pixels_per_scanline + x) as usize;
                self.backbuffer[offset] = color;
                self.mark_dirty(y);
            }
        }
    }

    /// Renders a single character glyph at pixel offset (x_start, y_start) onto the backbuffer.
    fn draw_char_pixel(&mut self, x_start: u32, y_start: u32, ch: u8, fg: Color, bg: Color) {
        let glyph = Self::glyph_for_byte(ch);
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let fg_rgb = fg.to_rgb(format);
        let bg_rgb = bg.to_rgb(format);

        if let Some(fb) = self.get_fb_info() {
            if x_start + GLYPH_W <= fb.width && y_start + GLYPH_H <= fb.height {
                for (row, &byte) in glyph.iter().enumerate() {
                    let mut scanline = [0u32; GLYPH_W as usize];
                    for (col, item) in scanline.iter_mut().enumerate() {
                        *item = if (byte & (1 << (7 - col))) != 0 { fg_rgb } else { bg_rgb };
                    }
                    let offset = ((y_start + row as u32) * fb.pixels_per_scanline + x_start) as usize;
                    
                    // SAFETY:
                    // - The bounds branch above guarantees the whole 8-pixel scanline is in range.
                    // - `backbuffer` is sized from `pixels_per_scanline * height`.
                    // - `scanline` has exactly `GLYPH_W` initialized pixels.
                    unsafe {
                        core::ptr::copy_nonoverlapping(scanline.as_ptr(), self.backbuffer.as_mut_ptr().add(offset), GLYPH_W as usize);
                    }
                }
                self.mark_dirty_range(y_start, y_start + 15);
            } else {
                for (row, &byte) in glyph.iter().enumerate() {
                    for col in 0..GLYPH_W {
                        let pixel_color = if (byte & (1 << (7 - col))) != 0 {
                            fg_rgb
                        } else {
                            bg_rgb
                        };
                        self.write_pixel(x_start + col, y_start + row as u32, pixel_color);
                    }
                }
            }
        }
    }

    /// Updates both the local shadow cells buffer and writes the character pixels to backbuffer.
    fn draw_char_at_cell(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        
        let idx = row * self.cols + col;
        let attr = ((bg as u8) << 4) | (fg as u8);
        let val = ((attr as u16) << 8) | (ch as u16);
        self.cells[idx] = val;

        self.draw_char_pixel(col as u32 * GLYPH_W, row as u32 * GLYPH_H, ch, fg, bg);
    }

    /// Renders or erases the visual representation of the console cursor at the current cursor coordinates.
    fn draw_cursor(&mut self, visible: bool) {
        if !self.cursor_enabled {
            return;
        }
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }

        let x_start = self.cursor_col as u32 * GLYPH_W;
        let y_start = self.cursor_row as u32 * GLYPH_H;

        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let color = if visible {
            self.fg.to_rgb(format)
        } else {
            self.bg.to_rgb(format)
        };

        for dy in (GLYPH_H - 2)..GLYPH_H {
            for dx in 0..GLYPH_W {
                self.write_pixel(x_start + dx, y_start + dy, color);
            }
        }
    }

    /// Erases the cursor by redrawing the character glyph parts hidden underneath the cursor box.
    fn erase_cursor(&mut self) {
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }
        let idx = self.cursor_row * self.cols + self.cursor_col;
        let cell = self.cells[idx];
        let ch = cell as u8;
        let attr = (cell >> 8) as u8;
        let fg = Color::from_nibble(attr & 0x0F);
        let bg = Color::from_nibble((attr >> 4) & 0x0F);

        let glyph = Self::glyph_for_byte(ch);
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let fg_rgb = fg.to_rgb(format);
        let bg_rgb = bg.to_rgb(format);

        let x_start = self.cursor_col as u32 * GLYPH_W;
        let y_start = self.cursor_row as u32 * GLYPH_H;

        for (dy, &byte) in glyph.iter().enumerate().skip(14) {
            for dx in 0..GLYPH_W {
                let pixel_color = if (byte & (1 << (7 - dx))) != 0 {
                    fg_rgb
                } else {
                    bg_rgb
                };
                self.write_pixel(x_start + dx, y_start + dy as u32, pixel_color);
            }
        }
    }

    /// Flushes dirty scanlines from the backbuffer to the physical VRAM.
    fn flush_to_vram(&mut self) {
        if self.dirty_y_min <= self.dirty_y_max {
            if let Some(fb) = self.fb_info {
                let fb_ptr = fb.base_address as *mut u32;
                let min_y = self.dirty_y_min;
                let max_y = self.dirty_y_max.min(fb.height.saturating_sub(1));
                
                if min_y <= max_y {
                    let start_offset = (min_y * fb.pixels_per_scanline) as usize;
                    let lines_to_copy = max_y - min_y + 1;
                    let pixels_to_copy = (lines_to_copy * fb.pixels_per_scanline) as usize;
                    
                    // SAFETY: 
                    // - `backbuffer` is large enough for all scanlines.
                    // - `fb_ptr` is the physical identity-mapped framebuffer memory.
                    // - The copy is bounded by height.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            self.backbuffer.as_ptr().add(start_offset),
                            fb_ptr.add(start_offset),
                            pixels_to_copy
                        );
                    }
                }
            }
            self.dirty_y_min = u32::MAX;
            self.dirty_y_max = 0;
        }
    }

    /// Handles deferred or explicit redraw synchronization.
    fn flush_redraw(&mut self) {
        if self.deferred_redraw {
            self.deferred_redraw = false;
        }
        self.flush_to_vram();
    }

    /// Shifts character cells upward when the cursor advances past the last visible row.
    fn scroll(&mut self) {
        if self.cursor_row >= self.rows {
            // Shift cells in system RAM shadow buffer up by one full row.
            let count = (self.rows - 1) * self.cols;
            for i in 0..count {
                self.cells[i] = self.cells[i + self.cols];
            }
            
            // Populate the newly freed bottom row with spaces.
            let attr = self.attribute();
            let blank = ((attr as u16) << 8) | (b' ' as u16);
            for col in 0..self.cols {
                self.cells[(self.rows - 1) * self.cols + col] = blank;
            }

            // Shift pixels in the RAM backbuffer up by one text row (16 scanlines)
            if let Some(fb) = self.fb_info {
                let stride = fb.pixels_per_scanline as usize;
                let copy_pixels = (self.rows - 1) * GLYPH_H as usize * stride;
                let shift_pixels = GLYPH_H as usize * stride;
                
                // memmove in RAM is extremely fast
                self.backbuffer.copy_within(shift_pixels..shift_pixels + copy_pixels, 0);
                
                // Clear the newly freed bottom row in the backbuffer
                let bg_rgb = self.bg.to_rgb(fb.pixel_format);
                let start_idx = copy_pixels;
                let end_idx = start_idx + GLYPH_H as usize * stride;
                self.backbuffer[start_idx..end_idx].fill(bg_rgb);
                
                // Mark the entire screen region dirty to upload the scrolled result
                self.mark_dirty_range(0, fb.height.saturating_sub(1));
            }

            // Fix logical cursor row and mark for deferred redraw.
            self.cursor_row = self.rows - 1;
            self.deferred_redraw = true;
        }
    }

    /// Renders a single byte char at the cursor position and parses control characters.
    fn put_char(&mut self, c: u8) {
        self.erase_cursor();

        match c {
            b'\n' => {
                self.cursor_row += 1;
                self.cursor_col = 0;
                self.scroll();
            }
            b'\r' => {
                self.cursor_col = 0;
            }
            b'\t' => {
                self.cursor_col = (self.cursor_col + 8) & !(8 - 1);
                if self.cursor_col >= self.cols {
                    self.cursor_col = 0;
                    self.cursor_row += 1;
                }
                self.scroll();
            }
            0x08 => {
                if self.cursor_col > 0 {
                    self.cursor_col -= 1;
                } else if self.cursor_row > 0 {
                    self.cursor_row -= 1;
                    self.cursor_col = self.cols - 1;
                } else {
                    return;
                }
                self.draw_char_at_cell(self.cursor_row, self.cursor_col, b' ', self.fg, self.bg);
            }
            _ => {
                if self.cursor_col >= self.cols {
                    self.cursor_row += 1;
                    self.cursor_col = 0;
                }

                if self.cursor_row >= self.rows {
                    self.scroll();
                }

                self.draw_char_at_cell(self.cursor_row, self.cursor_col, c, self.fg, self.bg);
                self.cursor_col += 1;
            }
        }
    }
}

impl core::fmt::Write for FramebufferConsole {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        for byte in s.bytes() {
            self.put_char(byte);
        }
        
        // At the end of a bulk write, draw the cursor and flush all dirty regions to VRAM
        self.draw_cursor(true);
        self.flush_redraw();
        Ok(())
    }
}

impl KernelConsole for FramebufferConsole {
    fn clear(&mut self) {
        let attr = self.attribute();
        let blank = ((attr as u16) << 8) | (b' ' as u16);
        for i in 0..self.cells.len() {
            self.cells[i] = blank;
        }
        
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.deferred_redraw = false;
        
        self.fill_physical_screen(self.bg);
        self.draw_cursor(true);
        self.flush_redraw();
    }

    fn print_char(&mut self, c: u8) {
        self.put_char(c);
        self.draw_cursor(true);
        self.flush_redraw();
    }

    fn print_str(&mut self, s: &str) {
        for byte in s.bytes() {
            self.put_char(byte);
        }
        self.draw_cursor(true);
        self.flush_redraw();
    }

    fn set_color(&mut self, color: Color) {
        self.fg = color;
    }

    fn set_cursor(&mut self, row: usize, col: usize) {
        self.erase_cursor();
        
        self.cursor_row = row.min(self.rows.saturating_sub(1));
        self.cursor_col = col.min(self.cols.saturating_sub(1));
        
        self.draw_cursor(true);
        self.flush_redraw();
    }

    fn get_cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    fn draw_box(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        fg: Color,
        bg: Color,
    ) {
        if width < 2 || height < 2 {
            return;
        }

        const TL: u8 = 0xDA;
        const TR: u8 = 0xBF;
        const BL: u8 = 0xC0;
        const BR: u8 = 0xD9;
        const H: u8 = 0xC4;
        const V: u8 = 0xB3;

        let last_col = col + width - 1;
        let last_row = row + height - 1;

        self.draw_char_at(row, col, TL, fg, bg);
        for c in col + 1..last_col {
            self.draw_char_at(row, c, H, fg, bg);
        }
        self.draw_char_at(row, last_col, TR, fg, bg);

        for r in row + 1..last_row {
            self.draw_char_at(r, col, V, fg, bg);
            self.draw_char_at(r, last_col, V, fg, bg);
        }

        self.draw_char_at(last_row, col, BL, fg, bg);
        for c in col + 1..last_col {
            self.draw_char_at(last_row, c, H, fg, bg);
        }
        self.draw_char_at(last_row, last_col, BR, fg, bg);
        
        self.flush_redraw();
    }

    fn draw_at(&mut self, mut row: usize, mut col: usize, text: &str, fg: Color, bg: Color) {
        for byte in text.bytes() {
            if col >= self.cols {
                col = 0;
                row += 1;
            }
            if row >= self.rows {
                break;
            }
            self.draw_char_at(row, col, byte, fg, bg);
            col += 1;
        }
        self.flush_redraw();
    }

    fn fill_rect(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        ch: u8,
        fg: Color,
        bg: Color,
    ) {
        for r in row..row.saturating_add(height).min(self.rows) {
            for c in col..col.saturating_add(width).min(self.cols) {
                self.draw_char_at(r, c, ch, fg, bg);
            }
        }
        self.flush_redraw();
    }

    fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        let is_cursor = row == self.cursor_row && col == self.cursor_col;
        if is_cursor {
            self.erase_cursor();
        }
        
        self.draw_char_at_cell(row, col, ch, fg, bg);
        
        if is_cursor {
            self.draw_cursor(true);
        }
        // Notice we don't flush_redraw() here for individual chars, let the batch methods do it.
    }

    fn blit_framebuffer(&mut self, cells: &[u16]) {
        self.erase_cursor();
        
        let count = cells.len().min(self.rows * self.cols);
        for (i, &val) in cells.iter().enumerate().take(count) {
            let ch = val as u8;
            let attr = (val >> 8) as u8;
            let fg = Color::from_nibble(attr & 0x0F);
            let bg = Color::from_nibble((attr >> 4) & 0x0F);
            
            let row = i / self.cols;
            let col = i % self.cols;
            
            self.cells[i] = val;
            self.draw_char_pixel(col as u32 * GLYPH_W, row as u32 * GLYPH_H, ch, fg, bg);
        }
        
        self.draw_cursor(true);
        self.flush_redraw();
    }

    fn get_dimensions(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    fn disable_hw_cursor(&mut self) {
        self.erase_cursor();
        self.cursor_enabled = false;
        self.flush_redraw();
    }

    fn enable_hw_cursor(&mut self) {
        self.cursor_enabled = true;
        self.draw_cursor(true);
        self.flush_redraw();
    }

    fn disable_blink_mode(&mut self) {
        // No-op for framebuffer.
    }

    fn enable_blink_mode(&mut self) {
        // No-op for framebuffer.
    }
}

impl FramebufferConsole {
    /// Clears the backbuffer with the background color and marks the entire screen dirty.
    fn fill_physical_screen(&mut self, bg: Color) {
        if let Some(fb) = self.fb_info {
            let bg_rgb = bg.to_rgb(fb.pixel_format);
            self.backbuffer.fill(bg_rgb);
            self.mark_dirty_range(0, fb.height.saturating_sub(1));
        }
    }
}
