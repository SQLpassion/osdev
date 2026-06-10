use alloc::string::String;
use alloc::vec::Vec;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Token {
    Let,
    Print,
    If,
    Then,
    Identifier(String),
    Number(i32),
    StringLiteral(String),
    Equals,
    Greater,
    Less,
    Eof,
}

pub fn tokenize_line(line: &str) -> Vec<Token> {
    let mut tokens = Vec::new();
    let chars: Vec<char> = line.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        // Skip whitespace
        if chars[i].is_whitespace() {
            i += 1;
            continue;
        }

        // Keywords & Identifiers
        if chars[i].is_alphabetic() {
            let mut s = String::new();
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '$') {
                s.push(chars[i]);
                i += 1;
            }
            // Check keywords (case-insensitive)
            let s_upper = s.to_uppercase();
            match s_upper.as_str() {
                "LET" => tokens.push(Token::Let),
                "PRINT" => tokens.push(Token::Print),
                "IF" => tokens.push(Token::If),
                "THEN" => tokens.push(Token::Then),
                _ => tokens.push(Token::Identifier(s_upper)),
            }
            continue;
        }

        // Numbers
        if chars[i].is_ascii_digit() {
            let mut val = 0;
            while i < chars.len() && chars[i].is_ascii_digit() {
                val = val * 10 + (chars[i] as i32 - '0' as i32);
                i += 1;
            }
            tokens.push(Token::Number(val));
            continue;
        }

        // String literals
        if chars[i] == '"' {
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                s.push(chars[i]);
                i += 1;
            }
            if i < chars.len() && chars[i] == '"' {
                i += 1; // skip closing quote
            }
            tokens.push(Token::StringLiteral(s));
            continue;
        }

        // Equals
        if chars[i] == '=' {
            tokens.push(Token::Equals);
            i += 1;
            continue;
        }

        // Greater than
        if chars[i] == '>' {
            tokens.push(Token::Greater);
            i += 1;
            continue;
        }

        // Less than
        if chars[i] == '<' {
            tokens.push(Token::Less);
            i += 1;
            continue;
        }

        // Unknown character
        i += 1;
    }
    tokens.push(Token::Eof);
    tokens
}

#[cfg(test)]
#[path = "tests/tokenizer.rs"]
mod tests;
