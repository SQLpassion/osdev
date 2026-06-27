//! Dialog widget — modal overlay text dialog box.
//!
//! A self-contained widget designed to be rendered on top of existing screen contents.
//! It draws a boxed frame with a title, a set of formatted text lines, and a bottom
//! action hint (e.g., "[ Press ENTER/Esc to close ]").

extern crate alloc;
use crate::screen::{with_screen, Color};
use alloc::borrow::Cow;
use alloc::vec::Vec;

/// Default foreground color of the dialog text.
const TEXT_FG: Color = Color::White;
/// Default background color of the dialog.
const DIALOG_BG: Color = Color::Black;
/// Border and title highlight color.
const BORDER_FG: Color = Color::LightCyan;

/// A reusable modal overlay dialog widget.
pub struct Dialog {
    /// Zero-based vertical screen row where the dialog box starts.
    row: usize,
    /// Zero-based horizontal screen column where the dialog box starts.
    col: usize,
    /// Width of the dialog box in columns.
    width: usize,
    /// Height of the dialog box in rows.
    height: usize,
    /// Dialog window title.
    title: Cow<'static, str>,
    /// Formatted content lines.
    lines: Vec<Cow<'static, str>>,
}

impl Dialog {
    /// Creates a new Dialog widget.
    pub fn new<T, L>(
        row: usize,
        col: usize,
        width: usize,
        height: usize,
        title: T,
        lines: Vec<L>,
    ) -> Self
    where
        T: Into<Cow<'static, str>>,
        L: Into<Cow<'static, str>>,
    {
        let title = title.into();
        let lines = lines.into_iter().map(|l| l.into()).collect();
        Self {
            row,
            col,
            width,
            height,
            title,
            lines,
        }
    }

    /// Renders the dialog box onto the active screen.
    pub fn draw(&self) {
        with_screen(|screen| {
            // Step 1: Draw the solid background.
            screen.fill_rect(
                self.row,
                self.col,
                self.width,
                self.height,
                b' ',
                TEXT_FG,
                DIALOG_BG,
            );

            // Step 2: Draw the box border using double-line characters.
            // Horizontal lines
            for c in 0..self.width {
                screen.draw_char_at(self.row, self.col + c, 0xCD, BORDER_FG, DIALOG_BG); // Double horizontal line ═
                screen.draw_char_at(
                    self.row + self.height - 1,
                    self.col + c,
                    0xCD,
                    BORDER_FG,
                    DIALOG_BG,
                );
            }
            // Vertical lines
            for r in 0..self.height {
                screen.draw_char_at(self.row + r, self.col, 0xBA, BORDER_FG, DIALOG_BG); // Double vertical line ║
                screen.draw_char_at(
                    self.row + r,
                    self.col + self.width - 1,
                    0xBA,
                    BORDER_FG,
                    DIALOG_BG,
                );
            }
            // Corners
            screen.draw_char_at(self.row, self.col, 0xC9, BORDER_FG, DIALOG_BG); // Top-left ╔
            screen.draw_char_at(
                self.row,
                self.col + self.width - 1,
                0xBB,
                BORDER_FG,
                DIALOG_BG,
            ); // Top-right ╗
            screen.draw_char_at(
                self.row + self.height - 1,
                self.col,
                0xC8,
                BORDER_FG,
                DIALOG_BG,
            ); // Bottom-left ╚
            screen.draw_char_at(
                self.row + self.height - 1,
                self.col + self.width - 1,
                0xBC,
                BORDER_FG,
                DIALOG_BG,
            ); // Bottom-right ╝

            // Step 3: Draw title centered on the top border.
            let title_len = self.title.len();
            if title_len + 4 <= self.width {
                let title_col = self.col + (self.width - title_len) / 2;
                // Draw decorative spacers around the title using raw CP437 single-byte codes: e.g. "╡ Title ╞"
                screen.draw_char_at(self.row, title_col - 2, 0xB5, BORDER_FG, DIALOG_BG); // ╡
                screen.draw_char_at(self.row, title_col - 1, b' ', BORDER_FG, DIALOG_BG);
                screen.draw_at(self.row, title_col, &self.title, TEXT_FG, DIALOG_BG);
                screen.draw_char_at(self.row, title_col + title_len, b' ', BORDER_FG, DIALOG_BG);
                screen.draw_char_at(
                    self.row,
                    title_col + title_len + 1,
                    0xC6,
                    BORDER_FG,
                    DIALOG_BG,
                ); // ╞
            }

            // Step 4: Draw content lines.
            let visible_height = self.height.saturating_sub(4); // Exclude border and footer space
            for (i, line) in self.lines.iter().take(visible_height).enumerate() {
                let r = self.row + 2 + i;
                let c = self.col + 2;
                let max_len = self.width.saturating_sub(4);
                let line_str = if line.len() > max_len {
                    &line[..max_len]
                } else {
                    line
                };
                screen.draw_at(r, c, line_str, TEXT_FG, DIALOG_BG);
            }

            // Step 5: Draw bottom action hint centered on the bottom border.
            let footer = "[ Press ENTER/Esc to close ]";
            let footer_len = footer.len();
            if footer_len + 4 <= self.width {
                let footer_row = self.row + self.height - 1;
                let footer_col = self.col + (self.width - footer_len) / 2;
                screen.draw_at(footer_row, footer_col, footer, BORDER_FG, DIALOG_BG);
            }
        });
    }
}
