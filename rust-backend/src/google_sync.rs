//! Two-way sync between the local stores and Google (Calendar v3, People v1,
//! Tasks v1) for accounts signed in with Google (`provider_type = 'gmail'`).
//!
//! Each run: fetch the remote set, reconcile it against local rows (last-write-
//! wins by timestamp — see [`crate::sync_reconcile`]), then push local creates,
//! edits and deletions back to Google. Rows are matched by a stable remote id
//! (stored in `remote_id`, with a fallback to the legacy `uid` written by the
//! earlier pull-only sync). Deletions are conservative: a remote item is deleted
//! only for an explicit local tombstone, and a local row is dropped for a remote
//! deletion only when it is clean and inside the fetched scope.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::{Duration, Local, NaiveDate, NaiveDateTime, TimeZone};
use reqwest::{Method, StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

use crate::{
    calendar_routes as cal, contacts_routes as con, db, db::AppState, error::AppError, oauth,
    sync_reconcile::{reconcile_matched, reconcile_orphan, LocalState, MatchAction, OrphanAction},
    todo_routes as todo,
};

const CAL_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const PEOPLE_URL: &str = "https://people.googleapis.com/v1/people/me/connections";
const TASKLISTS_URL: &str = "https://tasks.googleapis.com/tasks/v1/users/@me/lists";
const DAY_MS: i64 = 86_400_000;
const RECONNECT_MSG: &str =
    "Reconnect your Google account to allow Calendar/Contacts access — sign out and sign in with Google again.";

// ─────────────────────────── shared HTTP ───────────────────────────

async fn gmail_token(state: &Arc<AppState>, account_id: i64) -> Result<String, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider_type FROM accounts WHERE account_id = ?")
            .bind(account_id)
            .fetch_optional(&general)
            .await?;
    if provider.as_deref() != Some(oauth::PROVIDER_GMAIL) {
        return Err(AppError::BadRequest(
            "This account is not a Google account, so there is nothing to sync.".into(),
        ));
    }
    oauth::access_token_for_account(&general, account_id)
        .await
        .map_err(AppError::BadRequest)
}

async fn google_get<T: DeserializeOwned>(
    http: &reqwest::Client,
    url: &str,
    token: &str,
) -> Result<T, AppError> {
    google_send(http, Method::GET, url, token, None).await
}

async fn google_send<T: DeserializeOwned>(
    http: &reqwest::Client,
    method: Method,
    url: &str,
    token: &str,
    body: Option<&Value>,
) -> Result<T, AppError> {
    let mut req = http.request(method, url).bearer_auth(token);
    if let Some(b) = body {
        req = req.json(b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::BadRequest(format!("Google request failed: {e}")))?;
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(AppError::BadRequest(RECONNECT_MSG.into()));
    }
    if !status.is_success() {
        let body = resp.text().await.unwrap_or_default();
        let snippet: String = body.chars().take(200).collect();
        return Err(AppError::BadRequest(format!(
            "Google API error ({}): {}",
            status.as_u16(),
            snippet
        )));
    }
    resp.json::<T>()
        .await
        .map_err(|e| AppError::BadRequest(format!("Could not parse Google response: {e}")))
}

/// DELETE that tolerates already-gone (404/410) as success.
async fn google_delete(http: &reqwest::Client, url: &str, token: &str) -> Result<(), AppError> {
    let resp = http
        .delete(url)
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| AppError::BadRequest(format!("Google request failed: {e}")))?;
    let status = resp.status();
    if status.is_success() || status == StatusCode::NOT_FOUND || status == StatusCode::GONE {
        Ok(())
    } else if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        Err(AppError::BadRequest(RECONNECT_MSG.into()))
    } else {
        Err(AppError::BadRequest(format!(
            "Google delete failed ({})",
            status.as_u16()
        )))
    }
}

fn label_of(t: &Option<String>) -> String {
    match t.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim().to_lowercase(),
        _ => "other".to_string(),
    }
}

fn rfc3339_to_ms(s: &str) -> i64 {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .map(|d| d.timestamp_millis())
        .unwrap_or(0)
}

/// Interpret a naive local ISO string as a timezone-aware RFC3339 timestamp.
fn local_rfc3339(iso: &str) -> String {
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(ndt) = NaiveDateTime::parse_from_str(iso.trim(), fmt) {
            if let Some(dt) = Local.from_local_datetime(&ndt).earliest() {
                return dt.to_rfc3339();
            }
        }
    }
    iso.to_string()
}

fn plus_one_day_date(date: &str) -> String {
    NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .map(|d| (d + Duration::days(1)).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| date.to_string())
}

fn minus_one_day(date: &str) -> String {
    NaiveDate::parse_from_str(date.trim(), "%Y-%m-%d")
        .map(|d| (d - Duration::days(1)).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| date.to_string())
}

fn v_str(v: &Value, key: &str) -> String {
    v.get(key).and_then(|x| x.as_str()).unwrap_or("").to_string()
}

async fn set_remote_id(pool: &SqlitePool, table: &str, id_col: &str, id: i64, remote_id: &str) {
    let _ = sqlx::query(&format!("UPDATE {table} SET remote_id = ? WHERE {id_col} = ?"))
        .bind(remote_id)
        .bind(id)
        .execute(pool)
        .await;
}

// ─────────────────────────── status ───────────────────────────

pub async fn google_status(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider_type FROM accounts WHERE account_id = ?")
            .bind(account_id)
            .fetch_optional(&general)
            .await?;
    let is_gmail = provider.as_deref() == Some(oauth::PROVIDER_GMAIL);
    Ok(Json(json!({
        "provider": provider,
        "gmail": is_gmail,
        "configured": oauth::is_configured(),
        "available": is_gmail && oauth::is_configured(),
    })))
}

/// Probe whether the stored Google token actually grants Calendar access.
///
/// `google_status` only reports whether the account is Gmail and OAuth is built
/// in; it can't tell whether the *calendar* scope was granted. Accounts that
/// signed in before the calendar scope was added keep a mail-only refresh token,
/// so a real API call is the only reliable signal. This does one cheap
/// `calendarList` request (a 401/403 ⇒ not granted). Used by the Calendar tab to
/// decide whether to prompt the user to reconnect.
pub async fn calendar_access(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let provider: Option<String> =
        sqlx::query_scalar("SELECT provider_type FROM accounts WHERE account_id = ?")
            .bind(account_id)
            .fetch_optional(&general)
            .await?;
    let is_gmail = provider.as_deref() == Some(oauth::PROVIDER_GMAIL);
    let configured = oauth::is_configured();
    if !is_gmail || !configured {
        return Ok(Json(json!({ "gmail": is_gmail, "configured": configured, "granted": false })));
    }
    // Best-effort: any failure (no token, revoked scope, transient) ⇒ not granted.
    let granted = match oauth::access_token_for_account(&general, account_id).await {
        Ok(token) => {
            let http = reqwest::Client::new();
            let url = format!("{CAL_LIST_URL}?maxResults=1&fields=kind");
            google_get::<Value>(&http, &url, &token).await.is_ok()
        }
        Err(_) => false,
    };
    Ok(Json(json!({ "gmail": true, "configured": true, "granted": granted })))
}

// ─────────────────────────── Calendar ───────────────────────────

#[derive(Deserialize, Default)]
struct CalListResp {
    #[serde(default)]
    items: Vec<CalListItem>,
}
#[derive(Deserialize, Default)]
struct CalListItem {
    #[serde(default)]
    id: String,
    #[serde(default)]
    summary: String,
    #[serde(default, rename = "backgroundColor")]
    background_color: Option<String>,
    #[serde(default, rename = "accessRole")]
    access_role: Option<String>,
}
#[derive(Deserialize, Default)]
struct EventsResp {
    #[serde(default)]
    items: Vec<GEvent>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}
#[derive(Deserialize, Default)]
struct GEvent {
    #[serde(default)]
    id: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    summary: Option<String>,
    #[serde(default)]
    location: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    start: Option<GTime>,
    #[serde(default)]
    end: Option<GTime>,
    #[serde(default)]
    recurrence: Option<Vec<String>>,
    #[serde(default, rename = "recurringEventId")]
    recurring_event_id: Option<String>,
    #[serde(default)]
    attendees: Option<Vec<GAttendee>>,
}
#[derive(Deserialize, Default)]
struct GTime {
    #[serde(default)]
    date: Option<String>,
    #[serde(default, rename = "dateTime")]
    date_time: Option<String>,
}
#[derive(Deserialize, Default)]
struct GAttendee {
    #[serde(default)]
    email: Option<String>,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
    #[serde(default, rename = "responseStatus")]
    response_status: Option<String>,
}

fn gtime_to_iso(g: &GTime) -> Option<(String, bool)> {
    if let Some(date) = &g.date {
        if !date.trim().is_empty() {
            return Some((date.trim().to_string(), true));
        }
    }
    if let Some(dt) = &g.date_time {
        let parsed = chrono::DateTime::parse_from_rfc3339(dt.trim()).ok()?;
        return Some((parsed.with_timezone(&Local).format("%Y-%m-%dT%H:%M").to_string(), false));
    }
    None
}

fn map_event(g: &GEvent, calendar_id: i64) -> Option<cal::EventCard> {
    let start = g.start.as_ref()?;
    let (start_iso, all_day) = gtime_to_iso(start)?;
    let end_iso = g.end.as_ref().and_then(gtime_to_iso).map(|(s, _)| s);
    let mut card = cal::EventCard::default();
    card.uid = format!("google-{}", g.id);
    card.calendar_id = Some(calendar_id);
    card.title = g.summary.clone().unwrap_or_default();
    card.location = g.location.clone().unwrap_or_default();
    card.description = g.description.clone().unwrap_or_default();
    card.all_day = all_day;
    card.start = start_iso.clone();
    if all_day {
        card.end = match end_iso {
            Some(e) if e != start_iso => minus_one_day(&e),
            _ => start_iso,
        };
    } else {
        card.end = end_iso.unwrap_or_else(|| card.start.clone());
    }
    card.status = match g.status.as_deref() {
        Some("tentative") => "tentative",
        _ => "confirmed",
    }
    .to_string();
    if let Some(lines) = &g.recurrence {
        for line in lines {
            if let Some(rest) = line.strip_prefix("RRULE:") {
                card.recurrence = cal::parse_rrule(rest);
                break;
            }
        }
    }
    if let Some(atts) = &g.attendees {
        card.attendees = atts
            .iter()
            .filter_map(|a| {
                a.email.clone().map(|email| cal::Attendee {
                    name: a.display_name.clone().unwrap_or_default(),
                    email,
                    status: a.response_status.clone().unwrap_or_default(),
                })
            })
            .collect();
    }
    Some(card)
}

fn event_to_google(card: &cal::EventCard) -> Value {
    let mut obj = json!({
        "summary": card.title,
        "location": card.location,
        "description": card.description,
        "status": if card.status == "tentative" { "tentative" } else { "confirmed" },
    });
    if card.all_day {
        let start = card.start.get(0..10).unwrap_or(&card.start).to_string();
        let end_inclusive = if card.end.trim().is_empty() { start.clone() } else { card.end.get(0..10).unwrap_or(&card.end).to_string() };
        obj["start"] = json!({ "date": start });
        obj["end"] = json!({ "date": plus_one_day_date(&end_inclusive) });
    } else {
        obj["start"] = json!({ "dateTime": local_rfc3339(&card.start) });
        let end = if card.end.trim().is_empty() { card.start.clone() } else { card.end.clone() };
        obj["end"] = json!({ "dateTime": local_rfc3339(&end) });
    }
    if let Some(rrule) = cal::build_rrule(card) {
        obj["recurrence"] = json!([format!("RRULE:{rrule}")]);
    }
    let attendees: Vec<Value> = card
        .attendees
        .iter()
        .filter(|a| !a.email.trim().is_empty())
        .map(|a| json!({ "email": a.email, "displayName": a.name }))
        .collect();
    if !attendees.is_empty() {
        obj["attendees"] = json!(attendees);
    }
    obj
}

struct LocalRow {
    id: i64,
    remote_id: Option<String>,
    uid: String,
    etag: Option<String>,
    state: LocalState,
    card_json: String,
    aux: i64, // events: calendar_id ; tasks: list_id ; contacts: unused
    scope_ms: i64, // events: start_ms ; others: 0
    recurs: bool,
}

fn parse_state(row: &sqlx::sqlite::SqliteRow) -> LocalState {
    let remote_id: Option<String> = row.try_get("remote_id").ok().flatten();
    LocalState {
        dirty: row.try_get::<i64, _>("dirty").unwrap_or(0) != 0,
        deleted: row.try_get::<i64, _>("deleted").unwrap_or(0) != 0,
        has_remote_id: remote_id.as_deref().map(|s| !s.is_empty()).unwrap_or(false),
        remote_updated_ms: row.try_get::<Option<i64>, _>("remote_updated_ms").ok().flatten().unwrap_or(0),
        local_updated_ms: row.try_get::<Option<i64>, _>("local_updated_ms").ok().flatten().unwrap_or(0),
    }
}

/// Resolve the Google calendar id a local event should be written to: its local
/// calendar's mirror, or the primary calendar for local-only calendars.
async fn target_gcal(pool: &SqlitePool, calendar_id: i64) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT remote_id FROM calendars WHERE calendar_id = ?")
        .bind(calendar_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "primary".to_string())
}

fn events_url(gcal: &str, event: Option<&str>) -> String {
    let mut url = Url::parse("https://www.googleapis.com/").unwrap();
    {
        let mut segs = url.path_segments_mut().unwrap();
        segs.extend(&["calendar", "v3", "calendars", gcal, "events"]);
        if let Some(ev) = event {
            segs.push(ev);
        }
    }
    url.to_string()
}

pub async fn sync_calendar(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let token = gmail_token(&state, account_id).await?;
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let http = reqwest::Client::new();

    // Fetch window (absolute for the API, naive for local in-scope checks).
    let now = chrono::Utc::now();
    let time_min = (now - Duration::days(365)).to_rfc3339();
    let time_max = (now + Duration::days(365)).to_rfc3339();
    let naive_now = Local::now().naive_local().and_utc().timestamp_millis();
    let win_from = naive_now - 365 * DAY_MS;
    let win_to = naive_now + 365 * DAY_MS;

    // 1. Mirror remote calendars → local, recording remote ids.
    let cal_list: CalListResp = google_get(&http, CAL_LIST_URL, &token).await?;
    let mut gcal_to_local: HashMap<String, i64> = HashMap::new();
    for item in &cal_list.items {
        if item.access_role.as_deref() == Some("freeBusyReader") {
            continue;
        }
        let summary = if item.summary.trim().is_empty() { item.id.clone() } else { item.summary.clone() };
        let color = item.background_color.clone().unwrap_or_else(|| "#246bce".into());
        let local_cal = cal::ensure_named_calendar(&pool, &format!("{summary} (Google)"), &color).await?;
        set_remote_id(&pool, "calendars", "calendar_id", local_cal, &item.id).await;
        gcal_to_local.insert(item.id.clone(), local_cal);
    }

    // 2. Fetch all remote events (per calendar), remember which gcal each came from.
    let mut remote: Vec<(String, GEvent)> = Vec::new();
    for (gcal, _local) in &gcal_to_local {
        let mut page: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            if pages > 20 {
                break;
            }
            let mut url = Url::parse(&events_url(gcal, None)).unwrap();
            {
                let mut qp = url.query_pairs_mut();
                qp.append_pair("timeMin", &time_min);
                qp.append_pair("timeMax", &time_max);
                qp.append_pair("singleEvents", "false");
                qp.append_pair("maxResults", "2500");
                qp.append_pair("showDeleted", "false");
                if let Some(t) = &page {
                    qp.append_pair("pageToken", t);
                }
            }
            let resp: EventsResp = google_get(&http, url.as_str(), &token).await?;
            for gev in resp.items {
                if gev.recurring_event_id.is_some() || gev.status.as_deref() == Some("cancelled") {
                    continue;
                }
                remote.push((gcal.clone(), gev));
            }
            page = resp.next_page_token;
            if page.is_none() {
                break;
            }
        }
    }

    // 3. Load local events.
    let rows = sqlx::query(
        "SELECT event_id, calendar_id, event_json, uid, remote_id, etag, dirty, deleted, remote_updated_ms, local_updated_ms, start_ms, recurs FROM events",
    )
    .fetch_all(&pool)
    .await?;
    let locals: Vec<LocalRow> = rows
        .iter()
        .map(|r| LocalRow {
            id: r.try_get("event_id").unwrap_or_default(),
            remote_id: r.try_get("remote_id").ok().flatten(),
            uid: r.try_get::<Option<String>, _>("uid").ok().flatten().unwrap_or_default(),
            etag: r.try_get("etag").ok().flatten(),
            state: parse_state(r),
            card_json: r.try_get::<Option<String>, _>("event_json").ok().flatten().unwrap_or_default(),
            aux: r.try_get::<Option<i64>, _>("calendar_id").ok().flatten().unwrap_or(0),
            scope_ms: r.try_get::<i64, _>("start_ms").unwrap_or(0),
            recurs: r.try_get::<i64, _>("recurs").unwrap_or(0) != 0,
        })
        .collect();

    let mut by_remote: HashMap<String, usize> = HashMap::new();
    let mut by_uid: HashMap<String, usize> = HashMap::new();
    for (i, l) in locals.iter().enumerate() {
        if let Some(rid) = l.remote_id.as_deref().filter(|s| !s.is_empty()) {
            by_remote.insert(rid.to_string(), i);
        }
        if !l.uid.is_empty() {
            by_uid.insert(l.uid.clone(), i);
        }
    }

    let mut seen: HashSet<usize> = HashSet::new();
    let mut pulled = 0i64;
    let mut pushed = 0i64;
    let mut deleted_remote = 0i64;

    // 4. Reconcile each remote event against local.
    for (gcal, gev) in &remote {
        let local_cal = *gcal_to_local.get(gcal).unwrap_or(&0);
        let Some(card) = map_event(gev, local_cal) else { continue };
        let remote_ms = gev.updated.as_deref().map(rfc3339_to_ms).unwrap_or(0);
        let idx = by_remote.get(&gev.id).or_else(|| by_uid.get(&format!("google-{}", gev.id))).copied();
        match idx {
            None => {
                cal::sync_write_event(&pool, None, &card, &gev.id, gev.etag.as_deref(), remote_ms).await?;
                pulled += 1;
            }
            Some(i) => {
                seen.insert(i);
                let local = &locals[i];
                match reconcile_matched(local.state, remote_ms) {
                    MatchAction::Noop => {}
                    MatchAction::PullOverwrite => {
                        cal::sync_write_event(&pool, Some(local.id), &card, &gev.id, gev.etag.as_deref(), remote_ms).await?;
                        pulled += 1;
                    }
                    MatchAction::UpdateRemote => {
                        if let Ok(lcard) = serde_json::from_str::<cal::EventCard>(&local.card_json) {
                            let body = event_to_google(&lcard);
                            let resp: Value = google_send(&http, Method::PUT, &events_url(gcal, Some(&gev.id)), &token, Some(&body)).await?;
                            let ms = rfc3339_to_ms(&v_str(&resp, "updated"));
                            cal::sync_mark_pushed(&pool, "events", "event_id", local.id, &gev.id, resp.get("etag").and_then(|x| x.as_str()), ms).await?;
                            pushed += 1;
                        }
                    }
                    MatchAction::DeleteRemote => {
                        google_delete(&http, &events_url(gcal, Some(&gev.id)), &token).await?;
                        cal::sync_hard_delete(&pool, "events", "event_id", local.id).await?;
                        deleted_remote += 1;
                    }
                }
            }
        }
    }

    // 5. Orphans: local events with no matching remote item this round.
    for (i, local) in locals.iter().enumerate() {
        if seen.contains(&i) {
            continue;
        }
        let in_scope = !local.recurs && local.scope_ms >= win_from && local.scope_ms < win_to;
        match reconcile_orphan(local.state, in_scope) {
            OrphanAction::Noop => {}
            OrphanAction::DropLocal | OrphanAction::DeleteLocal => {
                cal::sync_hard_delete(&pool, "events", "event_id", local.id).await?;
            }
            OrphanAction::CreateRemote | OrphanAction::RecreateRemote => {
                if let Ok(lcard) = serde_json::from_str::<cal::EventCard>(&local.card_json) {
                    let gcal = target_gcal(&pool, local.aux).await;
                    let body = event_to_google(&lcard);
                    let resp: Value = google_send(&http, Method::POST, &events_url(&gcal, None), &token, Some(&body)).await?;
                    let id = v_str(&resp, "id");
                    let ms = rfc3339_to_ms(&v_str(&resp, "updated"));
                    if !id.is_empty() {
                        cal::sync_mark_pushed(&pool, "events", "event_id", local.id, &id, resp.get("etag").and_then(|x| x.as_str()), ms).await?;
                        pushed += 1;
                    }
                }
            }
        }
    }

    Ok(Json(json!({ "pulled": pulled, "pushed": pushed, "deletedRemote": deleted_remote })))
}

// ─────────────────────────── Contacts (People) ───────────────────────────

#[derive(Deserialize, Default)]
struct ConnResp {
    #[serde(default)]
    connections: Vec<Person>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}
#[derive(Deserialize, Default)]
struct Person {
    #[serde(default, rename = "resourceName")]
    resource_name: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    names: Vec<PName>,
    #[serde(default, rename = "emailAddresses")]
    emails: Vec<PVal>,
    #[serde(default, rename = "phoneNumbers")]
    phones: Vec<PVal>,
    #[serde(default)]
    organizations: Vec<POrg>,
    #[serde(default)]
    addresses: Vec<PAddr>,
    #[serde(default)]
    biographies: Vec<PBio>,
    #[serde(default)]
    urls: Vec<PVal>,
    #[serde(default)]
    nicknames: Vec<PNick>,
}
#[derive(Deserialize, Default)]
struct PName {
    #[serde(default, rename = "givenName")]
    given_name: Option<String>,
    #[serde(default, rename = "familyName")]
    family_name: Option<String>,
    #[serde(default, rename = "middleName")]
    middle_name: Option<String>,
    #[serde(default, rename = "displayName")]
    display_name: Option<String>,
}
#[derive(Deserialize, Default)]
struct PVal {
    #[serde(default)]
    value: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}
#[derive(Deserialize, Default)]
struct POrg {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    department: Option<String>,
}
#[derive(Deserialize, Default)]
struct PAddr {
    #[serde(default, rename = "streetAddress")]
    street_address: Option<String>,
    #[serde(default)]
    city: Option<String>,
    #[serde(default)]
    region: Option<String>,
    #[serde(default, rename = "postalCode")]
    postal_code: Option<String>,
    #[serde(default)]
    country: Option<String>,
    #[serde(default, rename = "poBox")]
    po_box: Option<String>,
    #[serde(default, rename = "type")]
    kind: Option<String>,
}
#[derive(Deserialize, Default)]
struct PBio {
    #[serde(default)]
    value: Option<String>,
}
#[derive(Deserialize, Default)]
struct PNick {
    #[serde(default)]
    value: Option<String>,
}

fn map_person(p: &Person) -> con::ContactCard {
    let mut card = con::ContactCard::default();
    card.uid = p.resource_name.clone();
    if let Some(n) = p.names.first() {
        card.name.first = n.given_name.clone().unwrap_or_default();
        card.name.middle = n.middle_name.clone().unwrap_or_default();
        card.name.last = n.family_name.clone().unwrap_or_default();
        card.display_name = n.display_name.clone().unwrap_or_default();
    }
    if let Some(nick) = p.nicknames.first() {
        card.name.nickname = nick.value.clone().unwrap_or_default();
    }
    card.emails = p.emails.iter().filter_map(|e| e.value.clone().map(|value| con::LabeledValue { label: label_of(&e.kind), value })).collect();
    card.phones = p.phones.iter().filter_map(|e| e.value.clone().map(|value| con::LabeledValue { label: label_of(&e.kind), value })).collect();
    card.websites = p.urls.iter().filter_map(|e| e.value.clone().map(|value| con::LabeledValue { label: label_of(&e.kind), value })).collect();
    card.addresses = p.addresses.iter().map(|a| con::ContactAddress {
        label: label_of(&a.kind),
        po_box: a.po_box.clone().unwrap_or_default(),
        street: a.street_address.clone().unwrap_or_default(),
        city: a.city.clone().unwrap_or_default(),
        state: a.region.clone().unwrap_or_default(),
        postal_code: a.postal_code.clone().unwrap_or_default(),
        country: a.country.clone().unwrap_or_default(),
    }).collect();
    if let Some(o) = p.organizations.first() {
        card.organization.company = o.name.clone().unwrap_or_default();
        card.organization.job_title = o.title.clone().unwrap_or_default();
        card.organization.department = o.department.clone().unwrap_or_default();
    }
    if let Some(b) = p.biographies.first() {
        card.personal.notes = b.value.clone().unwrap_or_default();
    }
    card
}

const PERSON_FIELDS: &str = "names,emailAddresses,phoneNumbers,addresses,organizations,biographies,urls,nicknames";

fn person_to_google(card: &con::ContactCard, etag: Option<&str>) -> Value {
    let mut obj = json!({
        "names": [{ "givenName": card.name.first, "familyName": card.name.last, "middleName": card.name.middle }],
    });
    if let Some(e) = etag {
        obj["etag"] = json!(e);
    }
    if !card.name.nickname.trim().is_empty() {
        obj["nicknames"] = json!([{ "value": card.name.nickname }]);
    }
    let map = |list: &Vec<con::LabeledValue>| -> Vec<Value> {
        list.iter().filter(|x| !x.value.trim().is_empty()).map(|x| json!({ "value": x.value, "type": x.label })).collect()
    };
    let emails = map(&card.emails);
    if !emails.is_empty() { obj["emailAddresses"] = json!(emails); }
    let phones = map(&card.phones);
    if !phones.is_empty() { obj["phoneNumbers"] = json!(phones); }
    let urls = map(&card.websites);
    if !urls.is_empty() { obj["urls"] = json!(urls); }
    let addrs: Vec<Value> = card.addresses.iter().map(|a| json!({
        "streetAddress": a.street, "city": a.city, "region": a.state,
        "postalCode": a.postal_code, "country": a.country, "type": a.label,
    })).collect();
    if !addrs.is_empty() { obj["addresses"] = json!(addrs); }
    let org = &card.organization;
    if !(org.company.trim().is_empty() && org.job_title.trim().is_empty() && org.department.trim().is_empty()) {
        obj["organizations"] = json!([{ "name": org.company, "title": org.job_title, "department": org.department }]);
    }
    if !card.personal.notes.trim().is_empty() {
        obj["biographies"] = json!([{ "value": card.personal.notes, "contentType": "TEXT_PLAIN" }]);
    }
    obj
}

pub async fn sync_contacts(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let token = gmail_token(&state, account_id).await?;
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let http = reqwest::Client::new();

    // 1. Fetch all remote connections.
    let mut remote: Vec<Person> = Vec::new();
    let mut page: Option<String> = None;
    let mut pages = 0;
    loop {
        pages += 1;
        if pages > 25 {
            break;
        }
        let mut url = Url::parse(PEOPLE_URL).unwrap();
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair("personFields", PERSON_FIELDS);
            qp.append_pair("pageSize", "1000");
            if let Some(t) = &page {
                qp.append_pair("pageToken", t);
            }
        }
        let resp: ConnResp = google_get(&http, url.as_str(), &token).await?;
        remote.extend(resp.connections);
        page = resp.next_page_token;
        if page.is_none() {
            break;
        }
    }

    // 2. Load local contacts.
    let rows = sqlx::query(
        "SELECT contact_id, card_json, uid, remote_id, etag, dirty, deleted, remote_updated_ms, local_updated_ms FROM contacts",
    )
    .fetch_all(&pool)
    .await?;
    let locals: Vec<LocalRow> = rows
        .iter()
        .map(|r| LocalRow {
            id: r.try_get("contact_id").unwrap_or_default(),
            remote_id: r.try_get("remote_id").ok().flatten(),
            uid: r.try_get::<Option<String>, _>("uid").ok().flatten().unwrap_or_default(),
            etag: r.try_get("etag").ok().flatten(),
            state: parse_state(r),
            card_json: r.try_get::<Option<String>, _>("card_json").ok().flatten().unwrap_or_default(),
            aux: 0,
            scope_ms: 0,
            recurs: false,
        })
        .collect();
    let mut by_remote: HashMap<String, usize> = HashMap::new();
    let mut by_uid: HashMap<String, usize> = HashMap::new();
    for (i, l) in locals.iter().enumerate() {
        if let Some(rid) = l.remote_id.as_deref().filter(|s| !s.is_empty()) {
            by_remote.insert(rid.to_string(), i);
        }
        if !l.uid.is_empty() {
            by_uid.insert(l.uid.clone(), i);
        }
    }

    let mut seen: HashSet<usize> = HashSet::new();
    let (mut pulled, mut pushed, mut deleted_remote) = (0i64, 0i64, 0i64);

    // 3. Reconcile remote → local (contacts use etag-change as the remote signal).
    for p in &remote {
        let card = map_person(p);
        let idx = by_remote.get(&p.resource_name).or_else(|| by_uid.get(&p.resource_name)).copied();
        let remote_ms = rfc3339_to_ms(""); // People has no simple updated ts; etag drives change.
        match idx {
            None => {
                con::sync_write_contact(&pool, None, &card, &p.resource_name, p.etag.as_deref(), remote_ms).await?;
                pulled += 1;
            }
            Some(i) => {
                seen.insert(i);
                let local = &locals[i];
                if local.state.deleted {
                    let url = format!("https://people.googleapis.com/v1/{}:deleteContact", p.resource_name);
                    google_delete(&http, &url, &token).await?;
                    cal::sync_hard_delete(&pool, "contacts", "contact_id", local.id).await?;
                    deleted_remote += 1;
                } else if local.state.dirty {
                    // Local wins; update remote using the freshly-fetched etag.
                    if let Ok(lcard) = serde_json::from_str::<con::ContactCard>(&local.card_json) {
                        let body = person_to_google(&lcard, p.etag.as_deref());
                        let url = format!("https://people.googleapis.com/v1/{}:updateContact?updatePersonFields={}&personFields={}", p.resource_name, PERSON_FIELDS, PERSON_FIELDS);
                        let resp: Value = google_send(&http, Method::PATCH, &url, &token, Some(&body)).await?;
                        cal::sync_mark_pushed(&pool, "contacts", "contact_id", local.id, &p.resource_name, resp.get("etag").and_then(|x| x.as_str()), remote_ms).await?;
                        pushed += 1;
                    }
                } else if local.etag.as_deref() != p.etag.as_deref() {
                    // Remote changed and we have no local edits → pull.
                    con::sync_write_contact(&pool, Some(local.id), &card, &p.resource_name, p.etag.as_deref(), remote_ms).await?;
                    pulled += 1;
                }
            }
        }
    }

    // 4. Orphans.
    for (i, local) in locals.iter().enumerate() {
        if seen.contains(&i) {
            continue;
        }
        match reconcile_orphan(local.state, true) {
            OrphanAction::Noop => {}
            OrphanAction::DropLocal | OrphanAction::DeleteLocal => {
                cal::sync_hard_delete(&pool, "contacts", "contact_id", local.id).await?;
            }
            OrphanAction::CreateRemote | OrphanAction::RecreateRemote => {
                if let Ok(lcard) = serde_json::from_str::<con::ContactCard>(&local.card_json) {
                    let body = person_to_google(&lcard, None);
                    let url = format!("https://people.googleapis.com/v1/people:createContact?personFields={PERSON_FIELDS}");
                    let resp: Value = google_send(&http, Method::POST, &url, &token, Some(&body)).await?;
                    let rid = v_str(&resp, "resourceName");
                    if !rid.is_empty() {
                        cal::sync_mark_pushed(&pool, "contacts", "contact_id", local.id, &rid, resp.get("etag").and_then(|x| x.as_str()), 0).await?;
                        pushed += 1;
                    }
                }
            }
        }
    }

    Ok(Json(json!({ "pulled": pulled, "pushed": pushed, "deletedRemote": deleted_remote })))
}

// ─────────────────────────── Tasks ───────────────────────────

#[derive(Deserialize, Default)]
struct TaskListsResp {
    #[serde(default)]
    items: Vec<GTaskList>,
}
#[derive(Deserialize, Default)]
struct GTaskList {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
}
#[derive(Deserialize, Default)]
struct TasksResp {
    #[serde(default)]
    items: Vec<GTask>,
    #[serde(default, rename = "nextPageToken")]
    next_page_token: Option<String>,
}
#[derive(Deserialize, Default)]
struct GTask {
    #[serde(default)]
    id: String,
    #[serde(default)]
    etag: Option<String>,
    #[serde(default)]
    updated: Option<String>,
    #[serde(default)]
    title: Option<String>,
    #[serde(default)]
    notes: Option<String>,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    due: Option<String>,
}

fn map_task(g: &GTask, list_id: i64) -> Option<todo::TaskCard> {
    let title = g.title.clone().unwrap_or_default();
    if title.trim().is_empty() {
        return None;
    }
    let mut card = todo::TaskCard::default();
    card.uid = format!("gtasks-{}", g.id);
    card.list_id = Some(list_id);
    card.title = title;
    card.notes = g.notes.clone().unwrap_or_default();
    card.completed = g.status.as_deref() == Some("completed");
    if let Some(due) = &g.due {
        if due.len() >= 10 {
            card.due = due[..10].to_string();
        }
    }
    Some(card)
}

fn task_to_google(card: &todo::TaskCard) -> Value {
    let mut obj = json!({
        "title": card.title,
        "notes": card.notes,
        "status": if card.completed { "completed" } else { "needsAction" },
    });
    let date = card.due.get(0..10).unwrap_or("");
    if !date.is_empty() {
        obj["due"] = json!(format!("{date}T00:00:00.000Z"));
    }
    obj
}

fn tasks_url(tasklist: &str, task: Option<&str>) -> String {
    let mut url = Url::parse("https://tasks.googleapis.com/").unwrap();
    {
        let mut segs = url.path_segments_mut().unwrap();
        segs.extend(&["tasks", "v1", "lists", tasklist, "tasks"]);
        if let Some(t) = task {
            segs.push(t);
        }
    }
    url.to_string()
}

async fn target_tasklist(pool: &SqlitePool, list_id: i64) -> String {
    sqlx::query_scalar::<_, Option<String>>("SELECT remote_id FROM task_lists WHERE list_id = ?")
        .bind(list_id)
        .fetch_optional(pool)
        .await
        .ok()
        .flatten()
        .flatten()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "@default".to_string())
}

pub async fn sync_tasks(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let token = gmail_token(&state, account_id).await?;
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let http = reqwest::Client::new();

    // 1. Mirror remote task lists → local, recording remote ids.
    let lists: TaskListsResp = google_get(&http, TASKLISTS_URL, &token).await?;
    let mut gl_to_local: HashMap<String, i64> = HashMap::new();
    for gl in &lists.items {
        let title = if gl.title.trim().is_empty() { "Tasks".to_string() } else { gl.title.clone() };
        let local_list = todo::ensure_named_list(&pool, &format!("{title} (Google)")).await?;
        set_remote_id(&pool, "task_lists", "list_id", local_list, &gl.id).await;
        gl_to_local.insert(gl.id.clone(), local_list);
    }

    // 2. Fetch all remote tasks.
    let mut remote: Vec<(String, GTask)> = Vec::new();
    for (gl, _local) in &gl_to_local {
        let mut page: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            if pages > 20 {
                break;
            }
            let mut url = Url::parse(&tasks_url(gl, None)).unwrap();
            {
                let mut qp = url.query_pairs_mut();
                qp.append_pair("showCompleted", "true");
                qp.append_pair("showHidden", "true");
                qp.append_pair("maxResults", "100");
                if let Some(t) = &page {
                    qp.append_pair("pageToken", t);
                }
            }
            let resp: TasksResp = google_get(&http, url.as_str(), &token).await?;
            for gt in resp.items {
                remote.push((gl.clone(), gt));
            }
            page = resp.next_page_token;
            if page.is_none() {
                break;
            }
        }
    }

    // 3. Load local tasks.
    let rows = sqlx::query(
        "SELECT task_id, list_id, task_json, uid, remote_id, etag, dirty, deleted, remote_updated_ms, local_updated_ms FROM tasks",
    )
    .fetch_all(&pool)
    .await?;
    let locals: Vec<LocalRow> = rows
        .iter()
        .map(|r| LocalRow {
            id: r.try_get("task_id").unwrap_or_default(),
            remote_id: r.try_get("remote_id").ok().flatten(),
            uid: r.try_get::<Option<String>, _>("uid").ok().flatten().unwrap_or_default(),
            etag: r.try_get("etag").ok().flatten(),
            state: parse_state(r),
            card_json: r.try_get::<Option<String>, _>("task_json").ok().flatten().unwrap_or_default(),
            aux: r.try_get::<Option<i64>, _>("list_id").ok().flatten().unwrap_or(0),
            scope_ms: 0,
            recurs: false,
        })
        .collect();
    let mut by_remote: HashMap<String, usize> = HashMap::new();
    let mut by_uid: HashMap<String, usize> = HashMap::new();
    for (i, l) in locals.iter().enumerate() {
        if let Some(rid) = l.remote_id.as_deref().filter(|s| !s.is_empty()) {
            by_remote.insert(rid.to_string(), i);
        }
        if !l.uid.is_empty() {
            by_uid.insert(l.uid.clone(), i);
        }
    }

    let mut seen: HashSet<usize> = HashSet::new();
    let (mut pulled, mut pushed, mut deleted_remote) = (0i64, 0i64, 0i64);

    // 4. Reconcile remote → local.
    for (gl, gt) in &remote {
        let local_list = *gl_to_local.get(gl).unwrap_or(&0);
        let Some(card) = map_task(gt, local_list) else { continue };
        let remote_ms = gt.updated.as_deref().map(rfc3339_to_ms).unwrap_or(0);
        let idx = by_remote.get(&gt.id).or_else(|| by_uid.get(&format!("gtasks-{}", gt.id))).copied();
        match idx {
            None => {
                todo::sync_write_task(&pool, None, &card, &gt.id, gt.etag.as_deref(), remote_ms).await?;
                pulled += 1;
            }
            Some(i) => {
                seen.insert(i);
                let local = &locals[i];
                match reconcile_matched(local.state, remote_ms) {
                    MatchAction::Noop => {}
                    MatchAction::PullOverwrite => {
                        todo::sync_write_task(&pool, Some(local.id), &card, &gt.id, gt.etag.as_deref(), remote_ms).await?;
                        pulled += 1;
                    }
                    MatchAction::UpdateRemote => {
                        if let Ok(lcard) = serde_json::from_str::<todo::TaskCard>(&local.card_json) {
                            let body = task_to_google(&lcard);
                            let resp: Value = google_send(&http, Method::PATCH, &tasks_url(gl, Some(&gt.id)), &token, Some(&body)).await?;
                            let ms = rfc3339_to_ms(&v_str(&resp, "updated"));
                            todo::sync_write_task(&pool, Some(local.id), &lcard, &gt.id, resp.get("etag").and_then(|x| x.as_str()), ms).await?;
                            pushed += 1;
                        }
                    }
                    MatchAction::DeleteRemote => {
                        google_delete(&http, &tasks_url(gl, Some(&gt.id)), &token).await?;
                        cal::sync_hard_delete(&pool, "tasks", "task_id", local.id).await?;
                        deleted_remote += 1;
                    }
                }
            }
        }
    }

    // 5. Orphans.
    for (i, local) in locals.iter().enumerate() {
        if seen.contains(&i) {
            continue;
        }
        match reconcile_orphan(local.state, true) {
            OrphanAction::Noop => {}
            OrphanAction::DropLocal | OrphanAction::DeleteLocal => {
                cal::sync_hard_delete(&pool, "tasks", "task_id", local.id).await?;
            }
            OrphanAction::CreateRemote | OrphanAction::RecreateRemote => {
                if let Ok(lcard) = serde_json::from_str::<todo::TaskCard>(&local.card_json) {
                    let tasklist = target_tasklist(&pool, local.aux).await;
                    let body = task_to_google(&lcard);
                    let resp: Value = google_send(&http, Method::POST, &tasks_url(&tasklist, None), &token, Some(&body)).await?;
                    let id = v_str(&resp, "id");
                    let ms = rfc3339_to_ms(&v_str(&resp, "updated"));
                    if !id.is_empty() {
                        cal::sync_mark_pushed(&pool, "tasks", "task_id", local.id, &id, resp.get("etag").and_then(|x| x.as_str()), ms).await?;
                        pushed += 1;
                    }
                }
            }
        }
    }

    Ok(Json(json!({ "pulled": pulled, "pushed": pushed, "deletedRemote": deleted_remote })))
}
