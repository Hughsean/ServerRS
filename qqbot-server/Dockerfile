FROM rust:1.94-bookworm AS builder

WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY qqbot-server qqbot-server
COPY agent-core agent-core
COPY personal-secretary personal-secretary
COPY personal-secretary-mysql personal-secretary-mysql
COPY qqbot qqbot
COPY qq-open-platform qq-open-platform
COPY tools/architecture-tests tools/architecture-tests
RUN cargo build --locked --release -p qqbot-server

FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install --no-install-recommends -y ca-certificates libssl3 socat \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /app
STOPSIGNAL SIGTERM
COPY --from=builder /build/target/release/qqbot-server /usr/local/bin/qqbot-server
COPY qqbot-server/config/qqbot.example.toml /app/config/qqbot.toml
COPY docker/qqbot-entrypoint.sh /usr/local/bin/qqbot-entrypoint
RUN chmod 0555 /usr/local/bin/qqbot-server /usr/local/bin/qqbot-entrypoint \
    && mkdir -p /app/config/data

ENTRYPOINT ["/usr/local/bin/qqbot-entrypoint"]
