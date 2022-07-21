# Builds the KAOSLDR
cd /src/main64/kaosldr
make clean && make

# Builds the kernel
cd /src/main64/kernel
make clean && make