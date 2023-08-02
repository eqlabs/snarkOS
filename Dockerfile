FROM rust:slim as builder

WORKDIR /simple_node

RUN apt-get update && apt-get install -y \
    build-essential \
    curl \
    clang \
    gcc \
    libssl-dev \
    llvm \
    make \
    pkg-config \
    tmux \
    xz-utils \
    ufw

ADD node ./node/
ADD snarkos ./snarkos/
ADD display ./display/
ADD cli ./cli/
ADD account ./account/
ADD Cargo.lock ./Cargo.lock
ADD Cargo.toml ./Cargo.toml
ADD .integration ./.integration

RUN cargo build --profile release-devnet --package snarkos-node-narwhal --example simple_node && \
    cp target/release-devnet/examples/simple_node . &&  \
    cargo build --profile release-devnet --features jemalloc --package snarkos-node-narwhal --example simple_node && \
    cp target/release-devnet/examples/simple_node simple_node_jemalloc && \
    rm -rf target

FROM rust:slim

RUN apt-get update && apt-get install -y heaptrack

WORKDIR /simple_node
COPY --from=builder /simple_node/simple_node .
COPY --from=builder /simple_node/simple_node_jemalloc .
COPY --from=builder /simple_node/node/narwhal/examples/start-docker-node.sh .
RUN chmod 100 start-docker-node.sh

EXPOSE 5000 3000
ENTRYPOINT ["./start-docker-node.sh"]
