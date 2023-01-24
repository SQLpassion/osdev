###############
# Image Build
###############
# If the image is built on Apple Silicon (M1, M2), you have to disable the setting "Use Rosetta for x86/amd64 emulation on Apple Sillicon"
# within the Docker Desktop settings.
# On lower end machines (like the Apple Air M1 with 8 GB RAM), you also have to specify a lower value for the "make -j" flag (like "-j 4").
# Otherwise you could run into Out-Of-Memory conditions during the GCC build.

# docker image build -t sqlpassion/kaos-buildenv:latest .

# Use the latest Alpine image as a base
FROM --platform=linux/amd64 alpine:latest as builder

# Install all needed packages
RUN apk add wget
RUN apk add g++
RUN apk add make
RUN apk add flex
RUN apk add bison
RUN apk add texinfo
RUN apk add gmp-dev
RUN apk add mpfr-dev
RUN apk add mpc1-dev

# Create a new souce directory
RUN mkdir $HOME/src
WORKDIR $HOME/src

# Download the current version of GCC and the binutils
RUN wget ftp://ftp.gnu.org/gnu/gcc/gcc-12.2.0/gcc-12.2.0.tar.gz
RUN wget https://ftp.gnu.org/gnu/binutils/binutils-2.40.tar.gz

RUN tar -xf binutils-2.40.tar.gz
RUN tar -xf gcc-12.2.0.tar.gz

###############################
# Build the x64 Cross Compiler
###############################

ENV PREFIX="$HOME/opt/cross"
ENV TARGET=x86_64-elf
ENV PATH="$PREFIX/bin:$PATH"

RUN mkdir build-binutils
WORKDIR build-binutils

RUN ../binutils-2.40/configure --target=$TARGET --prefix=$PREFIX --with-sysroot --disable-nls --disable-werror
RUN make -j 8
RUN make -j 8 install

WORKDIR $HOME/src

RUN mkdir build-gcc
WORKDIR build-gcc

RUN ../gcc-12.2.0/configure --target=$TARGET --prefix="$PREFIX" --disable-nls --enable-languages=c,c++ --without-headers
RUN make -j 8 all-gcc
RUN make -j 8 install-gcc

#############
# fat_imgen
#############

RUN mkdir /build
WORKDIR build
RUN wget https://sourceforge.net/projects/fatimgen/files/fat_imgen%202.2.4/fat_imgen-2.2.4-Linux-amd64.tar.bz2
RUN tar -xf fat_imgen-2.2.4-Linux-amd64.tar.bz2
WORKDIR fat_imgen-2.2.4
RUN cp fat_imgen /opt/cross/bin/fat_imgen

######################################
# Building of the final Docker Image
######################################

# Create a new image and copy the built cross compiler into it
FROM --platform=linux/amd64 alpine:latest
COPY --from=builder /opt /opt

# Install additional needed packages
RUN apk add nasm
RUN apk add make
RUN apk add gmp-dev
RUN apk add mpfr-dev
RUN apk add mpc1-dev
RUN apk add build-base gcompat

# Export the cross compilers
ENV PATH="$HOME/opt/cross/bin:$PATH"

########
# Usage
########
# If you run the image on Apple Silicon (M1, M2), you have to *enable* the setting "Use Rosetta for x86/amd64 emulation on Apple Sillicon"
# within the Docker Desktop settings.

# docker run --rm -it -v d:\GitHub\SQLpassion/osdev:/src sqlpassion/kaos-buildenv /bin/sh /src/main64/build.sh
# docker run --rm -it -v $HOME/Documents/GitHub/SQLpassion/osdev:/src sqlpassion/kaos-buildenv /bin/sh /src/main64/build.sh

# Windows ARM with WSL2
# sudo docker run --rm -it -v /mnt/c/Users/klaus/documents/github/SQLpassion/osdev:/src sqlpassion/kaos-buildenv /bin/sh /src/main64/build.sh
# sudo docker run --rm -it -v /mnt/c/Users/klaus/documents/github/SQLpassion/osdev:/src sqlpassion/kaos-buildenv /bin/sh /src/main64/clean.sh