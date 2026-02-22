FROM rust:1.82-alpine AS builder
RUN apk add musl-dev
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY crates/ crates/
RUN cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:3.20
RUN apk add --no-cache ripgrep
COPY --from=builder /src/target/x86_64-unknown-linux-musl/release/openplanter-agent /usr/local/bin/
RUN mkdir -p /workspace
WORKDIR /workspace
ENTRYPOINT ["openplanter-agent"]
