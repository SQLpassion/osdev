# Automatically generate lists of sources using wildcards.
C_SOURCES = $(wildcard *.c syscalls/*.c libc/*.c)
HEADERS = $(wildcard *.h syscalls/*.h libc/*.h)

# Convert the *.c filenames to *.o to give a list of object files to build
OBJ = ${C_SOURCES:.c=.o}

# Links the C program
prog1.bin: program.o syscalls/syscall_asm.o ${OBJ}
	x86_64-elf-ld -o $@ -Tlink.ld $^ --oformat binary -z max-page-size=0x1000 -Map prog1.map
	x86_64-elf-objdump -M intel -S --disassemble program.o > program.generated
	
# Compiles the C program
%.o : %.c ${HEADERS}
	x86_64-elf-gcc -ffreestanding -mcmodel=large -mno-red-zone -mno-mmx -mno-sse -mno-sse2 -c $< -o $@

# Builds the SYSCALL functionality written in Assembler
syscalls/syscall_asm.o : syscalls/syscall.asm
	nasm -felf64 syscalls/syscall.asm -o syscalls/syscall_asm.o

# Clean up
clean:
	rm -f *.bin *.o *.generated syscalls/*.o common/*.o syscalls/*.generated