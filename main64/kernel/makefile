# Automatically generate lists of sources using wildcards.
C_SOURCES = $(wildcard *.c drivers/*.c isr/*.c memory/*.c multitasking/*.c)
HEADERS = $(wildcard *.h drivers/*.h isr/*.h memory/*.h multitasking/*.h)

# Convert the *.c filenames to *.o to give a list of object files to build
OBJ = ${C_SOURCES:.c=.o}

# Builds the final floppy image from which the OS can be booted.
kaos64.img : bootsector.bin kernel.bin
	fat_imgen -c -s ../boot/bootsector.bin -f ../kaos64.img
	fat_imgen -m -f ../kaos64.img -i ../kaosldr_16/kldr16.bin
	fat_imgen -m -f ../kaos64.img -i ../kaosldr_64/kldr64.bin
	fat_imgen -m -f ../kaos64.img -i ../kernel/kernel.bin
	x86_64-elf-objdump -M intel -S --disassemble kernel.o > kernel.generated
	x86_64-elf-objdump -M intel -S --disassemble drivers/screen.o > drivers/screen.generated

# Builds the boot sector
bootsector.bin: ../boot/bootsector.asm
	nasm -fbin ../boot/bootsector.asm -o ../boot/bootsector.bin

# Builds the ISR handlers written in Assembler
isr/idt_asm.o : isr/idt.asm
	nasm -felf64 isr/idt.asm -o isr/idt_asm.o

# Builds the IRQ handlers written in Assembler
isr/irq_asm.o : isr/irq.asm
	nasm -felf64 isr/irq.asm -o isr/irq_asm.o

# Builds the Task Switching functionality written in Assembler
multitasking/contextswitching.o : multitasking/contextswitching.asm
	nasm -felf64 multitasking/contextswitching.asm -o multitasking/contextswitching.o

# Compiles the C kernel
%.o : %.c ${HEADERS}
	x86_64-elf-gcc -ffreestanding -mcmodel=large -mno-red-zone -mno-mmx -mno-sse -mno-sse2 -c $< -o $@

# Links the C kernel
# The file "kernel.o" is specified explicitly, so that it is the first part of the file "kernel.bin"
kernel.bin: kernel.o isr/idt_asm.o isr/irq_asm.o multitasking/contextswitching.o ${OBJ}
	x86_64-elf-ld -o $@ -Tlink.ld $^ --oformat binary -z max-page-size=0x1000 -Map kernel.map

# Clean up
clean:
	rm -f ../boot/bootsector.bin
	rm -f ../kaos64.img
	rm -f *.o *.map *.bin
	rm -f drivers/*.o
	rm -f isr/*.o
	rm -f memory/*.o
	rm -f multitasking/*.o
	rm -f *.generated
