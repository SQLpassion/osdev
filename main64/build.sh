# Builds the KLDR16
cd /src/main64/kaosldr_16
make clean && make

# Builds the KLDR64
cd /src/main64/kaosldr_64
make clean && make

# Build the program1
cd /src/main64/programs/program1
make clean && make

# Build the program2
cd /src/main64/programs/program2
make clean && make


# Builds the kernel
cd /src/main64/kernel
make clean && make