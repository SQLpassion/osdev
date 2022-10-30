REM The following commands are building the kaos64.img file from which we can boot the OS
fat_imgen -c -s boot/bootsector.bin -f kaos64.img
fat_imgen -m -f kaos64.img -i kaosldr_16/kldr16.bin
fat_imgen -m -f kaos64.img -i kaosldr_64/kldr64.bin
fat_imgen -m -f kaos64.img -i kernel/kernel.bin