# =====================================================================
# Phase 1: Toolchain Build (GCC & Binutils cross compiler für x86_64-elf)
# =====================================================================
FROM --platform=linux/amd64 ubuntu:24.04 as toolchain-builder

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y \
    build-essential \
    bison \
    flex \
    libgmp-dev \
    libmpfr-dev \
    libmpc-dev \
    texinfo \
    wget \
    tar \
    bzip2

WORKDIR /usr/src
RUN wget ftp://ftp.gnu.org/gnu/gcc/gcc-12.2.0/gcc-12.2.0.tar.gz && \
    wget https://ftp.gnu.org/gnu/binutils/binutils-2.40.tar.gz && \
    tar -xf binutils-2.40.tar.gz && \
    tar -xf gcc-12.2.0.tar.gz

ENV PREFIX="/opt/cross"
ENV TARGET=x86_64-elf
ENV PATH="$PREFIX/bin:$PATH"

# Build Binutils
RUN mkdir build-binutils && cd build-binutils && \
    ../binutils-2.40/configure --target=$TARGET --prefix=$PREFIX --with-sysroot --disable-nls --disable-werror && \
    make -j$(nproc) && make install

# Build GCC (nur C/C++ Compiler ohne Header)
RUN mkdir build-gcc && cd build-gcc && \
    ../gcc-12.2.0/configure --target=$TARGET --prefix="$PREFIX" --disable-nls --enable-languages=c,c++ --without-headers && \
    make -j$(nproc) all-gcc && make install-gcc

# Download fat_imgen
WORKDIR /tmp
RUN wget https://sourceforge.net/projects/fatimgen/files/fat_imgen%202.2.4/fat_imgen-2.2.4-Linux-amd64.tar.bz2 && \
    tar -xf fat_imgen-2.2.4-Linux-amd64.tar.bz2 && \
    cp fat_imgen-2.2.4/fat_imgen /opt/cross/bin/fat_imgen

# =====================================================================
# Phase 2: Finale Entwicklungsumgebung (Ubuntu 24.04 mit QEMU Curses Support)
# =====================================================================
FROM --platform=linux/amd64 ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# 1. System-Tools, Compiler-Abhängigkeiten, GDB und QEMU mit Curses installieren
RUN apt-get update && apt-get install -y \
    bash \
    curl \
    git \
    build-essential \
    nasm \
    gdb \
    libgmp-dev \
    libmpfr-dev \
    libmpc-dev \
    qemu-system-x86 \
    libncursesw5-dev \
    ca-certificates

# 2. Rustup und Rust Nightly für x86_64-unknown-none (Bare Metal)
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
ENV PATH="/root/.cargo/bin:${PATH}"
RUN rustup target add x86_64-unknown-none
RUN cargo install cargo-binutils

# 3. Kopiere die Cross-Compiler-Toolchain & fat_imgen aus Phase 1
COPY --from=toolchain-builder /opt/cross /opt/cross
ENV PATH="/opt/cross/bin:${PATH}"

WORKDIR /workspace
