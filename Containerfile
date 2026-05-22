# syntax=docker/dockerfile:1.7

# --- builder ---------------------------------------------------------------
FROM docker.io/library/rust:1-bookworm AS builder

RUN apt-get update \
    && apt-get install -y --no-install-recommends libc++-dev libc++1 \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src

COPY Cargo.toml Cargo.lock build.rs ./
COPY src ./src
COPY tests ./tests

RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/src/target,sharing=locked \
    cargo build --release --locked \
    && mkdir -p /out/lib \
    && cp target/release/tg /out/tg \
    && cp target/release/build/tdlib-rs-*/out/tdlib/lib/libtdjson.so* /out/lib/

# --- runtime ---------------------------------------------------------------
FROM docker.io/library/debian:bookworm-slim

RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        libc++1 \
        libssl3 \
        zlib1g \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --create-home --uid 1000 tg \
    && mkdir -p /data /downloads \
    && chown tg:tg /data /downloads

COPY --from=builder /out/tg /usr/local/bin/tg
COPY --from=builder /out/lib/ /usr/local/lib/

ENV XDG_DATA_HOME=/data \
    LD_LIBRARY_PATH=/usr/local/lib

USER tg
WORKDIR /downloads

VOLUME ["/data", "/downloads"]

ENTRYPOINT ["/usr/local/bin/tg"]
