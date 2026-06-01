# Build stage
FROM rust:1.87-slim AS builder

WORKDIR /build

# Copy workspace manifests first for layer caching
COPY Cargo.toml Cargo.lock ./
COPY crates/coven-relay/Cargo.toml crates/coven-relay/Cargo.toml
COPY crates/coven-cli/Cargo.toml crates/coven-cli/Cargo.toml

# Dummy source to cache dependencies
RUN mkdir -p crates/coven-relay/src crates/coven-cli/src && \
    echo 'fn main() {}' > crates/coven-relay/src/main.rs && \
    echo 'fn main() {}' > crates/coven-cli/src/main.rs && \
    cargo build --release -p coven-relay && \
    rm -rf crates/coven-relay/src

# Build the real binary
COPY crates/coven-relay/src crates/coven-relay/src
RUN touch crates/coven-relay/src/main.rs && \
    cargo build --release -p coven-relay

# Runtime stage
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/coven-relay /usr/local/bin/coven-relay

ENV LISTEN_ADDR=0.0.0.0:8080
EXPOSE 8080

ENTRYPOINT ["/usr/local/bin/coven-relay"]
