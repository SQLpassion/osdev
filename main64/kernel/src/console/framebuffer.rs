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

/// Converts a `Color` enum variant to a 32-bit RGB color word based on the target `PixelFormat`.
///
/// This function acts as the color palette mapping step before writing bytes to VRAM.
fn color_to_rgb(color: Color, format: PixelFormat) -> u32 {
    // Step 1: Map the high-level console Color enum to default 32-bit BGR colors.
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

    // Step 2: Format the color representation according to the hardware pixel layout.
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

/// Converts a 4-bit attribute nibble (VGA compatible) to its corresponding `Color` variant.
fn u8_to_color(val: u8) -> Color {
    // Extract the lower 4 bits to map standard VGA colors (0-15).
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
    /// Internal hardware blink state tracker (unused in raw framebuffers).
    _blink_enabled: bool,
    /// Cached configuration parameters of the active linear graphics framebuffer.
    fb_info: Option<crate::boot_info::FramebufferInfo>,
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
    ///
    /// This resolves the hardware display configuration once and defaults to a 80x25 character fallback
    /// if no valid graphics framebuffer parameters are detected.
    pub fn new() -> Self {
        // Step 1: Load the raw pointer to the boot information structure.
        let raw = crate::boot_info::BOOT_INFO_PTR.load(core::sync::atomic::Ordering::Relaxed);
        let mut fb_info = None;

        // Step 2: Extract geometry dimensions and framebuffer info from BootInfo.
        let (cols, rows) = if raw == 0 {
            (80, 25)
        } else {
            // SAFETY:
            // - `BOOT_INFO_PTR` contains a valid physical address to a `BootInfo` structure.
            // - Structure alignment and size are guaranteed.
            let bi = unsafe { &*(raw as *const crate::boot_info::BootInfo) };
            if bi.video_type == crate::boot_info::VideoModeType::Framebuffer && bi.fb_info.base_address != 0 {
                fb_info = Some(bi.fb_info);
                ((bi.fb_info.width / 8) as usize, (bi.fb_info.height / 16) as usize)
            } else {
                (80, 25)
            }
        };

        // Step 3: Allocate the system memory shadow buffer for tracking written screen character cells.
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
            fb_info,
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

    /// Writes a single 32-bit pixel value into the graphics framebuffer at coordinates (x, y).
    ///
    /// Bounds checks coordinates to prevent corrupting system memory beyond the linear framebuffer window.
    fn write_pixel(&self, x: u32, y: u32, color: u32) {
        // Step 1: Ensure cached framebuffer parameters are available and coordinates are in bounds.
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

    /// Renders a single character glyph at pixel offset (x_start, y_start) onto the graphics framebuffer.
    ///
    /// Renders using the static 8x16 bitmap font.
    fn draw_char_pixel(&self, x_start: u32, y_start: u32, ch: u8, fg: Color, bg: Color) {
        // Step 1: Determine the character index mapping to ASCII range (fallback to '?' if outside).
        let glyph_idx = if ch < 128 { ch as usize } else { 0x3F };
        let glyph = FONT_8X16[glyph_idx];
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let fg_rgb = color_to_rgb(fg, format);
        let bg_rgb = color_to_rgb(bg, format);

        // Step 2: Draw the 8x16 pixel matrix efficiently using scanline copies.
        if let Some(fb) = self.get_fb_info() {
            if x_start + 8 <= fb.width && y_start + 16 <= fb.height {
                let fb_ptr = fb.base_address as *mut u32;
                for (row, &byte) in glyph.iter().enumerate() {
                    let mut scanline = [0u32; 8];
                    for col in 0..8 {
                        scanline[col] = if (byte & (1 << (7 - col))) != 0 { fg_rgb } else { bg_rgb };
                    }
                    let offset = ((y_start + row as u32) * fb.pixels_per_scanline + x_start) as isize;
                    
                    // SAFETY:
                    // - `scanline` is a valid 8-element array on the stack.
                    // - `fb_ptr` points to the identity-mapped framebuffer memory.
                    // - The bounds check ensures `x_start + 8` and `y_start + 16` are within the physical framebuffer.
                    // - The source (RAM) and destination (VRAM) do not overlap.
                    unsafe {
                        core::ptr::copy_nonoverlapping(scanline.as_ptr(), fb_ptr.offset(offset), 8);
                    }
                }
            } else {
                // Fallback for partial clipping at the exact screen edges.
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
        }
    }

    /// Updates both the local shadow cells buffer and writes the character pixels to VRAM.
    fn draw_char_at_cell(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        // Step 1: Reject out-of-bounds cell coordinates.
        if row >= self.rows || col >= self.cols {
            return;
        }
        
        // Step 2: Update the in-memory shadow cell entry to keep state consistent.
        let idx = row * self.cols + col;
        let attr = ((bg as u8) << 4) | (fg as u8);
        self.cells[idx] = ((attr as u16) << 8) | (ch as u16);

        // Step 3: Draw character glyph pixels to the VRAM area only if we are not deferring redraws.
        if !self.deferred_redraw {
            self.draw_char_pixel((col * 8) as u32, (row * 16) as u32, ch, fg, bg);
        }
    }

    /// Renders or erases the visual representation of the console cursor at the current cursor coordinates.
    fn draw_cursor(&self, visible: bool) {
        // Step 1: Exit early if the cursor is globally disabled or coordinates are out of bounds.
        if !self.cursor_enabled {
            return;
        }
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }

        let x_start = (self.cursor_col * 8) as u32;
        let y_start = (self.cursor_row * 16) as u32;

        // Step 2: Choose cursor color (foreground color to render, background color to erase).
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let color = if visible {
            color_to_rgb(self.fg, format)
        } else {
            color_to_rgb(self.bg, format)
        };

        // Step 3: Draw a horizontal block spanning the bottom two lines of the character cell matrix.
        for dy in 14..16 {
            for dx in 0..8 {
                self.write_pixel(x_start + dx as u32, y_start + dy as u32, color);
            }
        }
    }

    /// Erases the cursor by redrawing the character glyph parts hidden underneath the cursor box.
    fn erase_cursor(&self) {
        // Step 1: Ensure coordinates are in bounds before reading the cell data.
        if self.cursor_row >= self.rows || self.cursor_col >= self.cols {
            return;
        }
        let idx = self.cursor_row * self.cols + self.cursor_col;
        let cell = self.cells[idx];
        let ch = cell as u8;
        let attr = (cell >> 8) as u8;
        let fg = u8_to_color(attr & 0x0F);
        let bg = u8_to_color((attr >> 4) & 0x0F);

        // Step 2: Fetch the character glyph details.
        let glyph_idx = if ch < 128 { ch as usize } else { 0x3F };
        let glyph = FONT_8X16[glyph_idx];
        let format = self.get_fb_info().map(|fb| fb.pixel_format).unwrap_or(PixelFormat::Bgr);
        let fg_rgb = color_to_rgb(fg, format);
        let bg_rgb = color_to_rgb(bg, format);

        let x_start = (self.cursor_col * 8) as u32;
        let y_start = (self.cursor_row * 16) as u32;

        // Step 3: Redraw only the bottom two rows of the glyph that the cursor blocks.
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

    /// Redraws all character cells from the in-memory shadow buffer `cells` to the physical screen.
    /// This avoids reading from the slow physical framebuffer (VRAM) during scroll operations,
    /// and uses an intermediate line buffer in RAM to perform fast sequential block writes to VRAM.
    fn redraw_from_cells(&self) {
        if let Some(fb) = self.get_fb_info() {
            let fb_ptr = fb.base_address as *mut u32;
            let format = fb.pixel_format;
            let row_height = 16;
            let render_width = (self.cols * 8) as usize;
            
            // Step 1: Allocate a buffer for one entire text row (16 scanlines) in system RAM.
            let mut row_buffer = alloc::vec![0u32; render_width * row_height];
            
            // Step 2: Process each text row locally in RAM.
            for row in 0..self.rows {
                for col in 0..self.cols {
                    let idx = row * self.cols + col;
                    let cell = self.cells[idx];
                    let ch = cell as u8;
                    let attr = (cell >> 8) as u8;
                    let fg = u8_to_color(attr & 0x0F);
                    let bg = u8_to_color((attr >> 4) & 0x0F);
                    
                    let fg_rgb = color_to_rgb(fg, format);
                    let bg_rgb = color_to_rgb(bg, format);
                    
                    let glyph_idx = if ch < 128 { ch as usize } else { 0x3F };
                    let glyph = FONT_8X16[glyph_idx];
                    
                    for (gy, &byte) in glyph.iter().enumerate() {
                        let y_offset = gy * render_width;
                        let x_offset = col * 8;
                        for gx in 0..8 {
                            let pixel_color = if (byte & (1 << (7 - gx))) != 0 { fg_rgb } else { bg_rgb };
                            row_buffer[y_offset + x_offset + gx] = pixel_color;
                        }
                    }
                }
                
                // Step 3: Blast the fully rendered text row into VRAM efficiently.
                for gy in 0..row_height {
                    let fb_y = (row * 16 + gy) as u32;
                    if fb_y >= fb.height { break; }
                    let fb_offset = (fb_y * fb.pixels_per_scanline) as isize;
                    let buf_offset = gy * render_width;
                    
                    // SAFETY:
                    // - `row_buffer` is a valid heap allocation.
                    // - `fb_ptr` points to the mapped physical framebuffer.
                    // - `fb_offset` and `render_width` are within the bounds of the framebuffer geometry.
                    // - `copy_nonoverlapping` from RAM to VRAM is safe and massively faster than pixel loops.
                    unsafe {
                        core::ptr::copy_nonoverlapping(
                            row_buffer.as_ptr().add(buf_offset),
                            fb_ptr.offset(fb_offset),
                            render_width
                        );
                    }
                }
            }
        }
    }

    /// Flushes any pending full-screen redraws to VRAM.
    fn flush_redraw(&mut self) {
        if self.deferred_redraw {
            self.redraw_from_cells();
            self.deferred_redraw = false;
        }
    }

    /// Shifts character cells upward when the cursor advances past the last visible row.
    fn scroll(&mut self) {
        // Step 1: Check if the cursor coordinates require scrolling.
        if self.cursor_row >= self.rows {
            // Step 2: Shift cells in system RAM shadow buffer up by one full row.
            let count = (self.rows - 1) * self.cols;
            for i in 0..count {
                self.cells[i] = self.cells[i + self.cols];
            }
            
            // Step 3: Populate the newly freed bottom row with spaces.
            let attr = self.attribute();
            let blank = ((attr as u16) << 8) | (b' ' as u16);
            for col in 0..self.cols {
                self.cells[(self.rows - 1) * self.cols + col] = blank;
            }

            // Step 4: Fix logical cursor row and mark for deferred redraw.
            self.cursor_row = self.rows - 1;
            self.deferred_redraw = true;
        }
    }

    /// Renders a single byte char at the cursor position and parses control characters (\n, \r, \t, backspace).
    fn put_char(&mut self, c: u8) {
        // Step 1: Temporarily hide the visual cursor.
        self.erase_cursor();

        // Step 2: Handle control characters or default printable ASCII layout.
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
                // Handle backspace cursor movement and clear last character cell.
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
                // Auto-wrap character output if column exceeds screen width limits.
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
    /// Renders string content to the console, processing byte characters sequentially.
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        // Step 1: Draw string bytes individually.
        for byte in s.bytes() {
            self.put_char(byte);
        }
        
        // Step 2: Flush pending batch operations and restore the visual cursor on screen.
        self.flush_redraw();
        self.draw_cursor(true);
        Ok(())
    }
}

impl KernelConsole for FramebufferConsole {
    /// Clears the console shadow grid and clears the physical display screen with the background color.
    fn clear(&mut self) {
        // Step 1: Reset the character shadow cells buffer with blank spaces.
        let attr = self.attribute();
        let blank = ((attr as u16) << 8) | (b' ' as u16);
        for i in 0..self.cells.len() {
            self.cells[i] = blank;
        }
        
        // Step 2: Reset internal logical cursor tracking.
        self.cursor_row = 0;
        self.cursor_col = 0;
        self.deferred_redraw = false;
        
        // Step 3: Draw background color over physical VRAM pixels and show cursor.
        self.fill_physical_screen(self.bg);
        self.draw_cursor(true);
    }

    /// Renders a single character and forces redraw of the cursor.
    fn print_char(&mut self, _c: u8) {
        self.put_char(_c);
        self.flush_redraw();
        self.draw_cursor(true);
    }

    /// Renders a string and forces redraw of the cursor.
    fn print_str(&mut self, _s: &str) {
        for byte in _s.bytes() {
            self.put_char(byte);
        }
        self.flush_redraw();
        self.draw_cursor(true);
    }

    /// Configures the current foreground color for text printing.
    fn set_color(&mut self, _color: Color) {
        self.fg = _color;
    }

    /// Positions the logical cursor at (row, col) and redraws it on screen.
    fn set_cursor(&mut self, _row: usize, _col: usize) {
        // Step 1: Erase old cursor visual footprint.
        self.erase_cursor();
        
        // Step 2: Update cursor coordinates clamped to logical screen boundaries.
        self.cursor_row = _row.min(self.rows.saturating_sub(1));
        self.cursor_col = _col.min(self.cols.saturating_sub(1));
        
        // Step 3: Render cursor at the new position.
        self.draw_cursor(true);
    }

    /// Returns the current coordinates of the cursor (row, col).
    fn get_cursor(&self) -> (usize, usize) {
        (self.cursor_row, self.cursor_col)
    }

    /// Draws a double-line-styled borders box at (row, col) with custom width/height.
    fn draw_box(
        &mut self,
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        fg: Color,
        bg: Color,
    ) {
        // Step 1: Reject boxes with size too small to house margins.
        if width < 2 || height < 2 {
            return;
        }

        // Box border glyph representations in CP437 standard layout.
        const TL: u8 = 0xDA;
        const TR: u8 = 0xBF;
        const BL: u8 = 0xC0;
        const BR: u8 = 0xD9;
        const H: u8 = 0xC4;
        const V: u8 = 0xB3;

        let last_col = col + width - 1;
        let last_row = row + height - 1;

        // Step 2: Draw the top row including corners and horizontal segments.
        self.draw_char_at(row, col, TL, fg, bg);
        for c in col + 1..last_col {
            self.draw_char_at(row, c, H, fg, bg);
        }
        self.draw_char_at(row, last_col, TR, fg, bg);

        // Step 3: Draw vertical segments connecting corners.
        for r in row + 1..last_row {
            self.draw_char_at(r, col, V, fg, bg);
            self.draw_char_at(r, last_col, V, fg, bg);
        }

        // Step 4: Draw bottom border layout.
        self.draw_char_at(last_row, col, BL, fg, bg);
        for c in col + 1..last_col {
            self.draw_char_at(last_row, c, H, fg, bg);
        }
        self.draw_char_at(last_row, last_col, BR, fg, bg);
    }

    /// Renders string content starting at coordinates (row, col) without advancing cursor state globally.
    fn draw_at(&mut self, mut row: usize, mut col: usize, text: &str, fg: Color, bg: Color) {
        // Step 1: Parse and render string bytes, line-wrapping if column bounds are exceeded.
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

    /// Fills a rectangular grid segment starting at (row, col) with custom character and styling attributes.
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
        // Iterate and render characters row-by-row, column-by-column.
        for r in row..row.saturating_add(height).min(self.rows) {
            for c in col..col.saturating_add(width).min(self.cols) {
                self.draw_char_at(r, c, ch, fg, bg);
            }
        }
    }

    /// Renders a single character glyph at a specific position, handling cursor preservation.
    fn draw_char_at(&mut self, row: usize, col: usize, ch: u8, fg: Color, bg: Color) {
        // Step 1: If target position overlaps active cursor, erase it.
        let is_cursor = row == self.cursor_row && col == self.cursor_col;
        if is_cursor {
            self.erase_cursor();
        }
        
        // Step 2: Draw the character cell.
        self.draw_char_at_cell(row, col, ch, fg, bg);
        
        // Step 3: Redraw cursor if it was hidden.
        if is_cursor {
            self.draw_cursor(true);
        }
    }

    /// Copies a block of cell codes directly into the local shadow buffer and draws the graphics.
    fn blit_framebuffer(&mut self, cells: &[u16]) {
        // Step 1: Erase old cursor.
        self.erase_cursor();
        
        // Step 2: Populate shadow buffer cells and render glyph pixels.
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
        
        // Step 3: Render cursor again.
        self.flush_redraw();
        self.draw_cursor(true);
    }

    /// Returns the text grid size (rows, cols) of the active console screen.
    fn get_dimensions(&self) -> (usize, usize) {
        (self.rows, self.cols)
    }

    /// Disables logical rendering of the cursor footprint.
    fn disable_hw_cursor(&mut self) {
        self.erase_cursor();
        self.cursor_enabled = false;
    }

    /// Re-enables cursor rendering and draws it immediately.
    fn enable_hw_cursor(&mut self) {
        self.cursor_enabled = true;
        self.draw_cursor(true);
    }

    /// Disables cursor blinking (no-op in linear framebuffers).
    fn disable_blink_mode(&mut self) {
        self._blink_enabled = false;
    }

    /// Enables cursor blinking (no-op in linear framebuffers).
    fn enable_blink_mode(&mut self) {
        self._blink_enabled = true;
    }
}

impl FramebufferConsole {
    /// Clears the physical graphics framebuffer by writing the background color to every pixel.
    fn fill_physical_screen(&self, bg: Color) {
        // Step 1: Load cached framebuffer and set target RGB color code.
        if let Some(fb) = self.get_fb_info() {
            let fb_ptr = fb.base_address as *mut u32;
            let format = fb.pixel_format;
            let bg_rgb = color_to_rgb(bg, format);
            
            // Step 2: Allocate a scanline buffer populated with the background color.
            let mut scanline = alloc::vec![bg_rgb; fb.width as usize];
            let mut y = 0u32;
            
            // Step 3: Copy the scanline block into each row of the VRAM.
            while y < fb.height {
                let offset = (y * fb.pixels_per_scanline) as isize;
                // SAFETY:
                // - `scanline` is a valid heap allocation matching the screen width.
                // - `fb_ptr` points to the physical framebuffer memory.
                // - The `offset` strictly stays within `fb.height * pixels_per_scanline`.
                // - Fast bulk copy is safe as regions do not overlap.
                unsafe {
                    core::ptr::copy_nonoverlapping(scanline.as_ptr(), fb_ptr.offset(offset), fb.width as usize);
                }
                y += 1;
            }
        }
    }
}
