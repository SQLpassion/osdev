# Builds the KAOSLDR_x16
cd /src/main64/kaosldr_x16
make clean && make

# Builds the kernel
cd /src/main64/kernel
make clean && make