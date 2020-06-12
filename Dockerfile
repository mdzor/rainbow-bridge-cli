FROM ubuntu:18.04

RUN apt-get update && apt-get install -y \
    build-essential \
    curl \
    clang \
    cmake \
    git \
    g++ \
    nodejs \
    libssl-dev \
    llvm \
    netcat \
    pkg-config \
    python3 \
    && rm -rf /var/lib/apt/lists/*


ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl https://sh.rustup.rs -sSf | \
    sh -s -- -y --no-modify-path --default-toolchain nightly-2020-05-15

COPY . /usr/src/
COPY config* ~/.rainbowup

WORKDIR /usr/src/environment
RUN node index.js prepare
