# syntax=docker/dockerfile:1
# One image per workspace binary: `--build-arg BINARY=hub` builds oxihome-hub. See build-image.sh.
FROM rust:1.99-trixie AS build
ARG BINARY
WORKDIR /build
COPY . .
# The cache mounts keep crates and target/ between builds, so a rebuild is incremental.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release --locked -p "oxihome-$BINARY" \
    && cp "target/release/oxihome-$BINARY" /oxihome

FROM debian:trixie-slim
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*
COPY --from=build /oxihome /usr/local/bin/oxihome
WORKDIR /data
ENTRYPOINT ["/usr/local/bin/oxihome"]
