use crate::token::Token;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write as _;
use lib_kaos::console;

/// Maximum number of bytes handed to a single `WriteConsole` syscall.
///
/// Chosen to match the kernel's `MAX_CONSOLE_WRITE_LEN`; the accumulated output
/// is split into chunks of this size when flushed.
const OUTPUT_FLUSH_CHUNK: usize = 4096;

/// Batches interpreter output so that many `PRINT` statements collapse into a
/// few large `WriteConsole` syscalls instead of one syscall per line.
///
/// Every console write forces the kernel to push the affected region to VRAM
/// (framebuffer) or the VGA text buffer, and a newline that scrolls the screen
/// forces a *full-screen* copy. Flushing once per `PRINT` line therefore made
/// script output far slower than the shell's `cat`, which already batches its
/// reads into a single write per chunk. Accumulating output here restores
/// parity — a run of `PRINT`s now costs one syscall (and one screen flush) per
/// `OUTPUT_FLUSH_CHUNK` bytes rather than per line.
#[derive(Default)]
struct OutputBuffer {
    buf: Vec<u8>,
}

impl OutputBuffer {
    /// Pushes all buffered bytes to the console in `OUTPUT_FLUSH_CHUNK`-sized
    /// syscalls, then clears the buffer. A no-op when nothing is buffered.
    fn flush(&mut self) {
        let mut offset = 0;
        while offset < self.buf.len() {
            let end = (offset + OUTPUT_FLUSH_CHUNK).min(self.buf.len());
            let _ = console::writeline(&self.buf[offset..end]);
            offset = end;
        }
        self.buf.clear();
    }

    /// Flushes eagerly once a full chunk has accumulated, bounding memory use
    /// and output latency for long-running scripts.
    fn flush_if_full(&mut self) {
        if self.buf.len() >= OUTPUT_FLUSH_CHUNK {
            self.flush();
        }
    }
}

impl core::fmt::Write for OutputBuffer {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        self.buf.extend_from_slice(s.as_bytes());
        Ok(())
    }
}

fn get_variable_index(name: &str) -> Option<usize> {
    let first = name.chars().next()?;
    let c = first.to_ascii_uppercase();
    if c.is_ascii_uppercase() {
        Some((c as usize) - ('A' as usize))
    } else {
        None
    }
}

pub struct Interpreter {
    numeric_variables: [i32; 26],
    string_variables: [Option<String>; 26],
    /// Accumulates `PRINT` output for batched flushing (see [`OutputBuffer`]).
    output: OutputBuffer,
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            numeric_variables: [0; 26],
            string_variables: Default::default(),
            output: OutputBuffer::default(),
        }
    }

    /// Flushes any buffered `PRINT` output to the console.
    ///
    /// The interactive REPL calls this after each executed line so that command
    /// output appears immediately; [`execute_script`](Self::execute_script)
    /// flushes once at the end of a run.
    pub fn flush_output(&mut self) {
        self.output.flush();
    }

    /// Executes multiple lines of BASIC code from a script content string.
    pub fn execute_script(&mut self, script: &str) {
        for line in script.lines() {
            let line_trimmed = line.trim();
            if !line_trimmed.is_empty() {
                let tokens = crate::token::tokenize_line(line_trimmed);
                self.execute(&tokens);
            }
        }
        // Emit any output still buffered from the final `PRINT` statements.
        self.output.flush();
    }

    fn eval_expression(&self, tokens: &[Token], index: &mut usize) -> i32 {
        if *index >= tokens.len() {
            return 0;
        }
        match &tokens[*index] {
            Token::Number(val) => {
                *index += 1;
                *val
            }
            Token::Identifier(name) => {
                *index += 1;
                if let Some(idx) = get_variable_index(name) {
                    self.numeric_variables[idx]
                } else {
                    0
                }
            }
            _ => 0,
        }
    }

    pub fn execute(&mut self, tokens: &[Token]) {
        let mut index = 0;
        if index >= tokens.len() {
            return;
        }

        match &tokens[index] {
            Token::Let => {
                index += 1;
                if index < tokens.len() {
                    if let Token::Identifier(name) = &tokens[index] {
                        let is_string = name.ends_with('$');
                        let var_name = name.clone();
                        index += 1;
                        if index < tokens.len() && tokens[index] == Token::Equals {
                            index += 1;
                            if is_string {
                                if index < tokens.len() {
                                    if let Token::StringLiteral(s) = &tokens[index] {
                                        if let Some(idx) = get_variable_index(&var_name) {
                                            self.string_variables[idx] = Some(s.clone());
                                        }
                                    }
                                }
                            } else {
                                let val = self.eval_expression(tokens, &mut index);
                                if let Some(idx) = get_variable_index(&var_name) {
                                    self.numeric_variables[idx] = val;
                                }
                            }
                        }
                    }
                }
            }
            Token::Print => {
                index += 1;
                if index < tokens.len() {
                    match &tokens[index] {
                        Token::Identifier(name) => {
                            let is_string = name.ends_with('$');
                            if let Some(idx) = get_variable_index(name) {
                                if is_string {
                                    if let Some(val) = &self.string_variables[idx] {
                                        let _ = writeln!(self.output, "{}", val);
                                    } else {
                                        let _ = writeln!(self.output);
                                    }
                                } else {
                                    let _ = writeln!(self.output, "{}", self.numeric_variables[idx]);
                                }
                            }
                        }
                        Token::StringLiteral(s) => {
                            let _ = writeln!(self.output, "{}", s);
                        }
                        Token::Number(_) => {
                            let val = self.eval_expression(tokens, &mut index);
                            let _ = writeln!(self.output, "{}", val);
                        }
                        _ => {}
                    }
                    // Keep the buffer bounded during long output-heavy scripts.
                    self.output.flush_if_full();
                }
            }
            Token::If => {
                index += 1;
                let left = self.eval_expression(tokens, &mut index);
                if index < tokens.len()
                    && (tokens[index] == Token::Greater || tokens[index] == Token::Less)
                {
                    let op = tokens[index].clone();
                    index += 1;
                    let right = self.eval_expression(tokens, &mut index);
                    if index < tokens.len() && tokens[index] == Token::Then {
                        index += 1;
                        let condition = match op {
                            Token::Greater => left > right,
                            Token::Less => left < right,
                            _ => false,
                        };
                        if condition {
                            // Execute remaining tokens starting from `index`
                            self.execute(&tokens[index..]);
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
#[path = "tests/interpreter.rs"]
mod tests;
