# dualis-timetable-dav-sync-docker
A dockerized version of the [dualis-timetable-scraper](https://github.com/cl0q/dhbw_dualis_timetable_scraper) API by [@cl0q](https://github.com/cl0q) with an added python script to sync the events to a dedicated CalDAV calendar.

## Setup:
Populate the files `docker-compose.yml` and `api/.env` with your credentials (examples provided).

Run:
```
docker compose up -d --build
```
