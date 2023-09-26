FROM rust:slim as builder

WORKDIR /build

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

RUN cargo build --profile release-devnet && \
    cp target/release-devnet/snarkos snarkos_node &&  \
    cargo build --profile release-devnet --features jemalloc && \
    cp target/release-devnet/snarkos snarkos_jemalloc && \
    rm -rf target

FROM rust:slim

RUN apt-get update && apt-get install -y heaptrack

WORKDIR /snarkos
COPY --from=builder /build/snarkos_node .
COPY --from=builder /build/snarkos_jemalloc .
ADD docker/start-docker-node.sh .
RUN chmod 100 start-docker-node.sh

EXPOSE 5000 3000
ENTRYPOINT ["./start-docker-node.sh"]
