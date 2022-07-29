# Automatically generate lists of sources using wildcards.
C_SOURCES = $(wildcard *.c)
HEADERS = $(wildcard *.h)

# Convert the *.c filenames to *.o to give a list of object files to build
OBJ = ${C_SOURCES:.c=.o}

# Links the KLDR64.BIN
kldr64.bin: kaosldr.o kernel.o ${OBJ}
	x86_64-elf-ld -o $@ -Tlink.ld $^ --oformat binary -z max-page-size=0x1000

# Compiles the KLDR64.BIN
%.o : %.c ${HEADERS}
	x86_64-elf-gcc -ffreestanding -mcmodel=large -mno-red-zone -mno-mmx -mno-sse -mno-sse2 -c $< -o $@

# Build the Kernel loader
kernel.o : kernel.asm
	nasm kernel.asm -f elf64 -o kernel.o

# Clean up
clean:
	rm -f *.bin *.o