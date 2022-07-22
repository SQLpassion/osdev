# Remove the built KLDR16.BIN
cd /src/main64/kaosldr_16
make clean

# Remove the built KLDR64.BIN
cd /src/main64/kaosldr_64
make clean

# Remove the built kernel
cd /src/main64/kernel
make clean