# PIChain Node — Multi-stage Docker Build
# Usage:
#   docker build -t pichain .
#   docker run -v pichain-data:/data -p 8314:8314 -p 9314:9314 pichain

# Stage 1: Build
FROM rust:1.82-bookworm AS builder

RUN apt-get update && apt-get install -y \
    clang \
    libclang-dev \
    cmake \
    protobuf-compiler \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /build
COPY . .

RUN cargo build --release --bin pichain --bin pichain-cli --bin pichain-miner

# Stage 2: Runtime
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y \
    ca-certificates \
    libssl3 \
    curl \
    && rm -rf /var/lib/apt/lists/*

# Create pichain user
RUN useradd -m -s /bin/bash pichain

# Copy binaries
COPY --from=builder /build/target/release/pichain /usr/local/bin/pichain
COPY --from=builder /build/target/release/pichain-cli /usr/local/bin/pichain-cli
COPY --from=builder /build/target/release/pichain-miner /usr/local/bin/pichain-miner

# Data directory
RUN mkdir -p /data && chown pichain:pichain /data
VOLUME ["/data"]

# Default ports: 8314 (RPC), 9314 (P2P)
EXPOSE 8314 9314

USER pichain

# Health check
HEALTHCHECK --interval=10s --timeout=3s --start-period=15s --retries=3 \
    CMD curl -sf http://localhost:8314/health || exit 1

ENTRYPOINT ["pichain"]
CMD ["run", "--data-dir", "/data", "--rpc-addr", "0.0.0.0:8314", "--p2p-addr", "/ip4/0.0.0.0/tcp/9314"]
