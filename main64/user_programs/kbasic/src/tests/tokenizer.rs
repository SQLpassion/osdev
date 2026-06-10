use super::*;

#[test]
fn test_tokenize_basic_keywords() {
    let tokens = tokenize_line("LET PRINT IF THEN");
    assert_eq!(tokens[0], Token::Let);
    assert_eq!(tokens[1], Token::Print);
    assert_eq!(tokens[2], Token::If);
    assert_eq!(tokens[3], Token::Then);
    assert_eq!(tokens[4], Token::Eof);
}

#[test]
fn test_tokenize_keywords_case_insensitivity() {
    let tokens = tokenize_line("let Print iF tHeN");
    assert_eq!(tokens[0], Token::Let);
    assert_eq!(tokens[1], Token::Print);
    assert_eq!(tokens[2], Token::If);
    assert_eq!(tokens[3], Token::Then);
    assert_eq!(tokens[4], Token::Eof);
}

#[test]
fn test_tokenize_identifiers() {
    let tokens = tokenize_line("A B$ X1 Y2$");
    assert_eq!(tokens[0], Token::Identifier(String::from("A")));
    assert_eq!(tokens[1], Token::Identifier(String::from("B$")));
    assert_eq!(tokens[2], Token::Identifier(String::from("X1")));
    assert_eq!(tokens[3], Token::Identifier(String::from("Y2$")));
    assert_eq!(tokens[4], Token::Eof);
}

#[test]
fn test_tokenize_numbers() {
    let tokens = tokenize_line("0 42 9999");
    assert_eq!(tokens[0], Token::Number(0));
    assert_eq!(tokens[1], Token::Number(42));
    assert_eq!(tokens[2], Token::Number(9999));
    assert_eq!(tokens[3], Token::Eof);
}

#[test]
fn test_tokenize_strings() {
    let tokens = tokenize_line("\"\" \"hello\" \"hello world!\"");
    assert_eq!(tokens[0], Token::StringLiteral(String::from("")));
    assert_eq!(tokens[1], Token::StringLiteral(String::from("hello")));
    assert_eq!(tokens[2], Token::StringLiteral(String::from("hello world!")));
    assert_eq!(tokens[3], Token::Eof);
}

#[test]
fn test_tokenize_unterminated_string() {
    // If quote is not closed, it consumes until end of line
    let tokens = tokenize_line("\"unterminated string");
    assert_eq!(tokens[0], Token::StringLiteral(String::from("unterminated string")));
    assert_eq!(tokens[1], Token::Eof);
}

#[test]
fn test_tokenize_operators() {
    let tokens = tokenize_line("= > <");
    assert_eq!(tokens[0], Token::Equals);
    assert_eq!(tokens[1], Token::Greater);
    assert_eq!(tokens[2], Token::Less);
    assert_eq!(tokens[3], Token::Eof);
}

#[test]
fn test_tokenize_unknown_chars() {
    // Unknown characters (like #, @, etc.) should be skipped/ignored
    let tokens = tokenize_line("LET A = # 42 @");
    assert_eq!(tokens[0], Token::Let);
    assert_eq!(tokens[1], Token::Identifier(String::from("A")));
    assert_eq!(tokens[2], Token::Equals);
    assert_eq!(tokens[3], Token::Number(42));
    assert_eq!(tokens[4], Token::Eof);
}

#[test]
fn test_tokenize_whitespace() {
    let tokens = tokenize_line("   LET \t A   =   5   ");
    assert_eq!(tokens[0], Token::Let);
    assert_eq!(tokens[1], Token::Identifier(String::from("A")));
    assert_eq!(tokens[2], Token::Equals);
    assert_eq!(tokens[3], Token::Number(5));
    assert_eq!(tokens[4], Token::Eof);
}
