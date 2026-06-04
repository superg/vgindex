# Rust >= 1.86 required: lindera pulls ICU 2.x (rust-version 1.86); the lockfile
# targets the 1.94 toolchain. Bump in lockstep if you change the Rust version.
FROM rust:1.94-bookworm AS builder
WORKDIR /app

COPY Cargo.toml Cargo.lock* ./
RUN mkdir src && echo 'fn main() {}' > src/main.rs && cargo build --release && rm -rf src

COPY src ./src
COPY templates ./templates
COPY static ./static
COPY migrations ./migrations
ENV SQLX_OFFLINE=true
RUN find src -name "*.rs" -exec touch {} + && cargo build --release && cargo test

FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /app/target/release/vgindex ./vgindex
COPY templates ./templates
COPY static ./static
COPY migrations ./migrations
EXPOSE 3000
CMD ["./vgindex"]
