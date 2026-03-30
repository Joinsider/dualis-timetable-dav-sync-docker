#!/usr/bin/env python3
"""
sync_schedule.py — Sync a weekly schedule API to a dedicated Baikal CalDAV calendar.

Strategy: wipe all events from the calendar on every run, then push the full
set from the API. Safe to do because this calendar is exclusively managed by
this script — no manually created events will be lost.

Run via cron, e.g. every hour:
    0 * * * * /usr/bin/python3 /path/to/sync_schedule.py
"""

import logging
import os
import re
import sys
from datetime import datetime, timedelta
from zoneinfo import ZoneInfo
from xml.etree import ElementTree as ET

import requests
from requests.auth import HTTPBasicAuth

from icalendar import Calendar, Event

# ---------------------------------------------------------------------------
# Configuration — edit these or override via environment variables
# ---------------------------------------------------------------------------

API_BASE_URL    = os.getenv("SCHEDULE_API_URL", "http://localhost:3000/timetable")
API_WEEK_PARAM  = os.getenv("SCHEDULE_API_WEEK_PARAM", "week")
API_BEARER_TOKEN = os.getenv("SCHEDULE_API_TOKEN", "")

BAIKAL_URL      = os.getenv("BAIKAL_URL", "")
BAIKAL_USER     = os.getenv("BAIKAL_USER", "")
BAIKAL_PASSWORD = os.getenv("BAIKAL_PASSWORD", "")
BAIKAL_CALENDAR = os.getenv("BAIKAL_CALENDAR", "dualis")
# Override if your Baikal installation uses a different path prefix
CALDAV_CALENDAR_URL = os.getenv(
    "CALDAV_CALENDAR_URL",
    f"{BAIKAL_URL}/dav.php/calendars/{BAIKAL_USER}/{BAIKAL_CALENDAR}/",
)

TIMEZONE      = ZoneInfo(os.getenv("TIMEZONE", "Europe/Berlin"))
WEEKS_AHEAD   = int(os.getenv("WEEKS_AHEAD", "2"))  # current week + 2 ahead
UID_DOMAIN    = os.getenv("UID_DOMAIN", "schedule-sync.local")

LOG_LEVEL = os.getenv("LOG_LEVEL", "INFO")
logging.basicConfig(
    level=getattr(logging, LOG_LEVEL),
    format="%(asctime)s %(levelname)s %(message)s",
    datefmt="%Y-%m-%dT%H:%M:%S",
)
log = logging.getLogger(__name__)

# ---------------------------------------------------------------------------
# HTTP session
# ---------------------------------------------------------------------------

SESSION = requests.Session()
SESSION.auth = HTTPBasicAuth(BAIKAL_USER, BAIKAL_PASSWORD)

API_SESSION = requests.Session()
if API_BEARER_TOKEN:
    API_SESSION.headers["Authorization"] = f"Bearer {API_BEARER_TOKEN}"

# ---------------------------------------------------------------------------
# CalDAV — list, delete, put
# ---------------------------------------------------------------------------

PROPFIND_BODY = """<?xml version="1.0" encoding="utf-8"?>
<propfind xmlns="DAV:">
  <prop><getetag/></prop>
</propfind>"""

NS = {"d": "DAV:", "c": "urn:ietf:params:xml:ns:caldav"}


def caldav_list_hrefs() -> list[str]:
    """Return all .ics hrefs currently in the calendar via PROPFIND depth 1."""
    resp = SESSION.request(
        "PROPFIND",
        CALDAV_CALENDAR_URL,
        data=PROPFIND_BODY,
        headers={"Depth": "1", "Content-Type": "application/xml"},
    )
    if resp.status_code != 207:
        raise RuntimeError(f"PROPFIND → {resp.status_code}: {resp.text[:300]}")

    root = ET.fromstring(resp.text)
    hrefs = []
    for response in root.findall("d:response", NS):
        href = response.findtext("d:href", namespaces=NS)
        # Skip the collection itself; keep only .ics resources
        if href and href.rstrip("/") != CALDAV_CALENDAR_URL.rstrip("/") and href.endswith(".ics"):
            hrefs.append(href)
    return hrefs


def caldav_delete_href(href: str) -> None:
    """Delete a calendar object by its full href path."""
    url = href if href.startswith("http") else f"{BAIKAL_URL}{href}"
    resp = SESSION.delete(url)
    if resp.status_code not in (200, 204, 404):
        raise RuntimeError(f"DELETE {url} → {resp.status_code}: {resp.text[:200]}")
    log.debug("DELETE %s → %s", url, resp.status_code)


def caldav_put(uid: str, ics: bytes) -> None:
    url = f"{CALDAV_CALENDAR_URL.rstrip('/')}/{uid}.ics"
    resp = SESSION.put(
        url,
        data=ics,
        headers={"Content-Type": "text/calendar; charset=utf-8"},
    )
    if resp.status_code not in (200, 201, 204):
        raise RuntimeError(f"PUT {url} → {resp.status_code}: {resp.text[:200]}")
    log.debug("PUT %s → %s", url, resp.status_code)


# ---------------------------------------------------------------------------
# Date / time parsing
#   API date format : "09.03.2026"
#   API time format : "13:00 - 16:30"
# ---------------------------------------------------------------------------

def parse_datetime(date_str: str, time_str: str) -> tuple[datetime, datetime]:
    date = datetime.strptime(date_str, "%d.%m.%Y").date()
    match = re.match(r"(\d{1,2}:\d{2})\s*-\s*(\d{1,2}:\d{2})", time_str)
    if not match:
        raise ValueError(f"Cannot parse time string: {time_str!r}")
    start_t = datetime.strptime(match.group(1), "%H:%M").time()
    end_t   = datetime.strptime(match.group(2), "%H:%M").time()
    dtstart = datetime.combine(date, start_t, tzinfo=TIMEZONE)
    dtend   = datetime.combine(date, end_t,   tzinfo=TIMEZONE)
    if dtend <= dtstart:          # handle overnight edge case
        dtend += timedelta(days=1)
    return dtstart, dtend


# ---------------------------------------------------------------------------
# iCalendar builder
# ---------------------------------------------------------------------------

def make_uid(date_str: str, title: str, index: int) -> str:
    """
    Human-readable UID. index disambiguates the rare case of two events
    with the same title on the same day.
    """
    slug = re.sub(r"[^a-z0-9]+", "-", title.strip().lower()).strip("-")
    date_slug = date_str.replace(".", "")   # "09032026"
    return f"{date_slug}-{slug}-{index}@{UID_DOMAIN}"


def build_ics(uid: str, date_str: str, event: dict) -> bytes:
    dtstart, dtend = parse_datetime(date_str, event["time"])
    now = datetime.now(tz=TIMEZONE)

    cal = Calendar()
    cal.add("prodid", "-//schedule-sync//EN")
    cal.add("version", "2.0")
    cal.add("calscale", "GREGORIAN")

    ev = Event()
    ev.add("uid",           uid)
    ev.add("summary",       event.get("title", "Untitled"))
    ev.add("dtstart",       dtstart)
    ev.add("dtend",         dtend)
    ev.add("dtstamp",       now)
    ev.add("last-modified", now)
    ev.add("sequence",      0)

    location_parts = [p for p in [event.get("room"), event.get("event_type")] if p]
    if location_parts:
        ev.add("location", " — ".join(location_parts))

    description_parts = []
    if event.get("lecturer"):
        description_parts.append(f"Lecturer: {event['lecturer']}")
    if event.get("event_type"):
        description_parts.append(f"Type: {event['event_type']}")
    if description_parts:
        ev.add("description", "\n".join(description_parts))

    cal.add_component(ev)
    return cal.to_ical()


# ---------------------------------------------------------------------------
# API fetching
# ---------------------------------------------------------------------------

def iso_week(dt: datetime) -> str:
    return f"{dt.isocalendar().year}-W{dt.isocalendar().week:02d}"


def fetch_week(week: str) -> list[tuple[str, dict]]:
    resp = API_SESSION.get(API_BASE_URL, params={API_WEEK_PARAM: week}, timeout=15)
    resp.raise_for_status()
    data = resp.json()
    events = []
    for day in data.get("days", []):
        for ev in day.get("events", []):
            events.append((day["date"], ev))
    return events


def fetch_all_weeks() -> list[tuple[str, dict]]:
    today = datetime.now(tz=TIMEZONE)
    all_events: list[tuple[str, dict]] = []
    seen: set[str] = set()
    for offset in range(WEEKS_AHEAD + 1):
        week = iso_week(today + timedelta(weeks=offset))
        if week in seen:
            continue
        seen.add(week)
        log.info("Fetching week %s", week)
        try:
            all_events.extend(fetch_week(week))
        except Exception as exc:
            log.error("Failed to fetch week %s: %s", week, exc)
    return all_events


# ---------------------------------------------------------------------------
# Main sync logic — wipe then repopulate
# ---------------------------------------------------------------------------

def sync() -> None:
    # 1. Fetch fresh data from the API first.
    #    If this fails we abort before touching the calendar, so we never
    #    end up with a wiped but empty calendar.
    api_events = fetch_all_weeks()
    log.info("Fetched %d events from API", len(api_events))

    if not api_events:
        log.warning("API returned no events at all — aborting to avoid wiping calendar")
        sys.exit(1)

    # 2. Wipe everything currently in the calendar.
    log.info("Listing existing calendar entries…")
    try:
        hrefs = caldav_list_hrefs()
    except Exception as exc:
        log.error("Could not list calendar contents: %s", exc)
        sys.exit(1)

    log.info("Deleting %d existing events…", len(hrefs))
    delete_errors = 0
    for href in hrefs:
        try:
            caldav_delete_href(href)
        except Exception as exc:
            log.error("Failed to delete %s: %s", href, exc)
            delete_errors += 1

    if delete_errors:
        log.error("%d deletions failed — aborting push to avoid duplicates", delete_errors)
        sys.exit(1)

    # 3. Push all events fresh.
    created = errors = 0
    # Track (date, title) to handle the rare duplicate-title-same-day case
    seen_keys: dict[tuple[str, str], int] = {}

    for date_str, event in api_events:
        title = event.get("title", "")
        if not title:
            log.warning("Skipping event with no title on %s", date_str)
            continue

        key = (date_str, title)
        index = seen_keys.get(key, 0)
        seen_keys[key] = index + 1

        uid = make_uid(date_str, title, index)
        try:
            ics = build_ics(uid, date_str, event)
            caldav_put(uid, ics)
            log.info("CREATED %s — %s %s", uid, date_str, title)
            created += 1
        except Exception as exc:
            log.error("Failed to create event %s on %s: %s", title, date_str, exc)
            errors += 1

    log.info("Sync complete — created: %d, errors: %d", created, errors)
    if errors:
        sys.exit(1)


if __name__ == "__main__":
    sync()
