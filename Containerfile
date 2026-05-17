# syntax=docker/dockerfile:1
# Build from the repository root:
# podman build -f Containerfile .

FROM rust:1.95.0-slim AS build
RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools \
    && rm -rf /var/lib/apt/lists/* \
    && rustup target add x86_64-unknown-linux-musl
WORKDIR /workspace
COPY . /workspace/tiny-httpd
WORKDIR /workspace/tiny-httpd
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM scratch
COPY --from=build /workspace/tiny-httpd/target/x86_64-unknown-linux-musl/release/tiny-httpd /app/tiny-httpd
ENTRYPOINT ["/app/tiny-httpd"]
