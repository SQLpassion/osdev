use alloc::string::String;
use lib_kaos::println;
use crate::token::Token;

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
}

impl Interpreter {
    pub fn new() -> Self {
        Self {
            numeric_variables: [0; 26],
            string_variables: Default::default(),
        }
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
                                        println!("{}", val);
                                    } else {
                                        println!();
                                    }
                                } else {
                                    println!("{}", self.numeric_variables[idx]);
                                }
                            }
                        }
                        Token::StringLiteral(s) => {
                            println!("{}", s);
                        }
                        Token::Number(_) => {
                            let val = self.eval_expression(tokens, &mut index);
                            println!("{}", val);
                        }
                        _ => {}
                    }
                }
            }
            Token::If => {
                index += 1;
                let left = self.eval_expression(tokens, &mut index);
                if index < tokens.len() && tokens[index] == Token::Greater {
                    index += 1;
                    let right = self.eval_expression(tokens, &mut index);
                    if index < tokens.len() && tokens[index] == Token::Then {
                        index += 1;
                        if left > right {
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
