FROM ubuntu:18.04

RUN apt-get update -y  && apt-get install curl gnupg -y

RUN curl -sS https://dl.yarnpkg.com/debian/pubkey.gpg | apt-key add - \ 
    && echo "deb https://dl.yarnpkg.com/debian/ stable main" | tee /etc/apt/sources.list.d/yarn.list

RUN apt-get update -qq && apt-get install -y -q --no-install-recommends \
    build-essential \
    curl \
    clang \
    cmake \
    git \
    g++ \
    libssl-dev \
    llvm \
    netcat \
    pkg-config \
    python3 \
    wget \
    yarn \
    && rm -rf /var/lib/apt/lists/*


ENV RUSTUP_HOME=/usr/local/rustup \
    CARGO_HOME=/usr/local/cargo \
    PATH=/usr/local/cargo/bin:$PATH

RUN curl https://sh.rustup.rs -sSf | \
    sh -s -- -y --no-modify-path --default-toolchain nightly-2020-05-15

RUN curl -o- https://raw.githubusercontent.com/nvm-sh/nvm/v0.35.2/install.sh | bash

RUN wget https://golang.org/dl/go1.15.2.linux-amd64.tar.gz
RUN tar -C /usr/local -xzf go1.15.2.linux-amd64.tar.gz
RUN echo 'export PATH=$PATH:/usr/local/go/bin' >> ~/.bashrc


SHELL ["/bin/bash", "--login", "-c", "-i"]
WORKDIR /usr/src
COPY ./.nvmrc .
RUN nvm install


COPY ./package.json .
RUN node --version
RUN yarn install

COPY . /usr/src/

COPY config* ~/.rainbow

RUN ./index.js prepare
