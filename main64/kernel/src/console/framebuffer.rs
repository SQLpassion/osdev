//! Graphics Framebuffer Console Implementation.
//!
//! Implements a console backend that renders text into the graphics
//! framebuffer using a static 8x16 bitmap font.

use crate::drivers::screen::Color;
use crate::boot_info::PixelFormat;
use super::KernelConsole;

// Static 8x16 bitmap font
// Use the basic 8x16 font for rendering
use super::font_basic::FONT_BASIC as FONT_8X16;

// Helper to convert Color enum to 32-bit RGB color
fn color_to_rgb(color: Color, format: PixelFormat) -> u32 {
    let bgr = match color {
        Color::Black => 0x00000000,
        Color::Blue => 0x000000AA,
        Color::Green => 0x0000AA00,
        Color::Cyan => 0x0000AAAA,
        Color::Red => 0x00AA0000,
        Color::Magenta => 0x00AA00AA,
        Color::Brown => 0x00AA5500,
        Color::LightGray => 0x00AAAAAA,
        Color::DarkGray => 0x00555555,
        Color::LightBlue => 0x005555FF,
        Color::LightGreen => 0x0055FF55,
        Color::LightCyan => 0x0055FFFF,
        Color::LightRed => 0x00FF5555,
        Color::Pink => 0x00FF55FF,
        Color::Yellow => 0x00FFFF55,
        Color::White => 0x00FFFFFF,
    };
    match format {
        PixelFormat::Rgb => {
            let r = (bgr >> 16) & 0xFF;
            let g = (bgr >> 8) & 0xFF;
            let b = bgr & 0xFF;
            r | (g << 8) | (b << 16)
        }
        _ => bgr,
    }
}

// Helper to convert attribute nibble to Color enum
fn u8_to_color(val: u8) -> Color {
    match val & 0x0F {
        0 => Color::Black,
        1 => Color::Blue,
        2 => Color::Green,
        3 => Color::Cyan,
        4 => Color::Red,
        5 => Color::Magenta,
        6 => Color::Brown,
        7 => Color::LightGray,
        8 => Color::DarkGray,
        9 => Color::LightBlue,
        10 => Color::LightGreen,
        11 => Color::LightCyan,
        12 => Color::LightRed,
        13 => Color::Pink,
        14 => Color::Yellow,
        _ => Color::White,
    }
}

pub struct FramebufferConsole {
    cells: alloc::vec::Vec<u16>,
    cols: usize,
    rows: usize,
    cursor_row: usize,
    cursor_col: usize,
    fg: Color,
    bg: Color,
    cursor_enabled: bool,
    _blink_enabled: bool,
}

impl Default for FramebufferConsole {
    fn default() -> Self {
        Self::new()
    }
}

impl FramebufferConsole {
    pub fn new() -> Self {
        let raw = crate::boot_info::BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Relaxed);
        let (cols, rows) = if raw == 0 {
            (80, 25)
        } else {
            // SAFETY:
            // - `BOOT_INFO_PTR` contains a valid physical address to a `BootInfo` structure.
            // - Structure alignment and size are guaranteed.
            let bi = unsafe { &*(raw as *const crate::boot_info::BootInfo) };
            if bi.video_type == crate::boot_info::VideoModeType::Framebuffer && bi.fb_info.base_address != 0 {
                ((bi.fb_info.width / 8) as usize, (bi.fb_info.height / 16) as usize)
            } else {
                (80, 25)
            }
        };

        let cells = alloc::vec![0x0720; cols * rows];

        Self {
            cells,
            cols,
            rows,
            cursor_row: 0,
            cursor_col: 0,
            fg: Color::White,
            bg: Color::Black,
            cursor_enabled: true,
            _blink_enabled: false,
        }
    }

    fn attribute(&self) -> u8 {
        ((self.bg as u8) << 4) | (self.fg as u8)
    }

    fn get_fb_info(&self) -> Option<crate::boot_info::FramebufferInfo> {
        let raw = crate::boot_info::BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Relaxed);
        if raw == 0 {
            None
        } else {
            // SAFETY:
            // - `BOOT_INFO_PTR` contains a valid physical address to a `BootInfo` structure.
            let bi = unsafe { &*(raw as *const crate::boot_info::BootInfo) };
            if bi.video_type == crate::boot_info::VideoModeType::Framebuffer {
                Some(bi.fb_info)
            } else {
                None
            }
        }
    }

    fn write_pixel(&self, x: u32, y: u32, color: u32) {
        if let Some(fb) = self.get_fb_info() {
            if x < fb.width && y < fb.height {
                let fb_ptr = fb.base_address as *mut u32;
                let offset = (y * fb.pixels_per_scanline + x) as isize;
                // SAFETY:
                // - The physical framebuffer is identity-mapped.
                // - Coordinates (x, y) are bounded and within the valid memory allocation of the framebuffer.
                // - Concurrent writes are synchronized via the global console lock.
                unsafe {
                    fb_ptr.offset(offset).write_volatile(color);
                }
            }
        }
    }

    fn draw_char_pixel(&self, x_start: u32, y_start: u32, ch: u8, fg: Color, bg: Color) {
        let glyph_idx = if ch < 128 { ch as usize } else { 0x3F };
        let glyph = FONT_8X16[glyph_idx];
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let fg_rgb = color_to_rgb(fg, format);
        let bg_rgb = color_to_rgb(bg, format);

        for (row, &byte) in glyph.iter().enumerate() {
            for col in 0..8 {
                let pixel_color = if (byte & (1 << (7 - col))) != 0 {
                    fg_rgb
                } else {
                    bg_rgb
                };
                self.write_pixel(x_start + col as u32, y_start + row as u32, pixel_color);
            }
        }
    }

    fn draw_char_at_cell(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        if row >= self.rows || col >= self.cols {
            return;
        }
        let idx = row * self.cols + col;
        let attr = ((bg as u8) << 4) | (fg as u8);
        self.cells[idx] = ((attr as u16) << 8) | (ch as u16);

        self.draw_char_pixel((col * 8) as u32, (row * 16) as u32, ch, fg, bg);
    }

    fn draw_cursor(&self, visible: bool) {
        if !self.cursor_enabled {
            return;
        }
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }

        let x_start = (self.cursor_col * 8) as u32;
        let y_start = (self.cursor_row * 16) as u32;

        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let color = if visible {
            color_to_rgb(self.fg, format)
        } else {
            color_to_rgb(self.bg, format)
        };

        for dy in 14..16 {
            for dx in 0..8 {
                self.write_pixel(x_start + dx as u32, y_start + dy as u32, color);
            }
        }
    }

    fn erase_cursor(&self) {
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }
        let idx = self.cursor_row * self.cols + self.cursor_col;
        let cell = self.cells[idx];
        let ch = cell as u8;
        let attr = (cell >> 8) as u8;
        let fg = u8_to_color(attr & 0x0F);
        let bg = u8_to_color((attr >> 4) & 0x0F);

        let glyph_idx = if ch < 128 { ch as usize } else { 0x3F };
        let glyph = FONT_8X16[glyph_idx];
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let fg_rgb = color_to_rgb(fg, format);
        let bg_rgb = color_to_rgb(bg, format);

        let x_start = (self.cursor_col * 8) as u32;
        let y_start = (self.cursor_row * 16) as u32;

        for (dy, &byte) in glyph.iter().enumerate().skip(14) {
            for dx in 0..8 {
                let pixel_color = if (byte & (1 << (7 - dx))) != 0 {
                    fg_rgb
                } else {
                    bg_rgb
                };
                self.write_pixel(x_start + dx as u32, y_start + dy as u32, pixel_color);
            }
        }
    }

    fn scroll_down(&mut self) {
        if let Some(fb) = self.get_fb_info() {
            let fb_ptr = fb.base_address as *mut u32;
            let scanline = fb.pixels_per_scanline as usize;
            let copy_rows = (fb.height as usize).saturating_sub(16);
            let count = copy_rows * scanline;

            let dst = fb_ptr;
            // SAFETY:
            // - `fb_ptr` starts the identity-mapped framebuffer memory area.
            // - `scanline` elements are valid, meaning `16 * scanline` is a valid offset within bounds.
            let src = unsafe { fb_ptr.add(16 * scanline) };

            // SAFETY:
            // - `src` and `dst` both point to mapped memory within the framebuffer.
            // - `count` elements does not exceed the valid size of the framebuffer.
            // - `core::ptr::copy` handles overlapping memory blocks safely.
            unsafe {
                core::ptr::copy(src, dst, count);
            }

            let format = fb.pixel_format;
            let bg_rgb = color_to_rgb(self.bg, format);
            let start_y = fb.height.saturating_sub(16);
            for y in start_y..fb.height {
                let row_offset = (y * fb.pixels_per_scanline) as isize;
                for x in 0..fb.width {
                    // SAFETY:
                    // - `fb_ptr` is mapped and writable.
                    // - `row_offset + x` is within the valid framebuffer bounds.
                    unsafe {
                        fb_ptr.offset(row_offset + x as isize).write_volatile(bg_rgb);
                    }
                }
            }
        }
    }

    fn scroll(&mut self) {
        if self.cursor_row >= self.rows {
            let count = (self.rows - 1) * self.cols;
            for i in 0..count {
                self.cells[i] = self.cells[i + self.cols];
            }
            let attr = self.attribute();
            let blank = ((attr as u16) << 8) | (b' ' as u16);
            for col in 0..self.cols {
                self.cells[(self.rows - 1) * self.cols + col] = blank;
            }

            self.cursor_row = self.rows - 1;
            self.scroll_down();
        }
    }

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
        self.draw_cursor(true);
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
        self.fill_physical_screen(self.bg);
        self.draw_cursor(true);
    }

    fn print_char(&mut self, _c: u8) {
        self.put_char(_c);
        self.draw_cursor(true);
    }

    fn print_str(&mut self, _s: &str) {
        for byte in _s.bytes() {
            self.put_char(byte);
        }
        self.draw_cursor(true);
    }

    fn set_color(&mut self, _color: Color) {
        self.fg = _color;
    }

    fn set_cursor(&mut self, _row: usize, _col: usize) {
        self.erase_cursor();
        self.cursor_row = _row.min(self.rows.saturating_sub(1));
        self.cursor_col = _col.min(self.cols.saturating_sub(1));
        self.draw_cursor(true);
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
    }

    fn blit_framebuffer(&mut self, cells: &[u16]) {
        self.erase_cursor();
        let count = cells.len().min(self.rows * self.cols);
        for (i, &val) in cells.iter().enumerate().take(count) {
            let ch = val as u8;
            let attr = (val >> 8) as u8;
            let fg = u8_to_color(attr & 0x0F);
            let bg = u8_to_color((attr >> 4) & 0x0F);
            
            let row = i / self.cols;
            let col = i % self.cols;
            
            self.cells[i] = val;
            self.draw_char_pixel((col * 8) as u32, (row * 16) as u32, ch, fg, bg);
        }
        self.draw_cursor(true);
    }

    fn get_dimensions(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    fn disable_hw_cursor(&mut self) {
        self.erase_cursor();
        self.cursor_enabled = false;
    }

    fn enable_hw_cursor(&mut self) {
        self.cursor_enabled = true;
        self.draw_cursor(true);
    }

    fn disable_blink_mode(&mut self) {
        self._blink_enabled = false;
    }

    fn enable_blink_mode(&mut self) {
        self._blink_enabled = true;
    }
}

impl FramebufferConsole {
    fn fill_physical_screen(&self, bg: Color) {
        if let Some(fb) = self.get_fb_info() {
            let fb_ptr = fb.base_address as *mut u32;
            let format = fb.pixel_format;
            let bg_rgb = color_to_rgb(bg, format);
            let mut y = 0u32;
            while y < fb.height {
                let row = (y * fb.pixels_per_scanline) as isize;
                let mut x = 0u32;
                while x < fb.width {
                    // SAFETY:
                    // - The framebuffer is mapped and writable.
                    // - Coordinates are within bounds.
                    unsafe {
                        fb_ptr.offset(row + x as isize).write_volatile(bg_rgb);
                    }
                    x += 1;
                }
                y += 1;
            }
        }
    }
}
