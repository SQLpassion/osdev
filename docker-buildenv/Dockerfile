# Use the latest Alpine image as a base
FROM alpine:latest as builder

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
RUN wget ftp://ftp.gnu.org/gnu/gcc/gcc-11.2.0/gcc-11.2.0.tar.gz
RUN wget https://ftp.gnu.org/gnu/binutils/binutils-2.37.tar.gz

RUN tar -xf binutils-2.37.tar.gz
RUN tar -xf gcc-11.2.0.tar.gz

###############################
# Build the x64 Cross Compiler
###############################

ENV PREFIX="$HOME/opt/cross"
ENV TARGET=x86_64-elf
ENV PATH="$PREFIX/bin:$PATH"

RUN mkdir build-binutils
WORKDIR build-binutils

RUN ../binutils-2.37/configure --target=$TARGET --prefix=$PREFIX --with-sysroot --disable-nls --disable-werror
RUN make
RUN make install

WORKDIR $HOME/src

RUN mkdir build-gcc
WORKDIR build-gcc

RUN ../gcc-11.2.0/configure --target=$TARGET --prefix="$PREFIX" --disable-nls --enable-languages=c,c++ --without-headers
RUN make all-gcc
RUN make install-gcc

###############################
# Build the x32 Cross Compiler
###############################

ENV PREFIX="$HOME/opt/cross"
ENV TARGET=i586-elf
ENV PATH="$PREFIX/bin:$PATH"

WORKDIR $HOME/src
RUN mkdir build-binutils-i586
WORKDIR build-binutils-i586

RUN ../binutils-2.37/configure --target=$TARGET --prefix=$PREFIX --with-sysroot --disable-nls --disable-werror
RUN make
RUN make install

WORKDIR $HOME/src
RUN mkdir build-gcc-i586
WORKDIR build-gcc-i586

RUN ../gcc-11.2.0/configure --target=$TARGET --prefix="$PREFIX" --disable-nls --enable-languages=c,c++ --without-headers
RUN make all-gcc
RUN make install-gcc

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
FROM alpine:latest
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
# USAGE
########

# docker image build -t sqlpassion/kaos-buildenv:latest .
# docker run --rm -it -v d:\GitHub\KAOS:/src sqlpassion/kaos-buildenv /bin/sh /src/main64/build.sh
# docker run --rm -it -v $HOME/Documents/GitHub/KAOS:/src sqlpassion/kaos-buildenv /bin/sh /src/main64/build.sh