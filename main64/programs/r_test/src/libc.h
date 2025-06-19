#ifndef LIBC_H
#define LIBC_H

// Defines the various available SysCalls
#define SYSCALL_PRINTF              1
#define SYSCALL_GETPID              2
#define SYSCALL_TERMINATE_PROCESS   3
#define SYSCALL_GETCHAR             4
#define SYSCALL_GETCURSOR           5
#define SYSCALL_SETCURSOR           6
#define SYSCALL_EXECUTE             7
#define SYSCALL_PRINTROOTDIRECTORY  8
#define SYSCALL_CLEARSCREEN         9
#define SYSCALL_OPENFILE            10
#define SYSCALL_READFILE            11
#define SYSCALL_WRITEFILE           12
#define SYSCALL_SEEKFILE            13
#define SYSCALL_ENDOFFILE           14
#define SYSCALL_CLOSEFILE           15
#define SYSCALL_DELETEFILE          16

#define KEY_RETURN      '\r'
#define KEY_BACKSPACE   '\b'

// SYSCALL with no arguments
static inline long SYSCALL0(int number)
{
    long result;
    __asm__ volatile (
        "int $0x80"
        : "=a"(result)
        : "D"(number)  // syscall number in RDI (per C calling convention)
        : "memory"
    );
    return result;
}

// SYSCALL with 1 argument
static inline long SYSCALL1(int number, void *arg1)
{
    long result;
    __asm__ volatile (
        "int $0x80"
        : "=a"(result)
        : "D"(number), "S"(arg1)
        : "memory"
    );
    return result;
}

#endif