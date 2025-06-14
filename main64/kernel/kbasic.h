#ifndef KBASIC_H
#define KBASIC_H

#define MAX_TOKENS 100
#define MAX_LINES  100
#define MAX_LINE_LENGTH 128

typedef enum
{
    TOKEN_LET, 
    TOKEN_PRINT, 
    TOKEN_IF, 
    TOKEN_THEN,
    TOKEN_IDENTIFIER, 
    TOKEN_NUMBER,
    TOKEN_STRING,
    TOKEN_EQUALS, 
    TOKEN_GREATER,
    TOKEN_END, 
    TOKEN_EOF, 
    TOKEN_UNKNOWN
} TokenType;

typedef struct
{
    TokenType type;
    char text[32];
} Token;

Token tokenize_single(const char** src);

int tokenize_line(const char* src, Token tokens[], int max);

void execute_tokens(Token tokens[]);

int eval_expression(Token** curtok);

int get_variable_index(char name);

#endif