# Change CRLF to LF in case when Windows has changed it...
find . -type f \( -name "*.sh" \) -exec sed -i 's/\r$//' {} \;

# Builds the boot sector
cd /src/tutorials/001-bootsectors-barebones
make clean && make ASM=$1