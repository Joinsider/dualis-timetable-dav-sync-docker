# --- Stage 1: Build Rust API ---
FROM rust:1.88-slim AS builder

WORKDIR /build
COPY api/ .
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/usr/local/cargo/git \
    --mount=type=cache,target=/build/target \
    cargo build --release && \
    strip target/release/dualis-scraper && \
    cp target/release/dualis-scraper /tmp/dualis-scraper

# --- Stage 2: Runtime ---
FROM debian:bookworm-slim

# OPTIMIZATION 3: Security. Create a non-root user to run the application.
RUN useradd -m -s /bin/bash appuser

# Install CA certificates and clean up apt cache to save space
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates && \
    rm -rf /var/lib/apt/lists/*

# Copy the stripped binary from the builder stage
COPY --from=builder /tmp/dualis-scraper /usr/local/bin/dualis-scraper

# Switch to the non-root user before running the app
USER appuser

EXPOSE 3000
CMD ["dualis-scraper"]