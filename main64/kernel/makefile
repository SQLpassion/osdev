# Builds the final floppy image from which the OS can be booted.
# The .vfd file is needed for Hyper-V.
kaos64.img : bootsector.bin
	fat_imgen -c -s ../boot/bootsector.bin -f ../kaos64.img
	fat_imgen -m -f ../kaos64.img -i ../boot/hello.txt
	fat_imgen -m -f ../kaos64.img -i ../kaosldr_x16/ldr_x16.bin
	cp ../kaos64.img ../kaos64.vfd

# Builds the boot sector
bootsector.bin: ../boot/bootsector.asm
	nasm -fbin ../boot/bootsector.asm -o ../boot/bootsector.bin

# Clean up
clean:
	rm -f ../boot/bootsector.bin
	rm -f ../kaos64.img
	rm -f ../kaos64.vfd