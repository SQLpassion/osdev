# Builds the final floppy image from which the OS can be booted
 kaosimg : bootsector.bin
	fat_imgen -c -s bootsector.bin -f kaos.img

# Builds the boot sector
bootsector.bin: $(ASM)
	nasm -fbin $(ASM) -o bootsector.bin

# Clean up
clean:
	rm -f bootsector.bin
	rm -f kaos.img