#!/bin/sh
# Inject Docker env vars into cron's environment
printenv | grep -v "^_=" > /etc/environment

# Start the Rust API in the background
dualis-scraper &

# Start cron in the foreground
exec cron -f
