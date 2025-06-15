# syntax=docker/dockerfile:1
FROM ghcr.io/videologyinc/scailx-builder-container:0.11.6

# Install Go 1.23.5 for ARM64
WORKDIR /tmp

SHELL ["/bin/bash", "-c"]

# Add Go
RUN wget https://go.dev/dl/go1.23.5.linux-arm64.tar.gz \
    && rm -rf /usr/go \
    && tar -C /usr -xzf go1.23.5.linux-arm64.tar.gz \
    && rm go1.23.5.linux-arm64.tar.gz

ENV PATH="/usr/go/bin:~/go/bin:$PATH"

# Add Rust
RUN opkg remove rust cargo packagegroup-rust-sdk-target-dev packagegroup-rust-sdk-target \
    && curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- --default-toolchain=1.85.1 -y

ENV PATH="/root/.cargo/bin:$PATH"

RUN ln -sf /usr/bin/bash /usr/bin/sh

# RUN addgroup -g 1001 -S dev && adduser -u 1001 -S dev  -G dev
RUN useradd --uid 1001 --user-group --create-home --shell /usr/bin/bash dev && echo "dev %wheel ALL=(ALL:ALL) NOPASSWD: ALL" >> /etc/sudoers

# set sudo password to empty
# RUN echo "dev:pwd" | chpasswd
# RUN echo "root:pwd" | chpasswd

USER dev
WORKDIR /home/dev
