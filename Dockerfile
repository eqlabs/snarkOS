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

RUN cargo build --release

FROM gcr.io/distroless/cc

WORKDIR /snarkos
COPY --from=builder /build/target/release/snarkos .

EXPOSE 5000 3000
ENTRYPOINT ["./snarkos"]
