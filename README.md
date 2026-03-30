# dualis-timetable-ics

A dockerized version of the [dualis-timetable-scraper](https://github.com/cl0q/dhbw_dualis_timetable_scraper) API by [@cl0q](https://github.com/cl0q) that serves your DHBW Dualis timetable as a subscribable `.ics` calendar.

## How it works

The container runs a single Rust HTTP server that:
1. Scrapes your timetable from the Dualis student portal
2. Generates an iCalendar (`.ics`) file
3. Serves it at `GET /calendar.ics?token=<YOUR_API_KEY>`

Calendar clients (Apple Calendar, Google Calendar, Thunderbird, etc.) can subscribe to this URL and will automatically receive updates.

## Setup

1. Copy the example files:
   ```
   cp docker-compose.yml.example docker-compose.yml
   cp api/.env.example api/.env
   ```

2. Fill in your credentials in `docker-compose.yml` (or `api/.env` for local development):
   - `API_KEY` - a secret token to protect the endpoint
   - `DUALIS_USERNAME` - your Dualis username (e.g. `xxxxxx@hb.dhbw-stuttgart.de`)
   - `DUALIS_PASSWORD` - your Dualis password

3. Run:
   ```
   docker compose up -d --build
   ```

4. Subscribe in your calendar app using:
   ```
   http://<your-host>:3000/calendar.ics?token=<YOUR_API_KEY>
   ```

## Configuration

| Variable | Default | Description |
|---|---|---|
| `API_KEY` | *(required)* | Secret token for authenticating requests |
| `DUALIS_USERNAME` | *(required)* | Dualis login username |
| `DUALIS_PASSWORD` | *(required)* | Dualis login password |
| `PORT` | `3000` | HTTP server port |
| `WEEKS_AHEAD` | `2` | Number of weeks to fetch beyond the current week |
| `CALENDAR_NAME` | `Dualis Timetable` | Display name shown in calendar apps |
| `UID_DOMAIN` | `schedule-sync.local` | Domain suffix for event UIDs |
| `TIMEZONE` | `Europe/Berlin` | IANA timezone for event times |
| `CACHE_TTL_SECONDS` | `3600` | How long to cache the calendar before re-fetching from Dualis |

## API Endpoints

| Endpoint | Auth | Description |
|---|---|---|
| `GET /health` | None | Health check |
| `GET /calendar.ics?token=<key>` | Query param | Subscribable iCalendar file |
| `GET /timetable?week=YYYY-Www` | Bearer token | JSON timetable for a single week |
| `GET /debug/timetable?week=YYYY-Www` | Bearer token | Raw HTML from Dualis (debugging) |
