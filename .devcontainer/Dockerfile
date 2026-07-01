# =====================================================================
# Development Container Dockerfile for KAOS OS Development
# =====================================================================
# This container provides the environment for building, running, and
# debugging the KAOS Rust-based Operating System.
#
# Target Platform: linux/amd64
# Base Image: Ubuntu 24.04
# =====================================================================

FROM --platform=linux/amd64 ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

# 1. Install system utilities, development dependencies, emulator, and debugger.
#
# Required tools:
#   - build-essential: General native build utilities (gcc, make, etc.).
#   - nasm: Netwide Assembler to compile 16-bit real-mode bootloader stages.
#   - qemu-system-x86 & ovmf: Emulator and UEFI firmware for booting & testing.
#   - gdb: GNU Debugger for kernel debugging.
#   - gdisk & mtools: Utilities for creating the bootable FAT32 superfloppy disk image.
#   - libncursesw5-dev: Ncurses library supporting QEMU's terminal display mode.
RUN apt-get update && apt-get install -y \
    bash \
    curl \
    git \
    build-essential \
    nasm \
    gdb \
    qemu-system-x86 \
    ovmf \
    gdisk \
    mtools \
    libncursesw5-dev \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# 2. Install Rust Nightly Toolchain and Targets
#
# KAOS is a bare-metal kernel and uses unstable compiler features.
# Pre-installing targets and components prevents rustup from modifying the 
# toolchain state at runtime, avoiding "cross-device link" (os error 18) issues
# when files are moved across docker mount boundaries.
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --default-toolchain nightly
ENV PATH="/root/.cargo/bin:${PATH}"

# Pre-install target platforms:
#   - x86_64-unknown-none: Target for the 64-bit kernel loader and the main kernel.
#   - x86_64-unknown-uefi: Target for the UEFI bootloader (kaosldr_uefi).
RUN rustup target add x86_64-unknown-none x86_64-unknown-uefi

# Install cargo-binutils and LLVM tools for binary manipulation (e.g., extracting flat binaries via rust-objcopy).
RUN rustup component add rust-src llvm-tools-preview \
    && cargo install cargo-binutils

WORKDIR /workspace
