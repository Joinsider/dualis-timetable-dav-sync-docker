use chrono::{NaiveDate, NaiveTime, Utc};

use crate::config::Config;
use crate::dualis::Timetable;

/// Build a complete iCalendar document from a list of timetables.
pub fn build_calendar(timetables: &[Timetable], config: &Config) -> String {
    let mut out = String::with_capacity(4096);

    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//dualis-scraper//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    out.push_str("METHOD:PUBLISH\r\n");
    fold_and_push(&mut out, &format!("X-WR-CALNAME:{}", config.calendar_name));
    fold_and_push(&mut out, &format!("X-WR-TIMEZONE:{}", config.timezone));

    // VTIMEZONE for Europe/Berlin (CET/CEST)
    if config.timezone == "Europe/Berlin" {
        out.push_str(VTIMEZONE_EUROPE_BERLIN);
    }

    let dtstamp = Utc::now().format("%Y%m%dT%H%M%SZ").to_string();

    for timetable in timetables {
        for day in &timetable.days {
            for (idx, event) in day.events.iter().enumerate() {
                if let Some(vevent) =
                    format_vevent(&day.date, event, idx, config, &dtstamp)
                {
                    out.push_str(&vevent);
                }
            }
        }
    }

    out.push_str("END:VCALENDAR\r\n");
    out
}

fn format_vevent(
    date_str: &str,
    event: &crate::dualis::Event,
    index: usize,
    config: &Config,
    dtstamp: &str,
) -> Option<String> {
    let date = NaiveDate::parse_from_str(date_str, "%d.%m.%Y").ok()?;

    // Parse time field like "08:30 - 12:00"
    let time_parts: Vec<&str> = event.time.splitn(2, " - ").collect();
    if time_parts.len() != 2 {
        return None;
    }
    let start_time = NaiveTime::parse_from_str(time_parts[0].trim(), "%H:%M").ok()?;
    let end_time = NaiveTime::parse_from_str(time_parts[1].trim(), "%H:%M").ok()?;

    let dtstart = format!(
        "{}T{}",
        date.format("%Y%m%d"),
        start_time.format("%H%M%S")
    );
    let dtend = format!(
        "{}T{}",
        date.format("%Y%m%d"),
        end_time.format("%H%M%S")
    );

    let uid = make_uid(date_str, &event.title, index, &config.uid_domain);

    let mut out = String::new();
    out.push_str("BEGIN:VEVENT\r\n");
    fold_and_push(&mut out, &format!("UID:{uid}"));
    fold_and_push(&mut out, &format!("DTSTAMP:{dtstamp}"));
    fold_and_push(
        &mut out,
        &format!("DTSTART;TZID={}:{}", config.timezone, dtstart),
    );
    fold_and_push(
        &mut out,
        &format!("DTEND;TZID={}:{}", config.timezone, dtend),
    );
    let summary = if event.is_exam {
        format!("PRÜFUNG: {}", escape_ical(&event.title))
    } else {
        escape_ical(&event.title)
    };
    fold_and_push(&mut out, &format!("SUMMARY:{}", summary));

    if let Some(ref room) = event.room {
        let mut location = room.clone();
        if let Some(ref etype) = event.event_type {
            location = format!("{location} -- {etype}");
        }
        fold_and_push(&mut out, &format!("LOCATION:{}", escape_ical(&location)));
    }

    let mut desc_parts = Vec::new();
    if let Some(ref lecturer) = event.lecturer {
        desc_parts.push(format!("Lecturer: {lecturer}"));
    }
    if let Some(ref etype) = event.event_type {
        desc_parts.push(format!("Type: {etype}"));
    }
    if !desc_parts.is_empty() {
        fold_and_push(
            &mut out,
            &format!("DESCRIPTION:{}", escape_ical(&desc_parts.join("\\n"))),
        );
    }

    out.push_str("SEQUENCE:0\r\n");
    out.push_str("END:VEVENT\r\n");

    Some(out)
}

/// Generate a stable UID matching the Python script's logic.
fn make_uid(date_str: &str, title: &str, index: usize, domain: &str) -> String {
    let date_slug = date_str.replace('.', "");
    let title_slug: String = title
        .to_lowercase()
        .chars()
        .map(|c| if c.is_alphanumeric() { c } else { '-' })
        .collect();
    let title_slug = title_slug.trim_matches('-');
    format!("{date_slug}-{title_slug}-{index}@{domain}")
}

/// Escape text per RFC 5545.
fn escape_ical(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace(';', "\\;")
        .replace(',', "\\,")
        .replace('\n', "\\n")
}

/// Fold a content line at 75 octets and append to output with CRLF.
fn fold_and_push(out: &mut String, line: &str) {
    let bytes = line.as_bytes();
    if bytes.len() <= 75 {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }

    let mut pos = 0;
    let mut first = true;
    while pos < bytes.len() {
        let max = if first { 75 } else { 74 }; // continuation lines have a leading space
        let mut end = (pos + max).min(bytes.len());

        // Don't split in the middle of a multi-byte UTF-8 character
        while end < bytes.len() && (bytes[end] & 0b1100_0000) == 0b1000_0000 {
            end -= 1;
        }

        if first {
            out.push_str(&line[pos..end]);
            first = false;
        } else {
            out.push_str(" ");
            out.push_str(&line[pos..end]);
        }
        out.push_str("\r\n");
        pos = end;
    }
}

const VTIMEZONE_EUROPE_BERLIN: &str = "\
BEGIN:VTIMEZONE\r\n\
TZID:Europe/Berlin\r\n\
BEGIN:DAYLIGHT\r\n\
TZOFFSETFROM:+0100\r\n\
TZOFFSETTO:+0200\r\n\
TZNAME:CEST\r\n\
DTSTART:19700329T020000\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=3\r\n\
END:DAYLIGHT\r\n\
BEGIN:STANDARD\r\n\
TZOFFSETFROM:+0200\r\n\
TZOFFSETTO:+0100\r\n\
TZNAME:CET\r\n\
DTSTART:19701025T030000\r\n\
RRULE:FREQ=YEARLY;BYDAY=-1SU;BYMONTH=10\r\n\
END:STANDARD\r\n\
END:VTIMEZONE\r\n";
