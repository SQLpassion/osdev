#include "common.h"
#include "kbasic.h"
#include "drivers/screen.h"

// A - Z
int numeric_variables[26];

// A$ - Z$
char *string_variables[26];

Token tokenize_single(const char** src)
{
    // Skip whitespaces
    while (**src == ' ' || **src == '\t') (*src)++;

    Token token = { TOKEN_UNKNOWN, {0} };

    if (**src == '\0' || **src == '\n')
    {
        token.type = TOKEN_EOF;
        return token;
    }

    if (strncmp(*src, "LET", 3) == 0 && !isalnum((*src)[3]))
    {
        *src += 3;
        token.type = TOKEN_LET;
        return token;
    }
    else if (strncmp(*src, "PRINT", 5) == 0 && !isalnum((*src)[5]))
    {
        *src += 5;
        token.type = TOKEN_PRINT;
        return token;
    } 
    else if (strncmp(*src, "IF", 2) == 0 && !isalnum((*src)[2]))
    {
        *src += 2;
        token.type = TOKEN_IF;
        return token;
    } 
    else if (strncmp(*src, "THEN", 4) == 0 && !isalnum((*src)[4])) 
    {
        *src += 4;
        token.type = TOKEN_THEN;
        return token;
    }

    if (**src == '=')
    { 
        (*src)++; 
        token.type = TOKEN_EQUALS; 
        return token; 
    }

    if (**src == '>') 
    { 
        (*src)++; 
        token.type = TOKEN_GREATER; 
        return token;
    }

    // Parse string literals
    if (**src == '"')
    {
        (*src)++;  // skip opening quote
        int i = 0;

        while (**src && **src != '"' && i < 31)
        {
            token.text[i++] = *(*src);
            (*src)++;
        }

        token.text[i] = '\0';

        if (**src == '"') (*src)++;  // skip closing quote
        token.type = TOKEN_STRING;

        return token;
    }

    // Identifier (A–Z)
    if (isalpha(**src))
    {
        token.type = TOKEN_IDENTIFIER;
        int i = 0;

        // First character must be a letter
        token.text[i++] = *(*src);
        (*src)++;

        // Check for trailing '$'
        if (**src == '$')
        {
            token.text[i++] = '$';
            (*src)++;
        }

        token.text[i] = '\0';

        toupper(token.text);
        return token;
    }

    // Number
    if (isdigit(**src))
    {
        token.type = TOKEN_NUMBER;
        int i = 0;

        while (isdigit(**src) && i < 31)
        {
            token.text[i++] = *(*src);
            (*src)++;
        }

        token.text[i] = '\0';
        return token;
    }

    // Unknown token
    (*src)++;

    return token;
}

int tokenize_line(const char* src, Token tokens[], int max)
{
    int count = 0;

    while (*src && count < max)
    {
        Token token = tokenize_single(&src);
        if (token.type == TOKEN_EOF) break;
        tokens[count++] = token;
    }

    tokens[count].type = TOKEN_EOF;
    return count;
}

void execute_tokens(Token tokens[])
{
    Token* current_token = tokens;

    // Examples:
    // => LET A = 5
    // => LET C$ = "Test Message"
    if (current_token->type == TOKEN_LET)
    {
        current_token++;

        if (current_token->type == TOKEN_IDENTIFIER)
        {
            char varname = current_token->text[0];
            int is_string = (current_token->text[1] == '$');
            current_token++;

            if (current_token->type == TOKEN_EQUALS)
            {
                current_token++;

                if (is_string && current_token->type == TOKEN_STRING)
                {
                    int idx = get_variable_index(varname);
                    string_variables[idx] = strdup(current_token->text);
                    current_token++;
                } 
                else if (!is_string)
                {
                    int val = eval_expression(&current_token);
                    numeric_variables[get_variable_index(varname)] = val;
                }
            }
        }
    }
    else if (current_token->type == TOKEN_PRINT)
    {
        current_token++;

        if (current_token->type == TOKEN_IDENTIFIER)
        {
            char varname = current_token->text[0];
            int is_string = (current_token->text[1] == '$');

            if (is_string)
            {
                char *val = string_variables[get_variable_index(varname)];

                if (val)
                {
                    int oldColor = SetColor(COLOR_GREEN);
                    printf(val);
                    printf("\n");
                    SetColor(oldColor);
                }
            } 
            else 
            {
                int oldColor = SetColor(COLOR_GREEN);
                printf_int(numeric_variables[get_variable_index(varname)], 10);
                printf("\n");
                SetColor(oldColor);
            }

            current_token++;
        }
        else if (current_token->type == TOKEN_STRING)
        {
            int oldColor = SetColor(COLOR_GREEN);
            printf(current_token->text);
            printf("\n");
            current_token++;
            SetColor(oldColor);
        }
        else if (current_token->type == TOKEN_NUMBER)
        {
            // Evaluate the expression and print out the variable
            int val = eval_expression(&current_token);
            int oldColor = SetColor(COLOR_GREEN);
            printf_int(val, 10);
            printf("\n");
            SetColor(oldColor);
            current_token++;
        }
    }
    // Examples:
    // => IF A > 3 THEN PRINT 42
    // => IF A > 3 THEN PRINT B
    // => IF A > 3 THEN PRINT "Test Message"
    else if (current_token->type == TOKEN_IF)
    {
        // Move to the next token that contains the expression to evaluate
        current_token++;

        // Evaluate the left expression and move to the next token
        int left = eval_expression(&current_token);

        // Token ">"
        if (current_token->type == TOKEN_GREATER)
        {
            // Move to the next token
            current_token++;

            // Evaluate the right expression and move to the next token
            int right = eval_expression(&current_token);

            if (current_token->type == TOKEN_THEN)
            {
                // Move to the next token
                current_token++;

                // Perform the expression
                if (left > right)
                {
                    // Execute the remaining tokens
                    execute_tokens(current_token);
                }
            }
        }
    }
}

int eval_expression(Token** curtok)
{
    Token* tok = *curtok;

    if (tok->type == TOKEN_NUMBER)
    {
        int val = atoi(tok->text);
        *curtok = tok + 1;

        return val;
    }

    if (tok->type == TOKEN_IDENTIFIER)
    {
        int index = get_variable_index(tok->text[0]);
        *curtok = tok + 1;

        return numeric_variables[index];
    }

    return 0;
}

int get_variable_index(char name)
{
    char str[2] = { name, '\0' }; 
    toupper(str); 

    return str[0] - 'A';
}