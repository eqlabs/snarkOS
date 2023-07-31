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

RUN cargo build --release --package snarkos-node-narwhal --example simple_node

FROM rust:slim

WORKDIR /simple_node
COPY --from=builder /simple_node/target/release/examples/simple_node .
COPY --from=builder --chmod=100 /simple_node/node/narwhal/examples/start-docker-node.sh .

EXPOSE 5000 3000
ENTRYPOINT ["./start-docker-node.sh"]
