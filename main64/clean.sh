# Remove the built KLDR16.BIN
cd /src/main64/kaosldr_16
make clean

# Remove the built KLDR64.BIN
cd /src/main64/kaosldr_64
make clean

# Remove the built kernel
cd /src/main64/kernel
make clean

# Remove the program1
cd /src/main64/programs/program1
make clean

# Remove the program2
cd /src/main64/programs/program2
make clean

# Removes the command shell
cd /src/main64/programs/shell
make clean