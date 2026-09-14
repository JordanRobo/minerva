# syntax=docker/dockerfile:1
# Build context: repository root (see deploy/docker-compose.yml).

FROM rust:1.95-slim AS builder
WORKDIR /build

# Copy manifests first so dependency downloads are cached across source changes.
# Stub src files make the member manifests parseable before the real sources arrive.
COPY server/Cargo.toml server/Cargo.lock ./
COPY server/domain/Cargo.toml domain/
COPY server/application/Cargo.toml application/
COPY server/infrastructure/Cargo.toml infrastructure/
COPY server/interface/Cargo.toml interface/
RUN mkdir -p domain/src application/src infrastructure/src interface/src \
    && touch domain/src/lib.rs application/src/lib.rs infrastructure/src/lib.rs interface/src/main.rs
RUN cargo fetch

COPY server/ .
RUN cargo build --release -p interface

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/minerva-server /usr/local/bin/minerva-server

ENV PORT=8080
EXPOSE 8080
CMD ["minerva-server"]
