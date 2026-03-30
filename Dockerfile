# --- Stage 1: Build Rust API ---
FROM rust:1.88-slim AS builder

WORKDIR /build
COPY api/ .
RUN cargo build --release

# --- Stage 2: Runtime ---
FROM python:3.12-slim

RUN apt-get update && apt-get install -y --no-install-recommends cron ca-certificates && rm -rf /var/lib/apt/lists/*

# Copy Rust binary — replace 'dualis-scraper' with the name in your Cargo.toml if different
COPY --from=builder /build/target/release/dualis-scraper /usr/local/bin/dualis-scraper
COPY api/.env /app/.env

# Install Python dependencies
WORKDIR /app
COPY sync/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt

COPY sync/sync-schedule.py .
COPY entrypoint.sh .
RUN chmod +x entrypoint.sh

# Cron job: run at midnight and noon
RUN echo "0 0,12 * * * root /usr/local/bin/python /app/sync-schedule.py >> /proc/1/fd/1 2>> /proc/1/fd/2" \
    > /etc/cron.d/sync-schedule \
    && chmod 0644 /etc/cron.d/sync-schedule

CMD ["./entrypoint.sh"]
