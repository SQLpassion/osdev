# Builds the KLDR16.BIN
kldr16.bin: kaosldr_entry.asm
	nasm -fbin kaosldr_entry.asm -o kldr16.bin

# Clean up
clean:
	rm -f kldr16.bin