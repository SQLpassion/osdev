#ifndef PROGRAM_H
#define PROGRAM_H

// The main entry point for the User Mode program.
void ProgramMain();

// This function triggers a GP fault, because the out instruction is not allowed in Ring 3 codee.
void outb(unsigned short Port, unsigned char Value);

#endif