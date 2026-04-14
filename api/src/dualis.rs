/// Dualis / CampusNet scraper.
///
/// The system embeds the session token directly in ARGUMENTS URL params,
/// so we extract it from the redirect after login rather than relying on
/// cookies alone (though we keep the cookie jar too, for safety).
///
/// URL pattern:
///   /scripts/mgrqispi.dll?APPNAME=CampusNet&PRGNAME=<page>&ARGUMENTS=-N<session>,...
use chrono::{Datelike, Days, IsoWeek, NaiveDate, Weekday};
use reqwest::{Client, ClientBuilder, redirect};
use scraper::{Html, Selector};
use serde::Serialize;
use tracing::debug;

use crate::error::AppError;

const BASE: &str = "https://dualis.dhbw.de";
const DLL: &str = "/scripts/mgrqispi.dll";

// ── Public types ──────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct Timetable {
    pub week: String,
    pub prev_week: String,
    pub next_week: String,
    pub days: Vec<Day>,
}

#[derive(Debug, Serialize)]
pub struct Day {
    pub date: String,
    pub weekday: String,
    pub events: Vec<Event>,
}

#[derive(Debug, Serialize)]
pub struct Event {
    pub time: String,
    pub title: String,
    pub room: Option<String>,
    pub lecturer: Option<String>,
    pub event_type: Option<String>,
    pub is_exam: bool,
}

// ── Client ────────────────────────────────────────────────────────────────────

pub struct DualisClient {
    http: Client,
}

impl DualisClient {
    pub fn new() -> Result<Self, AppError> {
        let http = ClientBuilder::new()
            .cookie_store(true)
            .redirect(redirect::Policy::limited(10))
            .user_agent("Mozilla/5.0 (compatible; dualis-scraper/1.0)")
            .build()?;
        Ok(Self { http })
    }

    /// Log in, scrape the timetable for the given ISO week, return structured data.
    pub async fn fetch_timetable(
        &self,
        username: &str,
        password: &str,
        week: IsoWeek,
    ) -> Result<Timetable, AppError> {
        let session = self.login(username, password).await?;
        let html = self.timetable_html(&session, week).await?;
        parse_timetable(&html, week)
    }

    /// Log in once and fetch timetables for multiple weeks with the same session.
    pub async fn fetch_timetables(
        &self,
        username: &str,
        password: &str,
        weeks: &[IsoWeek],
    ) -> Result<Vec<Timetable>, AppError> {
        let session = self.login(username, password).await?;
        let mut timetables = Vec::with_capacity(weeks.len());
        for &week in weeks {
            let html = self.timetable_html(&session, week).await?;
            timetables.push(parse_timetable(&html, week)?);
        }
        Ok(timetables)
    }

    /// Return the raw timetable HTML for debugging the table structure.
    pub async fn fetch_timetable_raw(
        &self,
        username: &str,
        password: &str,
        week: IsoWeek,
    ) -> Result<String, AppError> {
        let session = self.login(username, password).await?;
        self.timetable_html(&session, week).await
    }
}

// ── Login ─────────────────────────────────────────────────────────────────────

impl DualisClient {
    async fn login(&self, username: &str, password: &str) -> Result<Session, AppError> {
        let login_page_url = format!(
            "{BASE}{DLL}?APPNAME=CampusNet&PRGNAME=EXTERNALPAGES&ARGUMENTS=-N000000000000001,-N000324,-Awelcome"
        );

        // GET the login page first so any session cookies are set before the POST.
        self.http.get(&login_page_url).send().await?;

        let url = format!("{BASE}{DLL}");

        let params = [
            ("usrname", username),
            ("pass", password),
            ("APPNAME", "CampusNet"),
            ("PRGNAME", "LOGINCHECK"),
            ("ARGUMENTS", "clino,usrname,pass,menuno,menu_type,browser,platform"),
            ("clino", "000000000000001"),
            ("menuno", "000324"),
            ("menu_type", "classic"),
            ("browser", ""),
            ("platform", ""),
        ];

        let resp: reqwest::Response = self
            .http
            .post(&url)
            .header("Referer", &login_page_url)
            .header("Origin", BASE)
            .form(&params)
            .send()
            .await?;

        let status = resp.status();

        // Dualis returns 200 with a non-standard `Refresh` header on success:
        //   Refresh: 0; URL=/scripts/mgrqispi.dll?...&ARGUMENTS=-N<token>,...
        // (Not a 302 — reqwest's redirect following doesn't apply here.)
        let refresh: Option<String> = resp
            .headers()
            .get("refresh")
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string());

        debug!(?refresh, %status, "Login response");

        let refresh_url = refresh.as_deref().and_then(parse_refresh_url);

        let redirect_url: String = match refresh_url {
            Some(u) => u,
            None => {
                let body: String = resp.text().await.unwrap_or_default();
                let dualis_error = extract_login_error(&body);
                return Err(AppError::LoginFailed(format!(
                    "No Refresh header after login (status {status}). {}",
                    dualis_error.unwrap_or_else(|| "Check DUALIS_USERNAME and DUALIS_PASSWORD in .env".into())
                )));
            }
        };

        let session_token = extract_first_argument(&redirect_url).ok_or_else(|| {
            AppError::LoginFailed(format!(
                "Could not extract session token from Refresh URL: {redirect_url}"
            ))
        })?;

        // Reject the zero-padded placeholder — means we're not actually logged in.
        if session_token.starts_with("00000000000000") {
            return Err(AppError::LoginFailed(format!(
                "Session token looks like a placeholder ({session_token}). \
                 Check credentials and try again."
            )));
        }

        debug!(session_token, %redirect_url, "Login successful");
        Ok(Session { token: session_token })
    }
}

// ── Timetable fetch ───────────────────────────────────────────────────────────

impl DualisClient {
    async fn timetable_html(&self, session: &Session, week: IsoWeek) -> Result<String, AppError> {
        // Hit the main page first so Dualis registers our session as active.
        let main_url = format!(
            "{BASE}{DLL}?APPNAME=CampusNet&PRGNAME=MLSSTART&ARGUMENTS=-N{},-N000019,",
            session.token
        );
        let body: String = self
            .http
            .get(&main_url)
            .send()
            .await?
            .error_for_status()
            .map_err(AppError::Http)?
            .text()
            .await?;

        // Extract the Stundenplan menu number from the nav (usually 000028 but may vary).
        let menu_no = extract_scheduler_menu_no(&body, &session.token)
            .unwrap_or_else(|| "000028".to_string());

        // Compute Monday of the target week for the URL date parameter.
        let monday = NaiveDate::from_isoywd_opt(week.year(), week.week(), Weekday::Mon)
            .ok_or_else(|| AppError::Parse("Invalid ISO week".into()))?;
        let date_str = monday.format("%d.%m.%Y").to_string();

        // Full-week view URL. The format is:
        //   ARGUMENTS=-N<session>,-N<menuno>,-A<DD.MM.YYYY>,-A,-N1,-N000000000000000
        // The trailing -N1 would restrict to work days (Mon–Fri); omitting it shows Mon–Sun.
        let url = format!(
            "{BASE}{DLL}?APPNAME=CampusNet&PRGNAME=SCHEDULER\
             &ARGUMENTS=-N{},-N{},-A{},-A,-N1,-N000000000000000",
            session.token, menu_no, date_str,
        );

        debug!(%url, "Fetching week view");

        let html: String = self
            .http
            .get(&url)
            .send()
            .await?
            .error_for_status()
            .map_err(AppError::Http)?
            .text()
            .await?;

        Ok(html)
    }
}

// ── HTML parsing ──────────────────────────────────────────────────────────────

/// Parse the week view timetable HTML.
///
/// Dualis week view structure:
///   <table>
///     <tr>
///       <th class="tbtime.. time">HH:MM</th>          ← time label
///       <td abbr="Montag Spalte 1"> ... </td>         ← empty cell
///       <td abbr="Dienstag Spalte 1" rowspan="N">     ← event cell (spans N×15 min)
///         HH:MM - HH:MM ROOM\nCourse Title
///       </td>
///     </tr>
///     ...
///   </table>
///
/// Because rowspan cells don't reappear in subsequent rows, the simplest
/// approach is to scan ALL <td abbr="..."> cells and extract their day
/// directly from the abbr attribute instead of tracking row/column position.
fn parse_timetable(html: &str, week: IsoWeek) -> Result<Timetable, AppError> {
    let doc = Html::parse_document(html);

    // Build day structs for Mon–Sun (Sat/Sun dropped at end if empty).
    let all_weekdays = [
        Weekday::Mon, Weekday::Tue, Weekday::Wed,
        Weekday::Thu, Weekday::Fri, Weekday::Sat, Weekday::Sun,
    ];
    let mut days: Vec<Day> = all_weekdays
        .iter()
        .filter_map(|&wd| {
            NaiveDate::from_isoywd_opt(week.year(), week.week(), wd).map(|d| Day {
                date: d.format("%d.%m.%Y").to_string(),
                weekday: weekday_name(wd).to_string(),
                events: vec![],
            })
        })
        .collect();

    // Event cells have class="appointment" — empty time-slot cells don't.
    let cell_sel = Selector::parse("td.appointment[abbr]").unwrap();
    let time_sel = Selector::parse("span.timePeriod").unwrap();
    let link_sel = Selector::parse("a.link").unwrap();

    for cell in doc.select(&cell_sel) {
        let abbr = cell.attr("abbr").unwrap_or("");
        let day_idx = match german_weekday_index(abbr) {
            Some(i) => i,
            None => continue,
        };
        if day_idx >= days.len() {
            continue;
        }

        // Time and room come from the <span class="timePeriod"> text.
        // The content is a single text node with time and room separated by newlines:
        //   "08:30 - 12:00\n                HOR-121"
        let (time, room) = cell
            .select(&time_sel)
            .next()
            .map(|span| {
                let raw = span.text().collect::<String>();
                let parts: Vec<&str> = raw
                    .split('\n')
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .collect();
                let time = parts.first().copied().unwrap_or("").to_string();
                let room = parts.get(1).copied().map(|s| s.to_string());
                (time, room)
            })
            .unwrap_or_default();

        // Title comes from the <a class="link"> text.
        let title = cell
            .select(&link_sel)
            .next()
            .map(|a| a.text().collect::<String>().trim().to_string())
            .unwrap_or_default();

        if title.is_empty() {
            continue;
        }

        let is_exam = cell
            .attr("style")
            .map(|s| s.to_lowercase().contains("background-color:#ff6666"))
            .unwrap_or(false);

        debug!(day = %days[day_idx].weekday, %title, %time, "Event");
        days[day_idx].events.push(Event {
            time,
            title,
            room,
            lecturer: None,
            event_type: None,
            is_exam,
        });
    }

    // Drop weekend days with no events for cleaner output.
    days.retain(|d| d.weekday != "Saturday" && d.weekday != "Sunday" || !d.events.is_empty());

    let monday = NaiveDate::from_isoywd_opt(week.year(), week.week(), Weekday::Mon)
        .ok_or_else(|| AppError::Parse("Invalid ISO week".into()))?;
    let prev = monday.checked_sub_days(Days::new(7)).map(|d| d.iso_week());
    let next = monday.checked_add_days(Days::new(7)).map(|d| d.iso_week());
    let fmt = |w: IsoWeek| format!("{}-W{:02}", w.year(), w.week());

    Ok(Timetable {
        week: fmt(week),
        prev_week: prev.map(fmt).unwrap_or_default(),
        next_week: next.map(fmt).unwrap_or_default(),
        days,
    })
}


// ── Utility ───────────────────────────────────────────────────────────────────

struct Session {
    token: String,
}

/// Parse the URL out of a `Refresh` header value.
/// Format: "0; URL=/path/to/page" or "0;URL=..."
fn parse_refresh_url(refresh: &str) -> Option<String> {
    // Find "URL=" case-insensitively
    let url_part = refresh
        .split(';')
        .find(|part| part.trim().to_uppercase().starts_with("URL="))?;
    let url = url_part.trim().splitn(2, '=').nth(1)?.trim().to_string();
    if url.is_empty() { None } else { Some(url) }
}

/// Extract the first -N<token> value from an ARGUMENTS parameter.
/// e.g. "ARGUMENTS=-N441417119100802,-N000019," → "441417119100802"
fn extract_first_argument(url: &str) -> Option<String> {
    let args = url.split("ARGUMENTS=").nth(1)?;
    let first = args.split(',').next()?;
    let token = first.trim_start_matches("-N");
    if token.is_empty() { None } else { Some(token.to_string()) }
}

/// Extract the Stundenplan menu number from the dashboard nav.
/// e.g. href="...PRGNAME=SCHEDULER&ARGUMENTS=-N<session>,-N000028,..." → "000028"
fn extract_scheduler_menu_no(html: &str, session_token: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    let sel = Selector::parse("a[href*='SCHEDULER']").unwrap();
    for el in doc.select(&sel) {
        let href = el.attr("href").unwrap_or("");
        if !href.contains(session_token) {
            continue;
        }
        // ARGUMENTS=-N<session>,-N<menu>,...
        if let Some(args) = href.split("ARGUMENTS=").nth(1) {
            let parts: Vec<&str> = args.split(',').collect();
            if parts.len() >= 2 {
                let menu = parts[1].trim_start_matches("-N");
                if !menu.is_empty() {
                    return Some(menu.to_string());
                }
            }
        }
    }
    None
}

/// Map the German weekday prefix in an `abbr` attribute to a 0-based index (Mon=0).
/// e.g. "Dienstag Spalte 1" → Some(1)
fn german_weekday_index(abbr: &str) -> Option<usize> {
    let lower = abbr.to_lowercase();
    if lower.starts_with("montag") { Some(0) }
    else if lower.starts_with("dienstag") { Some(1) }
    else if lower.starts_with("mittwoch") { Some(2) }
    else if lower.starts_with("donnerstag") { Some(3) }
    else if lower.starts_with("freitag") { Some(4) }
    else if lower.starts_with("samstag") { Some(5) }
    else if lower.starts_with("sonntag") { Some(6) }
    else { None }
}

fn weekday_name(w: Weekday) -> &'static str {
    match w {
        Weekday::Mon => "Monday",
        Weekday::Tue => "Tuesday",
        Weekday::Wed => "Wednesday",
        Weekday::Thu => "Thursday",
        Weekday::Fri => "Friday",
        Weekday::Sat => "Saturday",
        Weekday::Sun => "Sunday",
    }
}

/// Try to extract Dualis's own error message from a failed login page.
fn extract_login_error(html: &str) -> Option<String> {
    let doc = Html::parse_document(html);
    for sel_str in &[".errors", ".errormessage", ".errorMessage", "p.error", "div.error", "span.error"] {
        if let Ok(sel) = Selector::parse(sel_str) {
            if let Some(el) = doc.select(&sel).next() {
                let text = el.text().collect::<String>().trim().to_string();
                if !text.is_empty() {
                    return Some(format!("Dualis says: \"{text}\""));
                }
            }
        }
    }
    None
}
