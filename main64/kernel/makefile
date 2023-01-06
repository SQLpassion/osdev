# Automatically generate lists of sources using wildcards.
C_SOURCES = $(wildcard *.c)
HEADERS = $(wildcard *.h)

# Builds the final floppy image from which the OS can be booted.
kaos64.img : bootsector.bin kernel.bin
	fat_imgen -c -s ../boot/bootsector.bin -f ../kaos64.img
	fat_imgen -m -f ../kaos64.img -i ../kaosldr_16/kldr16.bin
	fat_imgen -m -f ../kaos64.img -i ../kaosldr_64/kldr64.bin
	fat_imgen -m -f ../kaos64.img -i ../kernel/kernel.bin
	# x86_64-elf-objdump -M intel -S --disassemble kernel.o > kernel_generated.asm

# Builds the boot sector
bootsector.bin: ../boot/bootsector.asm
	nasm -fbin ../boot/bootsector.asm -o ../boot/bootsector.bin

# Links the C kernel
kernel.bin: kernel.o common.o drivers/screen.o ${OBJ}
	x86_64-elf-ld -o $@ -Tlink.ld $^ --oformat binary -z max-page-size=0x1000 -Map kernel.map

# Compiles the C kernel
%.o : %.c ${HEADERS}
	x86_64-elf-gcc -ffreestanding -mcmodel=large -mno-red-zone -mno-mmx -mno-sse -mno-sse2 -c $< -o $@

# Clean up
clean:
	rm -f ../boot/bootsector.bin
	rm -f ../kaos64.img
	rm -f *.o *.map *.bin
	rm -f drivers/*.o