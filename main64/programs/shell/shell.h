#ifndef PROGRAM_H
#define PROGRAM_H

// The number of available commands
#define COMMAND_COUNT 6

// The main entry point for the User Mode program.
void ShellMain();

int shell_cls(char *param);
int shell_dir(char *param);
int shell_mkfile(char *param);
int shell_type(char *param);
int shell_del(char *param);
int shell_open(char *param);

#endif