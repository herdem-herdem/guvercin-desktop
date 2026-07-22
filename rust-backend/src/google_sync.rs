//! One-way sync from Google into the local stores, for accounts signed in with
//! Google (`provider_type = 'gmail'`).
//!
//! It reuses the existing OAuth plumbing: [`crate::oauth::access_token_for_account`]
//! hands back a fresh access token from the account's stored refresh token, and
//! we call the Google Calendar v3 and People v1 REST APIs with it. Events and
//! contacts are upserted keyed on a stable remote id (stored in each record's
//! `uid`), so re-running a sync refreshes rather than duplicates.
//!
//! Push (local → Google) is intentionally out of scope here: pulling is safe and
//! non-destructive, whereas two-way sync needs sync tokens, tombstones and
//! conflict resolution. Accounts whose stored token predates the Calendar/Contacts
//! scopes simply get a clear "reconnect your Google account" message.

use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Local;
use reqwest::{StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize};
use serde_json::{json, Value};

use crate::{
    calendar_routes as cal, contacts_routes as con, db, db::AppState, error::AppError, oauth,
};

const CAL_LIST_URL: &str = "https://www.googleapis.com/calendar/v3/users/me/calendarList";
const PEOPLE_URL: &str = "https://people.googleapis.com/v1/people/me/connections";
const RECONNECT_MSG: &str =
    "Reconnect your Google account to allow Calendar/Contacts access — sign out and sign in with Google again.";

// ─────────────────────────── shared helpers ───────────────────────────

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
    let resp = http
        .get(url)
        .bearer_auth(token)
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

fn label_of(t: &Option<String>) -> String {
    match t.as_deref() {
        Some(s) if !s.trim().is_empty() => s.trim().to_lowercase(),
        _ => "other".to_string(),
    }
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

// ─────────────────────────── Calendar sync ───────────────────────────

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
        let local = parsed.with_timezone(&Local);
        return Some((local.format("%Y-%m-%dT%H:%M").to_string(), false));
    }
    None
}

fn minus_one_day(date_str: &str) -> String {
    chrono::NaiveDate::parse_from_str(date_str, "%Y-%m-%d")
        .map(|d| (d - chrono::Duration::days(1)).format("%Y-%m-%d").to_string())
        .unwrap_or_else(|_| date_str.to_string())
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
        // Google DTEND for all-day events is exclusive; store the inclusive last day.
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

pub async fn sync_calendar(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let token = gmail_token(&state, account_id).await?;
    let user_pool = db::get_user_db_pool(&state, account_id).await?;
    let http = reqwest::Client::new();

    let cal_list: CalListResp = google_get(&http, CAL_LIST_URL, &token).await?;

    let now = chrono::Utc::now();
    let time_min = (now - chrono::Duration::days(365)).to_rfc3339();
    let time_max = (now + chrono::Duration::days(365)).to_rfc3339();

    let mut total_events: i64 = 0;
    let mut calendar_count: i64 = 0;

    for item in &cal_list.items {
        // Skip calendars we can only see free/busy for.
        if item.access_role.as_deref() == Some("freeBusyReader") {
            continue;
        }
        let summary = if item.summary.trim().is_empty() {
            item.id.clone()
        } else {
            item.summary.clone()
        };
        let name = format!("{summary} (Google)");
        let color = item.background_color.clone().unwrap_or_else(|| "#246bce".into());
        let local_cal_id = cal::ensure_named_calendar(&user_pool, &name, &color).await?;

        let mut page_token: Option<String> = None;
        let mut pages = 0;
        loop {
            pages += 1;
            if pages > 20 {
                break;
            }
            let mut url = Url::parse("https://www.googleapis.com/").unwrap();
            url.path_segments_mut()
                .unwrap()
                .extend(&["calendar", "v3", "calendars", &item.id, "events"]);
            {
                let mut qp = url.query_pairs_mut();
                qp.append_pair("timeMin", &time_min);
                qp.append_pair("timeMax", &time_max);
                qp.append_pair("singleEvents", "false");
                qp.append_pair("maxResults", "2500");
                qp.append_pair("showDeleted", "false");
                if let Some(tok) = &page_token {
                    qp.append_pair("pageToken", tok);
                }
            }
            let resp: EventsResp = google_get(&http, url.as_str(), &token).await?;
            for gev in &resp.items {
                if gev.recurring_event_id.is_some() {
                    continue; // modified single instance of a series — skip to avoid dupes
                }
                if gev.status.as_deref() == Some("cancelled") {
                    continue;
                }
                if let Some(card) = map_event(gev, local_cal_id) {
                    cal::upsert_event_by_uid(&user_pool, &card).await?;
                    total_events += 1;
                }
            }
            page_token = resp.next_page_token.clone();
            if page_token.is_none() {
                break;
            }
        }
        calendar_count += 1;
    }

    Ok(Json(json!({
        "events": total_events,
        "calendars": calendar_count,
    })))
}

// ─────────────────────────── Contacts sync ───────────────────────────

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
    card.emails = p
        .emails
        .iter()
        .filter_map(|e| e.value.clone().map(|value| con::LabeledValue { label: label_of(&e.kind), value }))
        .collect();
    card.phones = p
        .phones
        .iter()
        .filter_map(|e| e.value.clone().map(|value| con::LabeledValue { label: label_of(&e.kind), value }))
        .collect();
    card.websites = p
        .urls
        .iter()
        .filter_map(|e| e.value.clone().map(|value| con::LabeledValue { label: label_of(&e.kind), value }))
        .collect();
    card.addresses = p
        .addresses
        .iter()
        .map(|a| con::ContactAddress {
            label: label_of(&a.kind),
            po_box: a.po_box.clone().unwrap_or_default(),
            street: a.street_address.clone().unwrap_or_default(),
            city: a.city.clone().unwrap_or_default(),
            state: a.region.clone().unwrap_or_default(),
            postal_code: a.postal_code.clone().unwrap_or_default(),
            country: a.country.clone().unwrap_or_default(),
        })
        .collect();
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

pub async fn sync_contacts(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let token = gmail_token(&state, account_id).await?;
    let user_pool = db::get_user_db_pool(&state, account_id).await?;
    let http = reqwest::Client::new();

    let mut page_token: Option<String> = None;
    let mut count: i64 = 0;
    let mut pages = 0;
    loop {
        pages += 1;
        if pages > 25 {
            break;
        }
        let mut url = Url::parse(PEOPLE_URL).unwrap();
        {
            let mut qp = url.query_pairs_mut();
            qp.append_pair(
                "personFields",
                "names,emailAddresses,phoneNumbers,addresses,organizations,biographies,urls,nicknames",
            );
            qp.append_pair("pageSize", "1000");
            if let Some(tok) = &page_token {
                qp.append_pair("pageToken", tok);
            }
        }
        let resp: ConnResp = google_get(&http, url.as_str(), &token).await?;
        for person in &resp.connections {
            let card = map_person(person);
            // Skip entries with no usable identity at all.
            if card.name.first.is_empty()
                && card.name.last.is_empty()
                && card.display_name.is_empty()
                && card.emails.is_empty()
                && card.phones.is_empty()
            {
                continue;
            }
            con::upsert_contact_by_uid(&user_pool, &card).await?;
            count += 1;
        }
        page_token = resp.next_page_token.clone();
        if page_token.is_none() {
            break;
        }
    }

    Ok(Json(json!({ "contacts": count })))
}
