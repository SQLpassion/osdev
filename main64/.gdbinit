set substitute-path /src/main64 /Users/klaus/dev/GitHub/SQLpassion/osdev/main64
set architecture i386:x86-64
set disassembly-flavor intel

file kernel/kernel.elf
target remote localhost:1234
add-symbol-file kernel/kernel.elf 0xFFFF800000100000
add-symbol-file programs/shell/shell.elf 0x0000700000000000