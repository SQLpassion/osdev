# Change CRLF to LF in case when Windows has changed it...
find . -type f \( -name "*.sh" \) -exec sed -i 's/\r$//' {} \;

# Remove the built the boot sector
cd /src/tutorials/001-bootsectors-barebones
make clean