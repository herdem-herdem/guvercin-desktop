//! Calendar: a full personal calendar with multiple colored calendars, recurring
//! events, reminders, attendees and iCalendar (.ics) import/export.
//!
//! The canonical representation of an event is [`EventCard`], serialized into the
//! `events.event_json` column. A handful of flat columns (title, start/end, all-day
//! and recurrence flags) are kept in sync on every write so range queries and
//! sorting stay cheap and never have to parse JSON per row.
//!
//! Times are stored as **naive wall-clock epoch millis**: the ISO-local string
//! (`2026-07-23T09:00`) is parsed as if it were UTC. The frontend computes its
//! query window the same way, so month/week/day views line up without any
//! timezone drift regardless of the machine's locale.
//!
//! Recurring events keep a single master row (`recurs = 1`) plus an `RRULE`-style
//! [`Recurrence`]; the concrete occurrences are expanded on read, only within the
//! requested window, and returned as individual [`EventInstance`]s.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header::CONTENT_DISPOSITION, header::CONTENT_TYPE},
    response::Response,
    Json,
};
use chrono::{DateTime, Datelike, Duration, Months, NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, SqlitePool};

use crate::{db, db::AppState, error::AppError};

// ─────────────────────────── Data model ───────────────────────────

fn default_interval() -> i64 {
    1
}
fn default_true() -> bool {
    true
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Attendee {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub email: String,
    #[serde(default)]
    pub status: String, // "", "accepted", "declined", "tentative"
}

#[derive(Serialize, Deserialize, Clone)]
pub struct Recurrence {
    /// "none" | "daily" | "weekly" | "monthly" | "yearly"
    #[serde(default)]
    pub freq: String,
    #[serde(default = "default_interval")]
    pub interval: i64,
    /// Inclusive end date "YYYY-MM-DD", or "" for open-ended.
    #[serde(default)]
    pub until: String,
    /// Max number of occurrences, 0 = unlimited.
    #[serde(default)]
    pub count: i64,
    /// Weekly BYDAY, 0 = Sunday .. 6 = Saturday.
    #[serde(default, rename = "byWeekday")]
    pub by_weekday: Vec<i64>,
}

impl Default for Recurrence {
    fn default() -> Self {
        Recurrence {
            freq: "none".to_string(),
            interval: 1,
            until: String::new(),
            count: 0,
            by_weekday: Vec::new(),
        }
    }
}

impl Recurrence {
    fn is_active(&self) -> bool {
        matches!(
            self.freq.as_str(),
            "daily" | "weekly" | "monthly" | "yearly"
        )
    }
    fn step(&self) -> i64 {
        self.interval.max(1)
    }
}

#[derive(Serialize, Deserialize, Clone)]
pub struct EventCard {
    #[serde(default)]
    pub uid: String,
    #[serde(default, rename = "calendarId")]
    pub calendar_id: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, rename = "allDay")]
    pub all_day: bool,
    /// ISO local start: "YYYY-MM-DDTHH:MM" (timed) or "YYYY-MM-DD" (all-day).
    #[serde(default)]
    pub start: String,
    /// ISO local end. For all-day events this is the inclusive last day.
    #[serde(default)]
    pub end: String,
    /// Optional per-event color override; empty falls back to the calendar color.
    #[serde(default)]
    pub color: String,
    /// "confirmed" | "tentative" | "cancelled"
    #[serde(default)]
    pub status: String,
    #[serde(default = "default_true")]
    pub busy: bool,
    #[serde(default)]
    pub attendees: Vec<Attendee>,
    /// Minutes-before reminders.
    #[serde(default)]
    pub reminders: Vec<i64>,
    #[serde(default)]
    pub recurrence: Recurrence,
}

impl Default for EventCard {
    fn default() -> Self {
        EventCard {
            uid: String::new(),
            calendar_id: None,
            title: String::new(),
            location: String::new(),
            description: String::new(),
            all_day: false,
            start: String::new(),
            end: String::new(),
            color: String::new(),
            status: "confirmed".to_string(),
            busy: true,
            attendees: Vec::new(),
            reminders: Vec::new(),
            recurrence: Recurrence::default(),
        }
    }
}

/// One concrete occurrence handed to the frontend. `card.start`/`card.end` are the
/// occurrence's own times; `start_ms`/`end_ms` are the naive-UTC millis used for
/// positioning; `id` always refers to the stored master row.
#[derive(Serialize)]
pub struct EventInstance {
    pub id: i64,
    #[serde(rename = "calendarId")]
    pub calendar_id: Option<i64>,
    #[serde(rename = "calendarName")]
    pub calendar_name: String,
    #[serde(rename = "calendarColor")]
    pub calendar_color: String,
    pub card: EventCard,
    #[serde(rename = "startMs")]
    pub start_ms: i64,
    #[serde(rename = "endMs")]
    pub end_ms: i64,
    #[serde(rename = "isRecurring")]
    pub is_recurring: bool,
    /// Stable key for an individual occurrence, e.g. "12@1753254000000".
    #[serde(rename = "instanceKey")]
    pub instance_key: String,
}

#[derive(Serialize)]
pub struct CalendarSummary {
    #[serde(rename = "calendarId")]
    pub calendar_id: i64,
    pub name: String,
    pub color: String,
    #[serde(rename = "isVisible")]
    pub is_visible: bool,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
    pub count: i64,
}

// ─────────────────────────── Time helpers ───────────────────────────

const DAY_MS: i64 = 86_400_000;

/// Parse an ISO-local date or datetime as naive-UTC epoch millis.
fn parse_iso_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    for fmt in [
        "%Y-%m-%dT%H:%M:%S",
        "%Y-%m-%dT%H:%M",
        "%Y-%m-%d %H:%M:%S",
        "%Y-%m-%d %H:%M",
    ] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc().timestamp_millis());
    }
    None
}

fn dt_from_ms(ms: i64) -> NaiveDateTime {
    DateTime::from_timestamp_millis(ms)
        .map(|d| d.naive_utc())
        .unwrap_or_default()
}

fn iso_datetime(ms: i64) -> String {
    dt_from_ms(ms).format("%Y-%m-%dT%H:%M").to_string()
}

fn iso_date(ms: i64) -> String {
    dt_from_ms(ms).format("%Y-%m-%d").to_string()
}

/// Resolve a card's start/end into a naive-UTC millis range.
/// Timed events use their exact bounds (min 1h). All-day events span whole days:
/// start = start-date 00:00, end = (end-date + 1 day) 00:00, i.e. end-exclusive.
fn compute_bounds(card: &EventCard) -> (i64, i64) {
    let start = parse_iso_ms(&card.start).unwrap_or(0);
    if card.all_day {
        let end_day = parse_iso_ms(&card.end).unwrap_or(start);
        // Snap to midnight and make the end exclusive (last day fully included).
        let start_day = (start / DAY_MS) * DAY_MS;
        let mut end = ((end_day / DAY_MS) * DAY_MS) + DAY_MS;
        if end <= start_day {
            end = start_day + DAY_MS;
        }
        (start_day, end)
    } else {
        let mut end = parse_iso_ms(&card.end).unwrap_or(start);
        if end <= start {
            end = start + 3_600_000; // default 1h
        }
        (start, end)
    }
}

fn recur_until_ms(card: &EventCard) -> Option<i64> {
    if !card.recurrence.is_active() {
        return None;
    }
    let u = card.recurrence.until.trim();
    if u.is_empty() {
        return None;
    }
    // Inclusive UNTIL date → end of that day.
    parse_iso_ms(u).map(|ms| (ms / DAY_MS) * DAY_MS + DAY_MS - 1)
}

/// Advance a naive datetime by `k` periods of the given recurrence frequency.
fn advance(base: NaiveDateTime, freq: &str, step: i64, k: i64) -> NaiveDateTime {
    let n = step * k;
    match freq {
        "daily" => base + Duration::days(n),
        "weekly" => base + Duration::days(n * 7),
        "monthly" => {
            if n >= 0 {
                base.checked_add_months(Months::new(n as u32)).unwrap_or(base)
            } else {
                base.checked_sub_months(Months::new((-n) as u32))
                    .unwrap_or(base)
            }
        }
        "yearly" => {
            if n >= 0 {
                base.checked_add_months(Months::new((n * 12) as u32))
                    .unwrap_or(base)
            } else {
                base.checked_sub_months(Months::new((-n * 12) as u32))
                    .unwrap_or(base)
            }
        }
        _ => base,
    }
}

/// Expand a master event into concrete occurrences overlapping the window
/// `[from, to)` (naive-UTC millis). Non-recurring events yield at most one.
fn expand_occurrences(card: &EventCard, from: i64, to: i64) -> Vec<(i64, i64)> {
    let (start_ms, end_ms) = compute_bounds(card);
    let duration = (end_ms - start_ms).max(0);
    let rec = &card.recurrence;

    if !rec.is_active() {
        if start_ms < to && end_ms > from {
            return vec![(start_ms, end_ms)];
        }
        return Vec::new();
    }

    let until = recur_until_ms(card).unwrap_or(i64::MAX);
    let hard_to = to.min(until.saturating_add(duration));
    let base = dt_from_ms(start_ms);
    let step = rec.step();
    let mut out: Vec<(i64, i64)> = Vec::new();
    let mut produced = 0i64;
    let cap = 2000; // safety bound on iterations

    let weekly_by_days = rec.freq == "weekly" && !rec.by_weekday.is_empty();

    // Fast-forward the period index so the first candidate lands near `from`
    // instead of looping from a possibly-distant master start.
    let period_ms: i64 = match rec.freq.as_str() {
        "daily" => DAY_MS * step,
        "weekly" => DAY_MS * 7 * step,
        "monthly" => 30 * DAY_MS * step,
        "yearly" => 365 * DAY_MS * step,
        _ => DAY_MS,
    };
    let mut k: i64 = if from > start_ms && period_ms > 0 {
        (((from - start_ms) / period_ms) - 2).max(0)
    } else {
        0
    };

    let mut iterations = 0;
    loop {
        iterations += 1;
        if iterations > cap {
            break;
        }
        let occ_base = advance(base, &rec.freq, step, k);
        let occ_base_ms = occ_base.and_utc().timestamp_millis();

        // Candidate occurrence start times for this period.
        let candidates: Vec<i64> = if weekly_by_days {
            // Occurrences fall on selected weekdays within the week of `occ_base`.
            // Align to the Sunday of that week, then offset by each weekday.
            let wd = occ_base.weekday().num_days_from_sunday() as i64;
            let week_sunday_ms = occ_base_ms - wd * DAY_MS;
            let mut days: Vec<i64> = rec
                .by_weekday
                .iter()
                .filter(|d| (0..=6).contains(*d))
                .map(|d| week_sunday_ms + d * DAY_MS)
                .collect();
            days.sort_unstable();
            days
        } else {
            vec![occ_base_ms]
        };

        // Stop once even the earliest candidate is past the window.
        let min_candidate = candidates.iter().copied().min().unwrap_or(occ_base_ms);
        if min_candidate >= hard_to {
            break;
        }

        for cand in candidates {
            if cand < start_ms {
                continue; // never emit before the series start
            }
            if cand > until {
                continue;
            }
            let occ_start = cand;
            let occ_end = occ_start + duration;
            if occ_start < to && occ_end > from {
                out.push((occ_start, occ_end));
            }
            // COUNT limits total occurrences produced across the whole series,
            // not just the windowed ones, so count every candidate at/after start.
            produced += 1;
            if rec.count > 0 && produced >= rec.count {
                return dedup_sorted(out);
            }
        }
        k += 1;
    }
    dedup_sorted(out)
}

fn dedup_sorted(mut v: Vec<(i64, i64)>) -> Vec<(i64, i64)> {
    v.sort_unstable();
    v.dedup();
    v
}

// ─────────────────────────── DB helpers ───────────────────────────

const SELECT_COLS: &str =
    "event_id, calendar_id, uid, title, location, all_day, start_ms, end_ms, recurs, recur_until_ms, event_json, created_at, updated_at";

fn row_to_card(row: &sqlx::sqlite::SqliteRow) -> (i64, EventCard) {
    let id: i64 = row.try_get("event_id").unwrap_or_default();
    let card_json: Option<String> = row.try_get("event_json").ok().flatten();
    let mut card = card_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<EventCard>(s).ok())
        .unwrap_or_default();
    // Trust the stored calendar_id column as authoritative.
    if let Ok(cid) = row.try_get::<Option<i64>, _>("calendar_id") {
        card.calendar_id = cid;
    }
    (id, card)
}

async fn ensure_default_calendar(pool: &SqlitePool) -> Result<i64, AppError> {
    if let Some(id) =
        sqlx::query_scalar::<_, i64>("SELECT calendar_id FROM calendars ORDER BY is_default DESC, calendar_id ASC LIMIT 1")
            .fetch_optional(pool)
            .await?
    {
        return Ok(id);
    }
    let res = sqlx::query(
        "INSERT INTO calendars (name, color, is_visible, is_default, sort_order) VALUES (?, ?, 1, 1, 0)",
    )
    .bind("My Calendar")
    .bind("#246bce")
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

async fn calendar_lookup(pool: &SqlitePool) -> std::collections::HashMap<i64, (String, String)> {
    let mut map = std::collections::HashMap::new();
    if let Ok(rows) = sqlx::query("SELECT calendar_id, name, color FROM calendars")
        .fetch_all(pool)
        .await
    {
        for r in rows {
            let id: i64 = r.try_get("calendar_id").unwrap_or_default();
            let name: String = r.try_get("name").unwrap_or_default();
            let color: String = r.try_get("color").unwrap_or_default();
            map.insert(id, (name, color));
        }
    }
    map
}

async fn upsert_event(
    pool: &SqlitePool,
    id: Option<i64>,
    card: &EventCard,
) -> Result<i64, AppError> {
    let (start_ms, end_ms) = compute_bounds(card);
    let recurs = card.recurrence.is_active();
    let until_ms = recur_until_ms(card);
    let event_json = serde_json::to_string(card).unwrap_or_else(|_| "{}".to_string());

    if let Some(id) = id {
        sqlx::query(
            r#"
            UPDATE events SET
                calendar_id = ?, uid = ?, title = ?, location = ?, all_day = ?,
                start_ms = ?, end_ms = ?, recurs = ?, recur_until_ms = ?,
                event_json = ?, updated_at = CURRENT_TIMESTAMP
            WHERE event_id = ?
            "#,
        )
        .bind(card.calendar_id)
        .bind(&card.uid)
        .bind(&card.title)
        .bind(&card.location)
        .bind(card.all_day as i64)
        .bind(start_ms)
        .bind(end_ms)
        .bind(recurs as i64)
        .bind(until_ms)
        .bind(&event_json)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(id)
    } else {
        let res = sqlx::query(
            r#"
            INSERT INTO events
                (calendar_id, uid, title, location, all_day, start_ms, end_ms,
                 recurs, recur_until_ms, event_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(card.calendar_id)
        .bind(&card.uid)
        .bind(&card.title)
        .bind(&card.location)
        .bind(card.all_day as i64)
        .bind(start_ms)
        .bind(end_ms)
        .bind(recurs as i64)
        .bind(until_ms)
        .bind(&event_json)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }
}

async fn fetch_card(pool: &SqlitePool, id: i64) -> Result<Option<(i64, EventCard)>, AppError> {
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM events WHERE event_id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| row_to_card(&r)))
}

/// Upsert an event keyed on its stored UID — used by external (Google) sync so
/// re-running a sync updates the same rows instead of creating duplicates.
pub async fn upsert_event_by_uid(pool: &SqlitePool, card: &EventCard) -> Result<i64, AppError> {
    let existing: Option<i64> = if card.uid.trim().is_empty() {
        None
    } else {
        sqlx::query_scalar("SELECT event_id FROM events WHERE uid = ? LIMIT 1")
            .bind(&card.uid)
            .fetch_optional(pool)
            .await?
    };
    upsert_event(pool, existing, card).await
}

/// Find or create a calendar by name, keeping its color in sync. Used to mirror
/// remote (Google) calendars into local ones.
pub async fn ensure_named_calendar(
    pool: &SqlitePool,
    name: &str,
    color: &str,
) -> Result<i64, AppError> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT calendar_id FROM calendars WHERE name = ? LIMIT 1")
        .bind(name)
        .fetch_optional(pool)
        .await?
    {
        let _ = sqlx::query("UPDATE calendars SET color = ? WHERE calendar_id = ?")
            .bind(sanitize_color(color))
            .bind(id)
            .execute(pool)
            .await;
        return Ok(id);
    }
    let res = sqlx::query(
        "INSERT INTO calendars (name, color, is_visible, is_default, sort_order) VALUES (?, ?, 1, 0, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM calendars))",
    )
    .bind(name)
    .bind(sanitize_color(color))
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

// ─────────────────────────── Event handlers ───────────────────────────

#[derive(Deserialize)]
pub struct RangeQuery {
    #[serde(default)]
    pub from: Option<i64>,
    #[serde(default)]
    pub to: Option<i64>,
    #[serde(default)]
    pub search: Option<String>,
    /// Comma-separated calendar ids to include (visible ones). Empty = all.
    #[serde(default)]
    pub calendars: Option<String>,
}

fn parse_id_csv(s: &Option<String>) -> Vec<i64> {
    s.as_deref()
        .map(|raw| {
            raw.split(',')
                .filter_map(|x| x.trim().parse::<i64>().ok())
                .collect()
        })
        .unwrap_or_default()
}

pub async fn list_events(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Query(q): Query<RangeQuery>,
) -> Result<Json<Vec<EventInstance>>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    ensure_default_calendar(&pool).await?;

    // Default window: a broad ± range so a bare request still returns something.
    let now = chrono::Utc::now().timestamp_millis();
    let from = q.from.unwrap_or(now - 365 * DAY_MS);
    let to = q.to.unwrap_or(now + 365 * DAY_MS);
    let cal_ids = parse_id_csv(&q.calendars);
    let search = q.search.as_deref().map(str::trim).unwrap_or("").to_lowercase();

    let mut sql = format!("SELECT {SELECT_COLS} FROM events WHERE ");
    // Overlap for one-off events, or a live recurrence that could reach the window.
    sql.push_str(
        "((recurs = 0 AND start_ms < ? AND end_ms > ?) \
          OR (recurs = 1 AND start_ms < ? AND (recur_until_ms IS NULL OR recur_until_ms >= ?)))",
    );
    if !cal_ids.is_empty() {
        let placeholders = cal_ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
        sql.push_str(&format!(" AND calendar_id IN ({placeholders})"));
    }
    if !search.is_empty() {
        sql.push_str(
            " AND (LOWER(COALESCE(title,'')) LIKE ? OR LOWER(COALESCE(location,'')) LIKE ? OR LOWER(COALESCE(event_json,'')) LIKE ?)",
        );
    }
    sql.push_str(" ORDER BY start_ms ASC");

    let mut query = sqlx::query(&sql).bind(to).bind(from).bind(to).bind(from);
    for id in &cal_ids {
        query = query.bind(id);
    }
    if !search.is_empty() {
        let pat = format!("%{search}%");
        query = query.bind(pat.clone()).bind(pat.clone()).bind(pat);
    }

    let rows = query.fetch_all(&pool).await?;
    let cals = calendar_lookup(&pool).await;

    let mut out: Vec<EventInstance> = Vec::new();
    for row in &rows {
        let (id, card) = row_to_card(row);
        let (name, color) = card
            .calendar_id
            .and_then(|cid| cals.get(&cid).cloned())
            .unwrap_or_else(|| (String::new(), "#246bce".to_string()));
        let is_recurring = card.recurrence.is_active();
        for (occ_start, occ_end) in expand_occurrences(&card, from, to) {
            let mut inst_card = card.clone();
            if card.all_day {
                inst_card.start = iso_date(occ_start);
                // stored end is inclusive last day → subtract the exclusive day
                inst_card.end = iso_date((occ_end - 1).max(occ_start));
            } else {
                inst_card.start = iso_datetime(occ_start);
                inst_card.end = iso_datetime(occ_end);
            }
            out.push(EventInstance {
                id,
                calendar_id: card.calendar_id,
                calendar_name: name.clone(),
                calendar_color: color.clone(),
                card: inst_card,
                start_ms: occ_start,
                end_ms: occ_end,
                is_recurring,
                instance_key: format!("{id}@{occ_start}"),
            });
        }
    }
    out.sort_by_key(|e| e.start_ms);
    Ok(Json(out))
}

pub async fn get_event(
    State(state): State<Arc<AppState>>,
    Path((account_id, event_id)): Path<(i64, i64)>,
) -> Result<Json<EventInstance>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let (id, card) = fetch_card(&pool, event_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Event not found".into()))?;
    let cals = calendar_lookup(&pool).await;
    let (name, color) = card
        .calendar_id
        .and_then(|cid| cals.get(&cid).cloned())
        .unwrap_or_else(|| (String::new(), "#246bce".to_string()));
    let (start_ms, end_ms) = compute_bounds(&card);
    let is_recurring = card.recurrence.is_active();
    Ok(Json(EventInstance {
        id,
        calendar_id: card.calendar_id,
        calendar_name: name,
        calendar_color: color,
        card,
        start_ms,
        end_ms,
        is_recurring,
        instance_key: format!("{id}@{start_ms}"),
    }))
}

async fn resolve_calendar_id(pool: &SqlitePool, requested: Option<i64>) -> Result<i64, AppError> {
    if let Some(cid) = requested {
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM calendars WHERE calendar_id = ?")
            .bind(cid)
            .fetch_optional(pool)
            .await?;
        if exists.is_some() {
            return Ok(cid);
        }
    }
    ensure_default_calendar(pool).await
}

pub async fn create_event(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(mut card): Json<EventCard>,
) -> Result<Json<EventInstance>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    if card.uid.trim().is_empty() {
        card.uid = new_uid();
    }
    card.calendar_id = Some(resolve_calendar_id(&pool, card.calendar_id).await?);
    let id = upsert_event(&pool, None, &card).await?;
    load_instance(&pool, id).await
}

pub async fn update_event(
    State(state): State<Arc<AppState>>,
    Path((account_id, event_id)): Path<(i64, i64)>,
    Json(mut card): Json<EventCard>,
) -> Result<Json<EventInstance>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let existing = fetch_card(&pool, event_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Event not found".into()))?;
    if card.uid.trim().is_empty() {
        card.uid = if existing.1.uid.trim().is_empty() {
            new_uid()
        } else {
            existing.1.uid.clone()
        };
    }
    card.calendar_id = Some(resolve_calendar_id(&pool, card.calendar_id).await?);
    upsert_event(&pool, Some(event_id), &card).await?;
    load_instance(&pool, event_id).await
}

async fn load_instance(pool: &SqlitePool, id: i64) -> Result<Json<EventInstance>, AppError> {
    let (id, card) = fetch_card(pool, id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Failed to load event".into()))?;
    let cals = calendar_lookup(pool).await;
    let (name, color) = card
        .calendar_id
        .and_then(|cid| cals.get(&cid).cloned())
        .unwrap_or_else(|| (String::new(), "#246bce".to_string()));
    let (start_ms, end_ms) = compute_bounds(&card);
    let is_recurring = card.recurrence.is_active();
    Ok(Json(EventInstance {
        id,
        calendar_id: card.calendar_id,
        calendar_name: name,
        calendar_color: color,
        card,
        start_ms,
        end_ms,
        is_recurring,
        instance_key: format!("{id}@{start_ms}"),
    }))
}

pub async fn delete_event(
    State(state): State<Arc<AppState>>,
    Path((account_id, event_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    sqlx::query("DELETE FROM events WHERE event_id = ?")
        .bind(event_id)
        .execute(&pool)
        .await?;
    Ok(Json(json!({ "status": "success" })))
}

// ─────────────────────────── Calendar handlers ───────────────────────────

pub async fn get_calendars(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    ensure_default_calendar(&pool).await?;

    let rows = sqlx::query(
        r#"
        SELECT c.calendar_id, c.name, c.color, c.is_visible, c.is_default,
               (SELECT COUNT(*) FROM events e WHERE e.calendar_id = c.calendar_id) AS cnt
        FROM calendars c
        ORDER BY c.is_default DESC, c.sort_order ASC, LOWER(c.name) ASC
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let mut calendars: Vec<CalendarSummary> = Vec::new();
    let mut total: i64 = 0;
    for r in &rows {
        let cnt: i64 = r.try_get("cnt").unwrap_or_default();
        total += cnt;
        calendars.push(CalendarSummary {
            calendar_id: r.try_get("calendar_id").unwrap_or_default(),
            name: r.try_get("name").unwrap_or_default(),
            color: r.try_get("color").unwrap_or_default(),
            is_visible: r.try_get::<i64, _>("is_visible").unwrap_or(1) != 0,
            is_default: r.try_get::<i64, _>("is_default").unwrap_or(0) != 0,
            count: cnt,
        });
    }

    Ok(Json(json!({ "calendars": calendars, "total": total })))
}

#[derive(Deserialize)]
pub struct CalendarBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default, rename = "isVisible")]
    pub is_visible: Option<bool>,
}

pub async fn create_calendar(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<CalendarBody>,
) -> Result<Json<CalendarSummary>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let name = body.name.as_deref().map(str::trim).unwrap_or("").to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("Calendar name is required".into()));
    }
    let color = sanitize_color(body.color.as_deref().unwrap_or("#8e44ad"));
    let res = sqlx::query(
        "INSERT INTO calendars (name, color, is_visible, is_default, sort_order) VALUES (?, ?, 1, 0, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM calendars))",
    )
    .bind(&name)
    .bind(&color)
    .execute(&pool)
    .await?;
    Ok(Json(CalendarSummary {
        calendar_id: res.last_insert_rowid(),
        name,
        color,
        is_visible: true,
        is_default: false,
        count: 0,
    }))
}

pub async fn update_calendar(
    State(state): State<Arc<AppState>>,
    Path((account_id, calendar_id)): Path<(i64, i64)>,
    Json(body): Json<CalendarBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    if let Some(name) = body.name.as_deref().map(str::trim) {
        if !name.is_empty() {
            sqlx::query("UPDATE calendars SET name = ? WHERE calendar_id = ?")
                .bind(name)
                .bind(calendar_id)
                .execute(&pool)
                .await?;
        }
    }
    if let Some(color) = body.color.as_deref() {
        sqlx::query("UPDATE calendars SET color = ? WHERE calendar_id = ?")
            .bind(sanitize_color(color))
            .bind(calendar_id)
            .execute(&pool)
            .await?;
    }
    if let Some(vis) = body.is_visible {
        sqlx::query("UPDATE calendars SET is_visible = ? WHERE calendar_id = ?")
            .bind(vis as i64)
            .bind(calendar_id)
            .execute(&pool)
            .await?;
    }
    Ok(Json(json!({ "status": "success" })))
}

pub async fn delete_calendar(
    State(state): State<Arc<AppState>>,
    Path((account_id, calendar_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM calendars")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    if count <= 1 {
        return Err(AppError::BadRequest(
            "You can't delete your only calendar.".into(),
        ));
    }
    // Events cascade-delete via the foreign key (PRAGMA foreign_keys = ON).
    sqlx::query("DELETE FROM events WHERE calendar_id = ?")
        .bind(calendar_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM calendars WHERE calendar_id = ?")
        .bind(calendar_id)
        .execute(&pool)
        .await?;
    // If we removed the default, promote another calendar.
    let has_default: Option<i64> =
        sqlx::query_scalar("SELECT calendar_id FROM calendars WHERE is_default = 1 LIMIT 1")
            .fetch_optional(&pool)
            .await?;
    if has_default.is_none() {
        let _ = sqlx::query(
            "UPDATE calendars SET is_default = 1 WHERE calendar_id = (SELECT calendar_id FROM calendars ORDER BY calendar_id ASC LIMIT 1)",
        )
        .execute(&pool)
        .await;
    }
    Ok(Json(json!({ "status": "success" })))
}

fn sanitize_color(input: &str) -> String {
    let s = input.trim();
    if s.starts_with('#') && (s.len() == 7 || s.len() == 4) && s[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        s.to_string()
    } else {
        "#246bce".to_string()
    }
}

// ─────────────────────────── ICS import / export ───────────────────────────

#[derive(Deserialize)]
pub struct ImportBody {
    #[serde(default)]
    pub ics: String,
    #[serde(default, rename = "calendarId")]
    pub calendar_id: Option<i64>,
}

pub async fn import_events(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<ImportBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let target = resolve_calendar_id(&pool, body.calendar_id).await?;
    let cards = parse_ics(&body.ics);
    if cards.is_empty() {
        return Err(AppError::BadRequest(
            "No calendar events found in this file. Choose an .ics file exported from another calendar app.".into(),
        ));
    }
    let mut imported = 0i64;
    let mut skipped = 0i64;
    for mut card in cards {
        if card.start.trim().is_empty() {
            skipped += 1;
            continue;
        }
        if card.uid.trim().is_empty() {
            card.uid = new_uid();
        }
        card.calendar_id = Some(target);
        upsert_event(&pool, None, &card).await?;
        imported += 1;
    }
    Ok(Json(json!({ "imported": imported, "skipped": skipped })))
}

#[derive(Deserialize)]
pub struct ExportQuery {
    #[serde(default)]
    pub ids: Option<String>,
    #[serde(default)]
    pub calendars: Option<String>,
}

pub async fn export_events(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;

    let ids = parse_id_csv(&q.ids);
    let cal_ids = parse_id_csv(&q.calendars);
    let mut sql = format!("SELECT {SELECT_COLS} FROM events");
    let mut clauses: Vec<String> = Vec::new();
    if !ids.is_empty() {
        clauses.push(format!(
            "event_id IN ({})",
            ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        ));
    }
    if !cal_ids.is_empty() {
        clauses.push(format!(
            "calendar_id IN ({})",
            cal_ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
        ));
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY start_ms ASC");

    let rows = sqlx::query(&sql).fetch_all(&pool).await?;
    let cards: Vec<EventCard> = rows.iter().map(|r| row_to_card(r).1).collect();
    let ics = build_ics(&cards);

    let mut resp = Response::new(ics.into());
    resp.headers_mut()
        .insert(CONTENT_TYPE, "text/calendar; charset=utf-8".parse().unwrap());
    resp.headers_mut().insert(
        CONTENT_DISPOSITION,
        "attachment; filename=\"guvercin.ics\"".parse().unwrap(),
    );
    Ok(resp)
}

// ── ICS writing ──

fn build_ics(cards: &[EventCard]) -> String {
    let mut out = String::new();
    out.push_str("BEGIN:VCALENDAR\r\n");
    out.push_str("VERSION:2.0\r\n");
    out.push_str("PRODID:-//Guvercin//Calendar//EN\r\n");
    out.push_str("CALSCALE:GREGORIAN\r\n");
    for card in cards {
        push_vevent(&mut out, card);
    }
    out.push_str("END:VCALENDAR\r\n");
    out
}

fn push_vevent(out: &mut String, card: &EventCard) {
    out.push_str("BEGIN:VEVENT\r\n");
    let uid = if card.uid.trim().is_empty() {
        new_uid()
    } else {
        card.uid.clone()
    };
    fold_line(out, &format!("UID:{uid}"));
    fold_line(out, &format!("DTSTAMP:{}", ics_utc_now()));

    let (start_ms, end_ms) = compute_bounds(card);
    if card.all_day {
        fold_line(out, &format!("DTSTART;VALUE=DATE:{}", ics_date(start_ms)));
        // ICS all-day DTEND is exclusive — our end_ms is already exclusive.
        fold_line(out, &format!("DTEND;VALUE=DATE:{}", ics_date(end_ms)));
    } else {
        fold_line(out, &format!("DTSTART:{}", ics_datetime(start_ms)));
        fold_line(out, &format!("DTEND:{}", ics_datetime(end_ms)));
    }
    if !card.title.trim().is_empty() {
        fold_line(out, &format!("SUMMARY:{}", escape_text(&card.title)));
    }
    if !card.location.trim().is_empty() {
        fold_line(out, &format!("LOCATION:{}", escape_text(&card.location)));
    }
    if !card.description.trim().is_empty() {
        fold_line(out, &format!("DESCRIPTION:{}", escape_text(&card.description)));
    }
    match card.status.as_str() {
        "tentative" => fold_line(out, "STATUS:TENTATIVE"),
        "cancelled" => fold_line(out, "STATUS:CANCELLED"),
        _ => {}
    }
    for a in &card.attendees {
        if a.email.trim().is_empty() {
            continue;
        }
        let cn = if a.name.trim().is_empty() {
            String::new()
        } else {
            format!(";CN={}", escape_text(&a.name))
        };
        fold_line(out, &format!("ATTENDEE{cn}:mailto:{}", a.email.trim()));
    }
    if let Some(rrule) = build_rrule(card) {
        fold_line(out, &format!("RRULE:{rrule}"));
    }
    out.push_str("END:VEVENT\r\n");
}

fn build_rrule(card: &EventCard) -> Option<String> {
    let r = &card.recurrence;
    if !r.is_active() {
        return None;
    }
    let mut parts = vec![format!("FREQ={}", r.freq.to_uppercase())];
    if r.interval > 1 {
        parts.push(format!("INTERVAL={}", r.interval));
    }
    if r.count > 0 {
        parts.push(format!("COUNT={}", r.count));
    }
    if !r.until.trim().is_empty() {
        if let Some(ms) = parse_iso_ms(r.until.trim()) {
            parts.push(format!("UNTIL={}", ics_date(ms)));
        }
    }
    if r.freq == "weekly" && !r.by_weekday.is_empty() {
        let days: Vec<&str> = r
            .by_weekday
            .iter()
            .filter_map(|d| weekday_to_ics(*d))
            .collect();
        if !days.is_empty() {
            parts.push(format!("BYDAY={}", days.join(",")));
        }
    }
    Some(parts.join(";"))
}

fn weekday_to_ics(d: i64) -> Option<&'static str> {
    match d {
        0 => Some("SU"),
        1 => Some("MO"),
        2 => Some("TU"),
        3 => Some("WE"),
        4 => Some("TH"),
        5 => Some("FR"),
        6 => Some("SA"),
        _ => None,
    }
}

fn ics_from_weekday(s: &str) -> Option<i64> {
    match s.trim().to_uppercase().as_str() {
        "SU" => Some(0),
        "MO" => Some(1),
        "TU" => Some(2),
        "WE" => Some(3),
        "TH" => Some(4),
        "FR" => Some(5),
        "SA" => Some(6),
        _ => None,
    }
}

fn ics_date(ms: i64) -> String {
    dt_from_ms(ms).format("%Y%m%d").to_string()
}
fn ics_datetime(ms: i64) -> String {
    dt_from_ms(ms).format("%Y%m%dT%H%M%S").to_string()
}
fn ics_utc_now() -> String {
    chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string()
}

fn escape_text(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace('\r', "")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

fn unescape_text(s: &str) -> String {
    let mut out = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            match chars.next() {
                Some('n') | Some('N') => out.push('\n'),
                Some(other) => out.push(other),
                None => {}
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// Fold a logical line at 74 octets per RFC 5545 (continuation lines start with a space).
fn fold_line(out: &mut String, line: &str) {
    let bytes = line.as_bytes();
    if bytes.len() <= 74 {
        out.push_str(line);
        out.push_str("\r\n");
        return;
    }
    let mut start = 0;
    let mut first = true;
    while start < bytes.len() {
        // Respect UTF-8 boundaries when slicing.
        let mut end = (start + if first { 74 } else { 73 }).min(bytes.len());
        while end < bytes.len() && (bytes[end] & 0xC0) == 0x80 {
            end -= 1;
        }
        if !first {
            out.push(' ');
        }
        out.push_str(&line[start..end]);
        out.push_str("\r\n");
        start = end;
        first = false;
    }
}

// ── ICS parsing ──

fn ics_unfold(input: &str) -> Vec<String> {
    let mut logical: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if (line.starts_with(' ') || line.starts_with('\t')) && !logical.is_empty() {
            if let Some(last) = logical.last_mut() {
                last.push_str(&line[1..]);
                continue;
            }
        }
        logical.push(line.to_string());
    }
    logical
}

struct IcsProp {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

fn parse_prop(line: &str) -> Option<IcsProp> {
    let colon = line.find(':')?;
    let (head, value) = line.split_at(colon);
    let value = &value[1..];
    let mut segs = head.split(';');
    let name = segs.next()?.trim().to_uppercase();
    let mut params = Vec::new();
    for seg in segs {
        if let Some(eq) = seg.find('=') {
            params.push((seg[..eq].trim().to_uppercase(), seg[eq + 1..].trim().to_string()));
        } else {
            params.push((seg.trim().to_uppercase(), String::new()));
        }
    }
    Some(IcsProp {
        name,
        params,
        value: value.to_string(),
    })
}

/// Convert an ICS DATE/DATE-TIME value into (iso_string, is_date_only).
fn parse_ics_datetime(value: &str, is_date: bool) -> Option<(String, bool)> {
    let v = value.trim().trim_end_matches('Z');
    if is_date || (v.len() == 8 && !v.contains('T')) {
        // YYYYMMDD
        let d = NaiveDate::parse_from_str(v, "%Y%m%d").ok()?;
        return Some((d.format("%Y-%m-%d").to_string(), true));
    }
    // YYYYMMDDTHHMMSS or YYYYMMDDTHHMM
    for fmt in ["%Y%m%dT%H%M%S", "%Y%m%dT%H%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(v, fmt) {
            return Some((dt.format("%Y-%m-%dT%H:%M").to_string(), false));
        }
    }
    // Fall back to date-only if it looks like a bare date.
    if let Ok(d) = NaiveDate::parse_from_str(v, "%Y%m%d") {
        return Some((d.format("%Y-%m-%d").to_string(), true));
    }
    None
}

pub fn parse_rrule(value: &str) -> Recurrence {
    let mut r = Recurrence::default();
    for part in value.split(';') {
        let mut kv = part.splitn(2, '=');
        let key = kv.next().unwrap_or("").trim().to_uppercase();
        let val = kv.next().unwrap_or("").trim();
        match key.as_str() {
            "FREQ" => {
                r.freq = match val.to_uppercase().as_str() {
                    "DAILY" => "daily",
                    "WEEKLY" => "weekly",
                    "MONTHLY" => "monthly",
                    "YEARLY" => "yearly",
                    _ => "none",
                }
                .to_string();
            }
            "INTERVAL" => r.interval = val.parse().unwrap_or(1).max(1),
            "COUNT" => r.count = val.parse().unwrap_or(0).max(0),
            "UNTIL" => {
                if let Some((iso, _)) = parse_ics_datetime(val, false) {
                    // Keep just the date portion for our simplified UNTIL.
                    r.until = iso.split('T').next().unwrap_or("").to_string();
                }
            }
            "BYDAY" => {
                r.by_weekday = val
                    .split(',')
                    .filter_map(|tok| {
                        // Strip any leading ordinal like "2MO" → "MO".
                        let t = tok.trim_start_matches(|c: char| c == '+' || c == '-' || c.is_ascii_digit());
                        ics_from_weekday(t)
                    })
                    .collect();
            }
            _ => {}
        }
    }
    r
}

fn parse_ics(input: &str) -> Vec<EventCard> {
    let lines = ics_unfold(input);
    let mut cards: Vec<EventCard> = Vec::new();
    let mut cur: Option<EventCard> = None;
    let mut end_is_date = false;

    for line in &lines {
        let upper = line.to_uppercase();
        if upper.starts_with("BEGIN:VEVENT") {
            cur = Some(EventCard::default());
            end_is_date = false;
            continue;
        }
        if upper.starts_with("END:VEVENT") {
            if let Some(mut card) = cur.take() {
                if card.status.is_empty() {
                    card.status = "confirmed".to_string();
                }
                // For all-day events the ICS DTEND is exclusive; convert back to
                // our inclusive last-day representation.
                if card.all_day && end_is_date {
                    if let (Some(s), Some(e)) =
                        (parse_iso_ms(&card.start), parse_iso_ms(&card.end))
                    {
                        if e > s {
                            card.end = iso_date(e - DAY_MS);
                        }
                    }
                }
                cards.push(card);
            }
            continue;
        }
        let Some(card) = cur.as_mut() else { continue };
        let Some(prop) = parse_prop(line) else { continue };
        let is_date_param = prop
            .params
            .iter()
            .any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("DATE"));
        match prop.name.as_str() {
            "UID" => card.uid = prop.value.trim().to_string(),
            "SUMMARY" => card.title = unescape_text(&prop.value),
            "LOCATION" => card.location = unescape_text(&prop.value),
            "DESCRIPTION" => card.description = unescape_text(&prop.value),
            "DTSTART" => {
                if let Some((iso, is_date)) = parse_ics_datetime(&prop.value, is_date_param) {
                    card.start = iso;
                    if is_date {
                        card.all_day = true;
                    }
                }
            }
            "DTEND" | "DTEND;VALUE=DATE" => {
                if let Some((iso, is_date)) = parse_ics_datetime(&prop.value, is_date_param) {
                    card.end = iso;
                    end_is_date = is_date;
                }
            }
            "STATUS" => {
                card.status = match prop.value.trim().to_uppercase().as_str() {
                    "TENTATIVE" => "tentative",
                    "CANCELLED" => "cancelled",
                    _ => "confirmed",
                }
                .to_string();
            }
            "RRULE" => card.recurrence = parse_rrule(&prop.value),
            "ATTENDEE" => {
                let email = prop
                    .value
                    .trim()
                    .trim_start_matches("mailto:")
                    .trim_start_matches("MAILTO:")
                    .trim()
                    .to_string();
                let name = prop
                    .params
                    .iter()
                    .find(|(k, _)| k == "CN")
                    .map(|(_, v)| unescape_text(v.trim_matches('"')))
                    .unwrap_or_default();
                if !email.is_empty() {
                    card.attendees.push(Attendee {
                        name,
                        email,
                        status: String::new(),
                    });
                }
            }
            _ => {}
        }
    }
    cards
}

// ─────────────────────────── misc ───────────────────────────

fn new_uid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!(
        "guvercin-{}-{:08x}",
        chrono::Utc::now().timestamp_millis(),
        nanos
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ev(start: &str, end: &str, all_day: bool) -> EventCard {
        EventCard {
            start: start.into(),
            end: end.into(),
            all_day,
            ..EventCard::default()
        }
    }

    #[test]
    fn timed_bounds_default_hour() {
        let (s, e) = compute_bounds(&ev("2026-07-23T09:00", "", false));
        assert_eq!(e - s, 3_600_000); // empty end → +1h
    }

    #[test]
    fn all_day_end_is_exclusive() {
        // A single all-day event should span exactly one day.
        let (s, e) = compute_bounds(&ev("2026-07-23", "2026-07-23", true));
        assert_eq!(e - s, DAY_MS);
    }

    #[test]
    fn non_recurring_in_window() {
        let c = ev("2026-07-23T09:00", "2026-07-23T10:00", false);
        let from = parse_iso_ms("2026-07-01T00:00").unwrap();
        let to = parse_iso_ms("2026-08-01T00:00").unwrap();
        assert_eq!(expand_occurrences(&c, from, to).len(), 1);
        // Outside the window → nothing.
        let from2 = parse_iso_ms("2026-09-01T00:00").unwrap();
        let to2 = parse_iso_ms("2026-10-01T00:00").unwrap();
        assert_eq!(expand_occurrences(&c, from2, to2).len(), 0);
    }

    #[test]
    fn daily_with_count() {
        let mut c = ev("2026-07-01T09:00", "2026-07-01T10:00", false);
        c.recurrence = Recurrence { freq: "daily".into(), interval: 1, count: 5, ..Default::default() };
        let from = parse_iso_ms("2026-07-01T00:00").unwrap();
        let to = parse_iso_ms("2026-08-01T00:00").unwrap();
        // COUNT=5 caps the series regardless of the (larger) window.
        assert_eq!(expand_occurrences(&c, from, to).len(), 5);
    }

    #[test]
    fn weekly_byweekday_within_window() {
        // Every Mon/Wed/Fri for one week.
        let mut c = ev("2026-07-06T09:00", "2026-07-06T10:00", false); // Monday
        c.recurrence = Recurrence {
            freq: "weekly".into(),
            interval: 1,
            by_weekday: vec![1, 3, 5],
            ..Default::default()
        };
        let from = parse_iso_ms("2026-07-06T00:00").unwrap();
        let to = parse_iso_ms("2026-07-13T00:00").unwrap();
        assert_eq!(expand_occurrences(&c, from, to).len(), 3);
    }

    #[test]
    fn recurrence_far_start_is_fast_forwarded() {
        // Master starts years before the window; expansion must still be cheap
        // and land the right daily occurrences inside a narrow window.
        let mut c = ev("2000-01-01T09:00", "2000-01-01T10:00", false);
        c.recurrence = Recurrence { freq: "daily".into(), interval: 1, ..Default::default() };
        let from = parse_iso_ms("2026-07-01T00:00").unwrap();
        let to = parse_iso_ms("2026-07-08T00:00").unwrap();
        assert_eq!(expand_occurrences(&c, from, to).len(), 7);
    }

    #[test]
    fn ics_roundtrip() {
        let mut c = ev("2026-07-23T09:00", "2026-07-23T10:30", false);
        c.title = "Team sync, weekly".into(); // comma must survive escaping
        c.location = "Room 5".into();
        c.uid = "abc-123".into();
        c.recurrence = Recurrence { freq: "weekly".into(), interval: 1, by_weekday: vec![1, 3], ..Default::default() };
        let ics = build_ics(std::slice::from_ref(&c));
        assert!(ics.contains("BEGIN:VEVENT"));
        assert!(ics.contains("RRULE:FREQ=WEEKLY"));
        let parsed = parse_ics(&ics);
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].title, "Team sync, weekly");
        assert_eq!(parsed[0].start, "2026-07-23T09:00");
        assert_eq!(parsed[0].recurrence.freq, "weekly");
        assert_eq!(parsed[0].recurrence.by_weekday, vec![1, 3]);
    }

    #[test]
    fn ics_all_day_roundtrip() {
        let c = ev("2026-07-23", "2026-07-24", true); // 2-day all-day event (inclusive 23–24)
        let ics = build_ics(std::slice::from_ref(&c));
        // DTEND should be exclusive (the 25th) in the ICS output.
        assert!(ics.contains("DTSTART;VALUE=DATE:20260723"));
        assert!(ics.contains("DTEND;VALUE=DATE:20260725"));
        let parsed = parse_ics(&ics);
        assert_eq!(parsed[0].start, "2026-07-23");
        assert_eq!(parsed[0].end, "2026-07-24"); // back to inclusive
        assert!(parsed[0].all_day);
    }
}
