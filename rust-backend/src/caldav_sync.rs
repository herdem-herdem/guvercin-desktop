//! Two-way sync between the local calendar store and a CalDAV server (RFC 4791).
//!
//! This mirrors [`crate::google_sync`] but speaks CalDAV instead of the Calendar v3
//! REST API. Connection details (a discovery/base URL, username and password — the
//! last usually an app-specific password) are stored on the account row and entered
//! through a dedicated form; there is no OAuth. Discovery follows the standard chain:
//! `current-user-principal` → `calendar-home-set` → the calendar collections under it.
//!
//! CalDAV has no simple per-item `updated` timestamp, so change detection is
//! ETag-based: the reconcile mirrors the contacts (People) path in `google_sync`
//! rather than the timestamp-driven [`crate::sync_reconcile::reconcile_matched`].
//! Events are exchanged as iCalendar, reusing the calendar module's ICS
//! builder/parser so CalDAV round-trips exactly like Import/Export. Deletes stay
//! conservative and are scoped to CalDAV-mirrored calendars so a second backend
//! (e.g. Google) configured on the same account is never touched.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use axum::{
    extract::{Path, State},
    Json,
};
use chrono::Duration;
use reqwest::{header, Method, StatusCode, Url};
use serde::Deserialize;
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

use crate::{
    calendar_routes as cal, db, db::AppState, error::AppError,
    sync_reconcile::{reconcile_orphan, LocalState, OrphanAction},
};

const DAY_MS: i64 = 86_400_000;
const RECONNECT_MSG: &str =
    "CalDAV rejected the credentials — check the server URL, username and (app-specific) password.";

// ─────────────────────────── config storage ───────────────────────────

struct CalDavConfig {
    url: String,
    username: String,
    password: String,
}

/// Load a complete CalDAV config for the account, or `None` if not fully set up.
async fn load_config(
    state: &Arc<AppState>,
    account_id: i64,
) -> Result<Option<CalDavConfig>, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let row = sqlx::query(
        "SELECT caldav_url, caldav_username, caldav_password FROM accounts WHERE account_id = ?",
    )
    .bind(account_id)
    .fetch_optional(&general)
    .await?;
    let Some(row) = row else { return Ok(None) };
    let url: String = row.try_get::<Option<String>, _>("caldav_url").ok().flatten().unwrap_or_default();
    let username: String = row.try_get::<Option<String>, _>("caldav_username").ok().flatten().unwrap_or_default();
    let password: String = row.try_get::<Option<String>, _>("caldav_password").ok().flatten().unwrap_or_default();
    if url.trim().is_empty() || username.trim().is_empty() || password.is_empty() {
        return Ok(None);
    }
    Ok(Some(CalDavConfig { url: url.trim().to_string(), username, password }))
}

pub async fn caldav_status(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let row = sqlx::query(
        "SELECT caldav_url, caldav_username FROM accounts WHERE account_id = ?",
    )
    .bind(account_id)
    .fetch_optional(&general)
    .await?;
    let cfg = load_config(&state, account_id).await?;
    let (url, username) = row
        .map(|r| {
            (
                r.try_get::<Option<String>, _>("caldav_url").ok().flatten().unwrap_or_default(),
                r.try_get::<Option<String>, _>("caldav_username").ok().flatten().unwrap_or_default(),
            )
        })
        .unwrap_or_default();
    Ok(Json(json!({
        "configured": cfg.is_some(),
        "available": cfg.is_some(),
        "url": url,
        "username": username,
    })))
}

pub async fn caldav_get_config(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let row = sqlx::query(
        "SELECT caldav_url, caldav_username, caldav_password FROM accounts WHERE account_id = ?",
    )
    .bind(account_id)
    .fetch_optional(&general)
    .await?;
    let (url, username, has_password) = row
        .map(|r| {
            let pw: Option<String> = r.try_get("caldav_password").ok().flatten();
            (
                r.try_get::<Option<String>, _>("caldav_url").ok().flatten().unwrap_or_default(),
                r.try_get::<Option<String>, _>("caldav_username").ok().flatten().unwrap_or_default(),
                pw.map(|p| !p.is_empty()).unwrap_or(false),
            )
        })
        .unwrap_or_default();
    Ok(Json(json!({ "url": url, "username": username, "hasPassword": has_password })))
}

#[derive(Deserialize, Default)]
pub struct ConfigBody {
    #[serde(default)]
    url: Option<String>,
    #[serde(default)]
    username: Option<String>,
    #[serde(default)]
    password: Option<String>,
}

/// Save (or clear) the CalDAV config. A non-empty URL is validated by running
/// discovery with the resolved credentials before it is persisted, so the form
/// gets immediate feedback; an empty URL disconnects and clears all three fields.
pub async fn caldav_put_config(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<ConfigBody>,
) -> Result<Json<Value>, AppError> {
    let general = state.ensure_ready(false).await?.general_pool.clone();
    let url = body.url.unwrap_or_default().trim().to_string();
    let username = body.username.unwrap_or_default().trim().to_string();

    if url.is_empty() {
        sqlx::query(
            "UPDATE accounts SET caldav_url = NULL, caldav_username = NULL, caldav_password = NULL WHERE account_id = ?",
        )
        .bind(account_id)
        .execute(&general)
        .await?;
        return Ok(Json(json!({ "configured": false })));
    }

    // Resolve the password: use the supplied one, else keep what is stored.
    let password = match body.password {
        Some(p) if !p.is_empty() => p,
        _ => sqlx::query_scalar::<_, Option<String>>(
            "SELECT caldav_password FROM accounts WHERE account_id = ?",
        )
        .bind(account_id)
        .fetch_optional(&general)
        .await?
        .flatten()
        .unwrap_or_default(),
    };
    if username.is_empty() || password.is_empty() {
        return Err(AppError::BadRequest(
            "CalDAV needs a server URL, a username and a password.".into(),
        ));
    }

    let cfg = CalDavConfig { url: url.clone(), username: username.clone(), password: password.clone() };
    let http = reqwest::Client::new();
    let collections = discover(&http, &cfg).await?;
    if collections.is_empty() {
        return Err(AppError::BadRequest(
            "Connected, but no calendars were found at that URL.".into(),
        ));
    }

    sqlx::query(
        "UPDATE accounts SET caldav_url = ?, caldav_username = ?, caldav_password = ? WHERE account_id = ?",
    )
    .bind(&url)
    .bind(&username)
    .bind(&password)
    .bind(account_id)
    .execute(&general)
    .await?;

    Ok(Json(json!({ "configured": true, "calendars": collections.len() as i64 })))
}

// ─────────────────────────── HTTP / DAV plumbing ───────────────────────────

fn propfind() -> Method {
    Method::from_bytes(b"PROPFIND").expect("valid method")
}
fn report() -> Method {
    Method::from_bytes(b"REPORT").expect("valid method")
}

struct DavResp {
    status: StatusCode,
    etag: Option<String>,
    body: String,
}

/// Perform an authenticated DAV request and read the body. `depth` sets the Depth
/// header (PROPFIND/REPORT); `body` is an XML/iCalendar request payload.
async fn dav_request(
    http: &reqwest::Client,
    cfg: &CalDavConfig,
    method: Method,
    url: &str,
    depth: Option<&str>,
    content_type: Option<&str>,
    extra: &[(&str, &str)],
    body: Option<String>,
) -> Result<DavResp, AppError> {
    let mut req = http
        .request(method, url)
        .basic_auth(&cfg.username, Some(&cfg.password));
    if let Some(d) = depth {
        req = req.header("Depth", d);
    }
    if let Some(ct) = content_type {
        req = req.header(header::CONTENT_TYPE, ct);
    }
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    if let Some(b) = body {
        req = req.body(b);
    }
    let resp = req
        .send()
        .await
        .map_err(|e| AppError::BadRequest(format!("CalDAV request failed: {e}")))?;
    let status = resp.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(AppError::BadRequest(RECONNECT_MSG.into()));
    }
    let etag = resp
        .headers()
        .get(header::ETAG)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());
    let text = resp.text().await.unwrap_or_default();
    Ok(DavResp { status, etag, body: text })
}

fn is_dav_success(status: StatusCode) -> bool {
    status.is_success() || status == StatusCode::MULTI_STATUS
}

// ─────────────────────────── tiny XML reader ───────────────────────────
//
// CalDAV bodies are namespaced XML (`D:`, `d:`, `cal:`, default, …). We only ever
// pull a handful of leaf values out of well-formed server responses, so a small
// namespace-agnostic scanner is enough and avoids a new dependency. Every element
// we extract is a leaf or a container that never nests another element of the same
// local name, which is what makes "first matching close tag" correct here.

fn local_name(tag: &str) -> &str {
    let tag = tag.trim();
    let end = tag.find(|c: char| c.is_whitespace() || c == '/').unwrap_or(tag.len());
    let name = &tag[..end];
    match name.rfind(':') {
        Some(i) => &name[i + 1..],
        None => name,
    }
}

/// Inner content of the first element with local name `local` at/after `from`, plus
/// the byte index just past its close tag. Self-closing elements yield `""`.
fn elem_inner_from<'a>(xml: &'a str, local: &str, from: usize) -> Option<(&'a str, usize)> {
    let mut i = from;
    while let Some(rel) = xml[i..].find('<') {
        let lt = i + rel;
        let rel_gt = xml[lt..].find('>')?;
        let gt = lt + rel_gt;
        let tag = &xml[lt + 1..gt];
        if tag.starts_with('/') || tag.starts_with('?') || tag.starts_with('!') {
            i = gt + 1;
            continue;
        }
        if local_name(tag).eq_ignore_ascii_case(local) {
            if tag.ends_with('/') {
                return Some(("", gt + 1));
            }
            let inner_start = gt + 1;
            let mut j = inner_start;
            while let Some(crel) = xml[j..].find("</") {
                let clt = j + crel;
                let crel_gt = xml[clt..].find('>')?;
                let cgt = clt + crel_gt;
                if local_name(&xml[clt + 2..cgt]).eq_ignore_ascii_case(local) {
                    return Some((&xml[inner_start..clt], cgt + 1));
                }
                j = cgt + 1;
            }
            return None;
        }
        i = gt + 1;
    }
    None
}

fn elem_inner<'a>(xml: &'a str, local: &str) -> Option<&'a str> {
    elem_inner_from(xml, local, 0).map(|(s, _)| s)
}

fn elem_text(xml: &str, local: &str) -> Option<String> {
    elem_inner(xml, local).map(|s| xml_unescape(s.trim()))
}

/// Whether any element with local name `local` appears in `xml` (open or self-closing).
fn has_elem(xml: &str, local: &str) -> bool {
    let mut i = 0;
    while let Some(rel) = xml[i..].find('<') {
        let lt = i + rel;
        let Some(rel_gt) = xml[lt..].find('>') else { break };
        let gt = lt + rel_gt;
        let tag = &xml[lt + 1..gt];
        if !tag.starts_with('/')
            && !tag.starts_with('?')
            && !tag.starts_with('!')
            && local_name(tag).eq_ignore_ascii_case(local)
        {
            return true;
        }
        i = gt + 1;
    }
    false
}

/// Iterate the `<response>` blocks of a multistatus body (inner content of each).
fn responses(xml: &str) -> Vec<&str> {
    let mut out = Vec::new();
    let mut i = 0;
    while let Some((inner, end)) = elem_inner_from(xml, "response", i) {
        out.push(inner);
        i = end;
    }
    out
}

fn xml_unescape(s: &str) -> String {
    if !s.contains('&') {
        return s.to_string();
    }
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(amp) = rest.find('&') {
        out.push_str(&rest[..amp]);
        let after = &rest[amp..];
        if let Some(semi) = after.find(';') {
            let ent = &after[1..semi];
            let repl = match ent {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                _ if ent.starts_with("#x") || ent.starts_with("#X") => {
                    u32::from_str_radix(&ent[2..], 16).ok().and_then(char::from_u32)
                }
                _ if ent.starts_with('#') => {
                    ent[1..].parse::<u32>().ok().and_then(char::from_u32)
                }
                _ => None,
            };
            if let Some(c) = repl {
                out.push(c);
                rest = &after[semi + 1..];
                continue;
            }
        }
        out.push('&');
        rest = &after[1..];
    }
    out.push_str(rest);
    out
}

// ─────────────────────────── discovery ───────────────────────────

struct CalCollection {
    url: Url,
    display_name: String,
    color: String,
}

const PRINCIPAL_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:"><d:prop><d:current-user-principal/></d:prop></d:propfind>"#;
const HOME_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><c:calendar-home-set/></d:prop></d:propfind>"#;
const CAL_LIST_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav" xmlns:cs="http://calendarserver.org/ns/" xmlns:ic="http://apple.com/ns/ical/"><d:prop><d:resourcetype/><d:displayname/><cs:getctag/><c:supported-calendar-component-set/><ic:calendar-color/></d:prop></d:propfind>"#;
const SELF_BODY: &str = r#"<?xml version="1.0" encoding="utf-8"?><d:propfind xmlns:d="DAV:" xmlns:ic="http://apple.com/ns/ical/"><d:prop><d:resourcetype/><d:displayname/><ic:calendar-color/></d:prop></d:propfind>"#;

const XML_CT: &str = "application/xml; charset=utf-8";

/// Follow one `<href>` inside a single-property PROPFIND response.
async fn propfind_href(
    http: &reqwest::Client,
    cfg: &CalDavConfig,
    at: &Url,
    body: &str,
    container: &str,
) -> Option<String> {
    let resp = dav_request(http, cfg, propfind(), at.as_str(), Some("0"), Some(XML_CT), &[], Some(body.to_string()))
        .await
        .ok()?;
    if !is_dav_success(resp.status) {
        return None;
    }
    let inner = elem_inner(&resp.body, container)?;
    elem_text(inner, "href").filter(|s| !s.trim().is_empty())
}

fn normalize_color(raw: Option<String>) -> String {
    let fallback = "#246bce".to_string();
    let Some(c) = raw else { return fallback };
    let c = c.trim();
    // Apple stores "#RRGGBBAA"; trim the alpha and validate.
    let hex = c.strip_prefix('#').unwrap_or(c);
    let hex: String = hex.chars().take(6).collect();
    if hex.len() == 6 && hex.chars().all(|ch| ch.is_ascii_hexdigit()) {
        format!("#{hex}")
    } else {
        fallback
    }
}

fn name_from_href(url: &Url) -> String {
    url.path_segments()
        .and_then(|segs| segs.filter(|s| !s.is_empty()).last())
        .map(|s| s.to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Calendar".to_string())
}

/// List the calendar collections under `home` (Depth 1), keeping only those that
/// are calendars and support VEVENT.
async fn list_calendars(
    http: &reqwest::Client,
    cfg: &CalDavConfig,
    home: &Url,
) -> Result<Vec<CalCollection>, AppError> {
    let resp = dav_request(http, cfg, propfind(), home.as_str(), Some("1"), Some(XML_CT), &[], Some(CAL_LIST_BODY.to_string())).await?;
    if !is_dav_success(resp.status) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for r in responses(&resp.body) {
        let Some(href) = elem_text(r, "href").filter(|s| !s.trim().is_empty()) else { continue };
        let rtype = elem_inner(r, "resourcetype").unwrap_or("");
        if !has_elem(rtype, "calendar") {
            continue; // the home collection itself, address books, etc.
        }
        // If the supported-component set is present and excludes VEVENT, skip
        // (VTODO/VJOURNAL-only collections). Absent ⇒ assume events are allowed.
        let supports_vevent = elem_inner(r, "supported-calendar-component-set")
            .map(|s| s.to_uppercase().contains("VEVENT"))
            .unwrap_or(true);
        if !supports_vevent {
            continue;
        }
        let Ok(abs) = home.join(&href) else { continue };
        let display_name = elem_text(r, "displayname")
            .filter(|s| !s.trim().is_empty())
            .unwrap_or_else(|| name_from_href(&abs));
        out.push(CalCollection {
            display_name,
            color: normalize_color(elem_text(r, "calendar-color")),
            url: abs,
        });
    }
    Ok(out)
}

/// Treat `at` itself as a calendar collection if it is one (handles a URL that
/// points straight at a calendar rather than a home set).
async fn self_calendar(http: &reqwest::Client, cfg: &CalDavConfig, at: &Url) -> Option<CalCollection> {
    let resp = dav_request(http, cfg, propfind(), at.as_str(), Some("0"), Some(XML_CT), &[], Some(SELF_BODY.to_string()))
        .await
        .ok()?;
    if !is_dav_success(resp.status) {
        return None;
    }
    let r = responses(&resp.body).into_iter().next().unwrap_or(&resp.body);
    let rtype = elem_inner(r, "resourcetype").unwrap_or("");
    if !has_elem(rtype, "calendar") {
        return None;
    }
    let display_name = elem_text(r, "displayname")
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| name_from_href(at));
    Some(CalCollection {
        display_name,
        color: normalize_color(elem_text(r, "calendar-color")),
        url: at.clone(),
    })
}

/// Full discovery chain from the configured URL to the set of calendar collections.
async fn discover(http: &reqwest::Client, cfg: &CalDavConfig) -> Result<Vec<CalCollection>, AppError> {
    let base = Url::parse(&cfg.url)
        .map_err(|_| AppError::BadRequest("That CalDAV URL is not valid.".into()))?;

    let principal = propfind_href(http, cfg, &base, PRINCIPAL_BODY, "current-user-principal").await;
    let principal_url = principal
        .and_then(|h| base.join(&h).ok())
        .unwrap_or_else(|| base.clone());

    let home = propfind_href(http, cfg, &principal_url, HOME_BODY, "calendar-home-set").await;
    let home_url = home
        .and_then(|h| principal_url.join(&h).ok())
        .unwrap_or_else(|| principal_url.clone());

    let mut cols = list_calendars(http, cfg, &home_url).await?;
    if cols.is_empty() {
        // The URL may point directly at a single calendar collection.
        if let Some(c) = self_calendar(http, cfg, &home_url).await {
            cols.push(c);
        } else if let Some(c) = self_calendar(http, cfg, &base).await {
            cols.push(c);
        }
    }
    Ok(cols)
}

// ─────────────────────────── events ───────────────────────────

struct RemoteEvent {
    href: String,
    etag: Option<String>,
    ics: String,
}

/// Fetch events in a calendar collection within [start, end] via calendar-query.
async fn fetch_events(
    http: &reqwest::Client,
    cfg: &CalDavConfig,
    collection: &Url,
    start: &str,
    end: &str,
) -> Result<Vec<RemoteEvent>, AppError> {
    let body = format!(
        r#"<?xml version="1.0" encoding="utf-8"?><c:calendar-query xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:prop><d:getetag/><c:calendar-data/></d:prop><c:filter><c:comp-filter name="VCALENDAR"><c:comp-filter name="VEVENT"><c:time-range start="{start}" end="{end}"/></c:comp-filter></c:comp-filter></c:filter></c:calendar-query>"#
    );
    let resp = dav_request(http, cfg, report(), collection.as_str(), Some("1"), Some(XML_CT), &[], Some(body)).await?;
    if !is_dav_success(resp.status) {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for r in responses(&resp.body) {
        let Some(href) = elem_text(r, "href").filter(|s| !s.trim().is_empty()) else { continue };
        let ics = elem_text(r, "calendar-data").unwrap_or_default();
        if !ics.to_uppercase().contains("BEGIN:VEVENT") {
            continue;
        }
        let abs = collection.join(&href).map(|u| u.to_string()).unwrap_or(href);
        out.push(RemoteEvent { href: abs, etag: elem_text(r, "getetag"), ics });
    }
    Ok(out)
}

/// PUT an event's iCalendar to `href`. `if_match` uses conditional update semantics;
/// `None` creates with `If-None-Match: *`. Returns the new ETag if the server sent one.
async fn put_event(
    http: &reqwest::Client,
    cfg: &CalDavConfig,
    href: &str,
    ics: &str,
    if_match: Option<&str>,
) -> Result<Option<String>, AppError> {
    let extra: [(&str, &str); 1] = match if_match {
        Some(etag) => [("If-Match", etag)],
        None => [("If-None-Match", "*")],
    };
    let resp = dav_request(
        http,
        cfg,
        Method::PUT,
        href,
        None,
        Some("text/calendar; charset=utf-8"),
        &extra,
        Some(ics.to_string()),
    )
    .await?;
    if !is_dav_success(resp.status) {
        return Err(AppError::BadRequest(format!(
            "CalDAV rejected an event ({})",
            resp.status.as_u16()
        )));
    }
    Ok(resp.etag)
}

/// DELETE an event, tolerating already-gone. On a precondition failure (the remote
/// changed since we fetched its ETag) retry unconditionally, honouring the local
/// tombstone.
async fn delete_event(
    http: &reqwest::Client,
    cfg: &CalDavConfig,
    href: &str,
    if_match: Option<&str>,
) -> Result<(), AppError> {
    let extra: Vec<(&str, &str)> = if_match.map(|e| vec![("If-Match", e)]).unwrap_or_default();
    let resp = dav_request(http, cfg, Method::DELETE, href, None, None, &extra, None).await?;
    if is_dav_success(resp.status)
        || resp.status == StatusCode::NOT_FOUND
        || resp.status == StatusCode::GONE
    {
        return Ok(());
    }
    if resp.status == StatusCode::PRECONDITION_FAILED && if_match.is_some() {
        let retry = dav_request(http, cfg, Method::DELETE, href, None, None, &[], None).await?;
        if is_dav_success(retry.status)
            || retry.status == StatusCode::NOT_FOUND
            || retry.status == StatusCode::GONE
        {
            return Ok(());
        }
        return Err(AppError::BadRequest(format!(
            "CalDAV delete failed ({})",
            retry.status.as_u16()
        )));
    }
    Err(AppError::BadRequest(format!(
        "CalDAV delete failed ({})",
        resp.status.as_u16()
    )))
}

fn sanitize_uid(uid: &str) -> String {
    let cleaned: String = uid
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    let trimmed = cleaned.trim_matches('-');
    if trimmed.is_empty() {
        cal::new_uid()
    } else {
        trimmed.to_string()
    }
}

// ─────────────────────────── local rows ───────────────────────────

struct LocalRow {
    id: i64,
    remote_id: Option<String>,
    uid: String,
    etag: Option<String>,
    state: LocalState,
    card_json: String,
    calendar_id: i64,
    start_ms: i64,
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

/// Build a single-VEVENT iCalendar body for `card`, guaranteeing a stable UID.
fn card_to_ics(card: &cal::EventCard) -> String {
    let mut c = card.clone();
    if c.uid.trim().is_empty() {
        c.uid = cal::new_uid();
    }
    cal::build_ics(std::slice::from_ref(&c))
}

// ─────────────────────────── sync ───────────────────────────

pub async fn sync_caldav(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let Some(cfg) = load_config(&state, account_id).await? else {
        return Err(AppError::BadRequest(
            "CalDAV is not configured for this account.".into(),
        ));
    };
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let http = reqwest::Client::new();

    // Fetch window (±1 year), mirroring the Google integration.
    let now = chrono::Utc::now();
    let time_start = (now - Duration::days(365)).format("%Y%m%dT%H%M%SZ").to_string();
    let time_end = (now + Duration::days(365)).format("%Y%m%dT%H%M%SZ").to_string();
    let naive_now = chrono::Local::now().naive_local().and_utc().timestamp_millis();
    let win_from = naive_now - 365 * DAY_MS;
    let win_to = naive_now + 365 * DAY_MS;

    // 1. Discover + mirror calendars, remembering which local ids are CalDAV-backed.
    let collections = discover(&http, &cfg).await?;
    let mut caldav_cal_ids: HashSet<i64> = HashSet::new();
    let mut col_by_local: HashMap<i64, Url> = HashMap::new();
    let mut remote: Vec<(i64, RemoteEvent)> = Vec::new();
    for col in &collections {
        let local_cal = cal::ensure_named_calendar(&pool, &format!("{} (CalDAV)", col.display_name), &col.color).await?;
        set_remote_id(&pool, local_cal, col.url.as_str()).await;
        caldav_cal_ids.insert(local_cal);
        col_by_local.insert(local_cal, col.url.clone());
        for ev in fetch_events(&http, &cfg, &col.url, &time_start, &time_end).await? {
            remote.push((local_cal, ev));
        }
    }

    // 2. Load local events.
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
            calendar_id: r.try_get::<Option<i64>, _>("calendar_id").ok().flatten().unwrap_or(0),
            start_ms: r.try_get::<i64, _>("start_ms").unwrap_or(0),
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
            by_uid.entry(l.uid.clone()).or_insert(i);
        }
    }

    let mut seen: HashSet<usize> = HashSet::new();
    let mut pulled = 0i64;
    let mut pushed = 0i64;
    let mut deleted_remote = 0i64;

    // 3. Reconcile each remote event against local (ETag drives change detection).
    for (local_cal, ev) in &remote {
        let cards = cal::parse_ics(&ev.ics);
        let Some(mut card) = cards.into_iter().find(|c| !c.start.trim().is_empty()) else { continue };
        card.calendar_id = Some(*local_cal);
        let remote_etag = ev.etag.as_deref();

        let idx = by_remote
            .get(&ev.href)
            .or_else(|| if card.uid.is_empty() { None } else { by_uid.get(&card.uid) })
            .copied();
        match idx {
            None => {
                cal::sync_write_event(&pool, None, &card, &ev.href, remote_etag, 0).await?;
                pulled += 1;
            }
            Some(i) => {
                seen.insert(i);
                let local = &locals[i];
                if local.state.deleted {
                    delete_event(&http, &cfg, &ev.href, local.etag.as_deref()).await?;
                    cal::sync_hard_delete(&pool, "events", "event_id", local.id).await?;
                    deleted_remote += 1;
                } else if local.state.dirty {
                    // Local wins; overwrite the remote using the freshly-fetched
                    // ETag (mirrors the contacts path in `google_sync`). Using the
                    // stored ETag here would 412 on a genuine conflict and wedge
                    // every later sync.
                    if let Ok(lcard) = serde_json::from_str::<cal::EventCard>(&local.card_json) {
                        let new_etag = put_event(&http, &cfg, &ev.href, &card_to_ics(&lcard), remote_etag).await?;
                        cal::sync_mark_pushed(&pool, "events", "event_id", local.id, &ev.href, new_etag.as_deref(), 0).await?;
                        pushed += 1;
                    }
                } else if local.etag.as_deref() != remote_etag {
                    cal::sync_write_event(&pool, Some(local.id), &card, &ev.href, remote_etag, 0).await?;
                    pulled += 1;
                }
            }
        }
    }

    // 4. Orphans: local events with no matching remote item this round. Deletions
    //    are confined to CalDAV-backed calendars so a co-configured backend
    //    (e.g. Google) is never disturbed.
    for (i, local) in locals.iter().enumerate() {
        if seen.contains(&i) {
            continue;
        }
        let is_caldav = caldav_cal_ids.contains(&local.calendar_id);
        let in_scope = is_caldav && !local.recurs && local.start_ms >= win_from && local.start_ms < win_to;
        match reconcile_orphan(local.state, in_scope) {
            OrphanAction::Noop => {}
            OrphanAction::DropLocal => {
                // A tombstone we never pushed (or belongs to us): safe to drop.
                if !local.state.has_remote_id || is_caldav {
                    cal::sync_hard_delete(&pool, "events", "event_id", local.id).await?;
                }
            }
            OrphanAction::DeleteLocal => {
                if is_caldav {
                    cal::sync_hard_delete(&pool, "events", "event_id", local.id).await?;
                }
            }
            OrphanAction::CreateRemote | OrphanAction::RecreateRemote => {
                // Only push brand-new local events, or recreate events that live in
                // a CalDAV calendar; never re-home another backend's rows.
                if local.state.has_remote_id && !is_caldav {
                    continue;
                }
                if let Ok(lcard) = serde_json::from_str::<cal::EventCard>(&local.card_json) {
                    let Some(collection) = col_by_local
                        .get(&local.calendar_id)
                        .cloned()
                        .or_else(|| collections.first().map(|c| c.url.clone()))
                    else { continue };
                    let ics = card_to_ics(&lcard);
                    let uid = if lcard.uid.trim().is_empty() { cal::new_uid() } else { lcard.uid.clone() };
                    let Ok(href) = collection.join(&format!("{}.ics", sanitize_uid(&uid))) else { continue };
                    let new_etag = put_event(&http, &cfg, href.as_str(), &ics, None).await?;
                    cal::sync_mark_pushed(&pool, "events", "event_id", local.id, href.as_str(), new_etag.as_deref(), 0).await?;
                    pushed += 1;
                }
            }
        }
    }

    Ok(Json(json!({ "pulled": pulled, "pushed": pushed, "deletedRemote": deleted_remote })))
}

async fn set_remote_id(pool: &SqlitePool, calendar_id: i64, remote_id: &str) {
    let _ = sqlx::query("UPDATE calendars SET remote_id = ? WHERE calendar_id = ?")
        .bind(remote_id)
        .bind(calendar_id)
        .execute(pool)
        .await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_name_strips_prefix_and_attrs() {
        assert_eq!(local_name("D:href"), "href");
        assert_eq!(local_name("href"), "href");
        assert_eq!(local_name("cal:calendar-data xmlns=\"x\""), "calendar-data");
        assert_eq!(local_name("collection/"), "collection");
    }

    #[test]
    fn elem_text_reads_first_leaf() {
        let xml = r#"<d:response><d:href>/cal/1.ics</d:href><d:getetag>"abc"</d:getetag></d:response>"#;
        assert_eq!(elem_text(xml, "href").as_deref(), Some("/cal/1.ics"));
        assert_eq!(elem_text(xml, "getetag").as_deref(), Some("\"abc\""));
    }

    #[test]
    fn calendar_data_is_unescaped() {
        let xml = r#"<c:calendar-data>BEGIN:VEVENT&#13;
SUMMARY:A &amp; B&#13;
END:VEVENT</c:calendar-data>"#;
        let got = elem_text(xml, "calendar-data").unwrap();
        assert!(got.contains("SUMMARY:A & B"));
    }

    #[test]
    fn resourcetype_detects_calendar() {
        let rt = r#"<d:resourcetype><d:collection/><c:calendar/></d:resourcetype>"#;
        let inner = elem_inner(rt, "resourcetype").unwrap();
        assert!(has_elem(inner, "calendar"));
        let plain = r#"<d:resourcetype><d:collection/></d:resourcetype>"#;
        assert!(!has_elem(elem_inner(plain, "resourcetype").unwrap(), "calendar"));
    }

    #[test]
    fn responses_are_split() {
        let xml = r#"<d:multistatus><d:response><d:href>/a/</d:href></d:response><d:response><d:href>/b/</d:href></d:response></d:multistatus>"#;
        let rs = responses(xml);
        assert_eq!(rs.len(), 2);
        assert_eq!(elem_text(rs[0], "href").as_deref(), Some("/a/"));
        assert_eq!(elem_text(rs[1], "href").as_deref(), Some("/b/"));
    }

    #[test]
    fn principal_href_is_nested() {
        let body = r#"<d:multistatus><d:response><d:href>/</d:href><d:propstat><d:prop><d:current-user-principal><d:href>/principals/u/</d:href></d:current-user-principal></d:prop></d:propstat></d:response></d:multistatus>"#;
        let inner = elem_inner(body, "current-user-principal").unwrap();
        assert_eq!(elem_text(inner, "href").as_deref(), Some("/principals/u/"));
    }

    #[test]
    fn color_normalizes_apple_argb() {
        assert_eq!(normalize_color(Some("#FF5733FF".into())), "#FF5733");
        assert_eq!(normalize_color(Some("nope".into())), "#246bce");
        assert_eq!(normalize_color(None), "#246bce");
    }

    #[test]
    fn sanitize_uid_is_path_safe() {
        assert_eq!(sanitize_uid("abc-123_def.ics"), "abc-123_def.ics");
        assert_eq!(sanitize_uid("a/b c:d"), "a-b-c-d");
        assert!(!sanitize_uid("///").is_empty());
    }
}
