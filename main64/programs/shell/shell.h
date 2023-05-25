#ifndef PROGRAM_H
#define PROGRAM_H

// The number of available commands
#define COMMAND_COUNT 3

// The main entry point for the User Mode program.
void ShellMain();

void shell_cls(char *param);
void shell_ver(char *param);
void shell_dir(char *param);

#endif