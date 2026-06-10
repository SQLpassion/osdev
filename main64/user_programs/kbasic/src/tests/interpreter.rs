use super::*;
use crate::token::tokenize_line;

#[test]
fn test_tokenize_less_than() {
    let tokens = tokenize_line("IF A < 10 THEN PRINT A");
    assert_eq!(tokens[0], Token::If);
    assert_eq!(tokens[1], Token::Identifier(String::from("A")));
    assert_eq!(tokens[2], Token::Less);
    assert_eq!(tokens[3], Token::Number(10));
    assert_eq!(tokens[4], Token::Then);
    assert_eq!(tokens[5], Token::Print);
}

#[test]
fn test_interpreter_less_than() {
    let mut interpreter = Interpreter::new();
    
    // Execute LET A = 5
    let tokens_let = tokenize_line("LET A = 5");
    interpreter.execute(&tokens_let);
    assert_eq!(interpreter.numeric_variables[0], 5);

    // Execute IF A < 10 THEN LET B = 1 (Condition is true, B should become 1)
    let tokens_if_true = tokenize_line("IF A < 10 THEN LET B = 1");
    interpreter.execute(&tokens_if_true);
    assert_eq!(interpreter.numeric_variables[1], 1);

    // Execute IF A < 3 THEN LET C = 1 (Condition is false, C should remain 0)
    let tokens_if_false = tokenize_line("IF A < 3 THEN LET C = 1");
    interpreter.execute(&tokens_if_false);
    assert_eq!(interpreter.numeric_variables[2], 0);
}

#[test]
fn test_tokenize_all_tokens() {
    let tokens = tokenize_line("LET x$ = \"hello\" PRINT IF THEN = > < 123");
    assert_eq!(tokens[0], Token::Let);
    assert_eq!(tokens[1], Token::Identifier(String::from("X$")));
    assert_eq!(tokens[2], Token::Equals);
    assert_eq!(tokens[3], Token::StringLiteral(String::from("hello")));
    assert_eq!(tokens[4], Token::Print);
    assert_eq!(tokens[5], Token::If);
    assert_eq!(tokens[6], Token::Then);
    assert_eq!(tokens[7], Token::Equals);
    assert_eq!(tokens[8], Token::Greater);
    assert_eq!(tokens[9], Token::Less);
    assert_eq!(tokens[10], Token::Number(123));
    assert_eq!(tokens[11], Token::Eof);
}

#[test]
fn test_tokenize_case_insensitivity() {
    let tokens = tokenize_line("let a = 10 print a");
    assert_eq!(tokens[0], Token::Let);
    assert_eq!(tokens[1], Token::Identifier(String::from("A")));
    assert_eq!(tokens[2], Token::Equals);
    assert_eq!(tokens[3], Token::Number(10));
    assert_eq!(tokens[4], Token::Print);
    assert_eq!(tokens[5], Token::Identifier(String::from("A")));
}

#[test]
fn test_interpreter_let_numeric() {
    let mut interpreter = Interpreter::new();
    // Variable starts at 0
    assert_eq!(interpreter.numeric_variables[0], 0);

    // Simple assignment
    interpreter.execute(&tokenize_line("LET A = 42"));
    assert_eq!(interpreter.numeric_variables[0], 42);

    // Assign from variable
    interpreter.execute(&tokenize_line("LET B = A"));
    assert_eq!(interpreter.numeric_variables[1], 42);

    // Case insensitivity
    interpreter.execute(&tokenize_line("let z = 99"));
    assert_eq!(interpreter.numeric_variables[25], 99);
}

#[test]
fn test_interpreter_let_string() {
    let mut interpreter = Interpreter::new();
    assert_eq!(interpreter.string_variables[0], None);

    // String assignment
    interpreter.execute(&tokenize_line("LET A$ = \"hello world\""));
    assert_eq!(interpreter.string_variables[0], Some(String::from("hello world")));

    // String variables are indexed by the starting letter
    interpreter.execute(&tokenize_line("LET Z$ = \"end\""));
    assert_eq!(interpreter.string_variables[25], Some(String::from("end")));
}

#[test]
fn test_interpreter_if_greater_than() {
    let mut interpreter = Interpreter::new();
    
    interpreter.execute(&tokenize_line("LET A = 10"));
    interpreter.execute(&tokenize_line("LET B = 5"));

    // 10 > 5 is true
    interpreter.execute(&tokenize_line("IF A > B THEN LET C = 100"));
    assert_eq!(interpreter.numeric_variables[2], 100);

    // 5 > 10 is false
    interpreter.execute(&tokenize_line("IF B > A THEN LET D = 200"));
    assert_eq!(interpreter.numeric_variables[3], 0);
}

#[test]
fn test_interpreter_nested_if() {
    let mut interpreter = Interpreter::new();
    interpreter.execute(&tokenize_line("LET A = 10"));
    interpreter.execute(&tokenize_line("LET B = 5"));

    // Nested true condition
    interpreter.execute(&tokenize_line("IF A > 5 THEN IF B < 10 THEN LET C = 1"));
    assert_eq!(interpreter.numeric_variables[2], 1);

    // Nested false condition
    interpreter.execute(&tokenize_line("IF A > 5 THEN IF B > 10 THEN LET D = 1"));
    assert_eq!(interpreter.numeric_variables[3], 0);
}

#[test]
fn test_interpreter_print_execution() {
    // Test that execution of print variants doesn't panic/error
    let mut interpreter = Interpreter::new();
    interpreter.execute(&tokenize_line("LET A = 12"));
    interpreter.execute(&tokenize_line("LET A$ = \"test\""));

    interpreter.execute(&tokenize_line("PRINT A"));
    interpreter.execute(&tokenize_line("PRINT A$"));
    interpreter.execute(&tokenize_line("PRINT \"literal\""));
    interpreter.execute(&tokenize_line("PRINT 456"));
}
