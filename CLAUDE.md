# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

A Dockerized Rust HTTP server that scrapes the DHBW Dualis student portal and serves the timetable as a subscribable `.ics` calendar file. Calendar clients subscribe to `GET /calendar.ics?token=<API_KEY>` and receive automatic updates.

## Development Commands

All Rust code lives in `api/`. Run these from inside `api/`:

```bash
# Local development (requires api/.env with credentials)
cd api && cargo run

# Build
cd api && cargo build --release

# Check (fast, no binary output)
cd api && cargo check

# Lint
cd api && cargo clippy

# Run tests
cd api && cargo test
```

**Docker (production-like):**
```bash
docker compose up -d --build
```

## Local Environment Setup

Copy and fill in credentials before running locally:
```bash
cp api/.env.example api/.env
cp docker-compose.yml.example docker-compose.yml
```

Required env vars: `API_KEY`, `DUALIS_USERNAME`, `DUALIS_PASSWORD`. All others have defaults (see `api/src/config.rs`).

## Architecture

The app is a single-binary Axum web server with shared state and an in-memory cache.

**Request flow for `/calendar.ics`:**
1. `routes::calendar_ics` validates the `?token=` query param against `config.api_key`
2. Checks the in-memory `RwLock<Option<CachedCalendar>>` in `AppState`; serves cached ICS if still within `CACHE_TTL_SECONDS`
3. On cache miss: `DualisClient::fetch_timetables` logs in once and scrapes `WEEKS_AHEAD + 1` weeks
4. `ical::build_calendar` converts the structured data into an RFC 5545-compliant ICS string
5. Result is stored in cache and returned

**Module responsibilities:**
- `config.rs` — reads all config from env vars; fails fast if required vars are missing
- `dualis.rs` — Dualis/CampusNet scraper: login (extracts session token from non-standard `Refresh` header), fetches week view HTML, parses `<td class="appointment">` cells via `scraper` crate
- `ical.rs` — builds ICS output including RFC 5545 line folding at 75 octets; hardcoded VTIMEZONE block for `Europe/Berlin`; exams are prefixed with `PRÜFUNG:` in SUMMARY
- `routes.rs` — Axum handlers; `/timetable` and `/debug/timetable` are Bearer-token protected via middleware; `/calendar.ics` uses query-param auth
- `middleware.rs` — extracts Bearer token from `Authorization` header, compares to `config.api_key`
- `error.rs` — `AppError` enum with `IntoResponse` impl; login failures map to 502

**Dualis scraping quirks** (documented in `dualis.rs`):
- Session token is in URL ARGUMENTS params (`-N<token>`), not just cookies
- Login success returns HTTP 200 with a non-standard `Refresh:` header (not a 302)
- Timetable cells use German weekday names in the `abbr` attribute (`"Montag Spalte 1"`, etc.)
- Exams are identified by `background-color:#ff6666` inline style on the `<td>`

## Docker Build

Multi-stage Dockerfile: Rust builder → `debian:bookworm-slim` runtime. Runs as non-root `appuser`. Exposed on port 3000. Cache mounts are used for `cargo registry/git/target` to speed up rebuilds.
