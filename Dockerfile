# --- Stage 1: Build Rust API ---
FROM rust:1.88-slim AS builder

WORKDIR /build
COPY api/ .
RUN cargo build --release

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

RUN apt-get update && apt-get install -y --no-install-recommends ca-certificates && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/dualis-scraper /usr/local/bin/dualis-scraper

EXPOSE 3000
CMD ["dualis-scraper"]
