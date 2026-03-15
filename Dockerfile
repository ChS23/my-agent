FROM rust:1.92-alpine3.22 AS builder

RUN apk add --no-cache musl-dev cmake make perl gcc g++

WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && \
    cargo build --release --target x86_64-unknown-linux-musl 2>/dev/null || true && \
    rm -rf src

# Build actual code
COPY src/ src/
RUN touch src/main.rs && \
    cargo build --release --target x86_64-unknown-linux-musl

FROM alpine:3.22

RUN apk add --no-cache ca-certificates tzdata

WORKDIR /app

COPY --from=builder /app/target/x86_64-unknown-linux-musl/release/agent .
COPY config.toml ./
COPY SOUL.md IDENTITY.md FORMAT.md ./
COPY skills/ skills/

VOLUME /app/data

ENV RUST_LOG=info,agent=debug
ENV TZ=Europe/Moscow

CMD ["./agent"]
