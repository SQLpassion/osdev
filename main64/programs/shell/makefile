# Automatically generate lists of sources using wildcards.
C_SOURCES = $(wildcard *.c ../../libc/*.c)
HEADERS = $(wildcard *.h ../../libc/*.h)

# Convert the *.c filenames to *.o to give a list of object files to build
OBJ = ${C_SOURCES:.c=.o}

# Links the C program
shell.bin: shell.o ../../libc/syscall_asm.o ${OBJ}
	x86_64-elf-ld -o $@ -Tlink.ld $^ --oformat binary -z max-page-size=0x1000 -Map shell.map
	x86_64-elf-objdump -M intel -S --disassemble shell.o > shell.generated
	
# Compiles the C program
%.o : %.c ${HEADERS}
	x86_64-elf-gcc -ffreestanding -mcmodel=large -mno-red-zone -mno-mmx -mno-sse -mno-sse2 -c $< -o $@

# Builds the SYSCALL functionality written in Assembler
../../libc/syscall_asm.o : ../../libc/syscall.asm
	nasm -felf64 ../../libc/syscall.asm -o ../../libc/syscall_asm.o

# Clean up
clean:
	rm -f *.bin *.o *.generated ../../libc/*.o common/*.o ../../libc/*.generated