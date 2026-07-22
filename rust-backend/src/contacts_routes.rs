//! Contacts: rich, Outlook-parity address book with vCard (.vcf) import/export.
//!
//! The canonical representation of a contact is [`ContactCard`], serialized into
//! the `contacts.card_json` column. A handful of flat columns (display name,
//! first/last name, company, primary email, favorite flag, …) are kept in sync on
//! every write so the list/search/sort queries stay cheap and don't have to parse
//! JSON per row.
//!
//! Import/export speak vCard. The parser is deliberately lenient: it accepts both
//! vCard 3.0 (iPhone, Google, Thunderbird) and the older 2.1 dialect that
//! Microsoft Outlook exports — including RFC-2425 line folding, vCard-2.1
//! QUOTED-PRINTABLE soft line breaks, and bare `TYPE` parameters (`TEL;WORK;VOICE:`).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{header::CONTENT_DISPOSITION, header::CONTENT_TYPE, StatusCode},
    response::{IntoResponse, Response},
    Json,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{Row, SqlitePool};

use crate::{db, db::AppState, error::AppError};

// ─────────────────────────── Data model ───────────────────────────

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ContactName {
    #[serde(default)]
    pub prefix: String,
    #[serde(default)]
    pub first: String,
    #[serde(default)]
    pub middle: String,
    #[serde(default)]
    pub last: String,
    #[serde(default)]
    pub suffix: String,
    #[serde(default)]
    pub nickname: String,
    #[serde(default, rename = "fileAs")]
    pub file_as: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct LabeledValue {
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub value: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ContactAddress {
    #[serde(default)]
    pub label: String,
    #[serde(default, rename = "poBox")]
    pub po_box: String,
    #[serde(default)]
    pub street: String,
    #[serde(default)]
    pub city: String,
    #[serde(default)]
    pub state: String,
    #[serde(default, rename = "postalCode")]
    pub postal_code: String,
    #[serde(default)]
    pub country: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ContactOrg {
    #[serde(default)]
    pub company: String,
    #[serde(default)]
    pub department: String,
    #[serde(default, rename = "jobTitle")]
    pub job_title: String,
    #[serde(default)]
    pub office: String,
    #[serde(default)]
    pub profession: String,
    #[serde(default, rename = "managerName")]
    pub manager_name: String,
    #[serde(default, rename = "assistantName")]
    pub assistant_name: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ContactPersonal {
    #[serde(default)]
    pub birthday: String,
    #[serde(default)]
    pub spouse: String,
    #[serde(default)]
    pub notes: String,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct ContactCard {
    #[serde(default)]
    pub uid: String,
    #[serde(default)]
    pub name: ContactName,
    #[serde(default, rename = "displayName")]
    pub display_name: String,
    #[serde(default)]
    pub emails: Vec<LabeledValue>,
    #[serde(default)]
    pub phones: Vec<LabeledValue>,
    #[serde(default)]
    pub addresses: Vec<ContactAddress>,
    #[serde(default)]
    pub websites: Vec<LabeledValue>,
    #[serde(default)]
    pub im: Vec<LabeledValue>,
    #[serde(default)]
    pub organization: ContactOrg,
    #[serde(default)]
    pub personal: ContactPersonal,
    #[serde(default)]
    pub categories: Vec<String>,
    #[serde(default, rename = "isFavorite")]
    pub is_favorite: bool,
    /// Photo as a `data:` URL (or bare base64). Optional.
    #[serde(default)]
    pub photo: String,
}

impl ContactCard {
    /// Best-effort human label for the contact, mirroring Outlook's "File As".
    fn effective_display_name(&self) -> String {
        let dn = self.display_name.trim();
        if !dn.is_empty() {
            return dn.to_string();
        }
        let file_as = self.name.file_as.trim();
        if !file_as.is_empty() {
            return file_as.to_string();
        }
        let full = [self.name.first.trim(), self.name.middle.trim(), self.name.last.trim()]
            .iter()
            .filter(|s| !s.is_empty())
            .cloned()
            .collect::<Vec<_>>()
            .join(" ");
        if !full.is_empty() {
            return full;
        }
        if !self.organization.company.trim().is_empty() {
            return self.organization.company.trim().to_string();
        }
        if let Some(first_email) = self.emails.iter().find(|e| !e.value.trim().is_empty()) {
            return first_email.value.trim().to_string();
        }
        if let Some(first_phone) = self.phones.iter().find(|p| !p.value.trim().is_empty()) {
            return first_phone.value.trim().to_string();
        }
        String::new()
    }

    fn primary_email(&self) -> String {
        self.emails
            .iter()
            .map(|e| e.value.trim())
            .find(|v| !v.is_empty())
            .unwrap_or("")
            .to_string()
    }
}

/// One row as returned to the frontend: the parsed card plus stored metadata.
#[derive(Serialize)]
pub struct ContactRecord {
    pub id: i64,
    pub card: ContactCard,
    pub updated_at: Option<String>,
    pub created_at: Option<String>,
}

// ─────────────────────────── DB helpers ───────────────────────────

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> ContactRecord {
    let id: i64 = row.try_get("contact_id").unwrap_or_default();
    let card_json: Option<String> = row.try_get("card_json").ok().flatten();
    let card = card_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<ContactCard>(s).ok())
        .unwrap_or_else(|| legacy_card_from_row(row));
    ContactRecord {
        id,
        card,
        updated_at: row.try_get("updated_at").ok().flatten(),
        created_at: row.try_get("created_at").ok().flatten(),
    }
}

/// Reconstruct a card from the old flat columns for contacts created before the
/// `card_json` column existed.
fn legacy_card_from_row(row: &sqlx::sqlite::SqliteRow) -> ContactCard {
    let mut card = ContactCard::default();
    card.display_name = row
        .try_get::<Option<String>, _>("display_name")
        .ok()
        .flatten()
        .or_else(|| row.try_get::<Option<String>, _>("name").ok().flatten())
        .unwrap_or_default();
    if let Some(mail) = row
        .try_get::<Option<String>, _>("mail_address")
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
    {
        card.emails.push(LabeledValue {
            label: "work".into(),
            value: mail,
        });
    }
    if let Some(site) = row
        .try_get::<Option<String>, _>("website")
        .ok()
        .flatten()
        .filter(|s| !s.trim().is_empty())
    {
        card.websites.push(LabeledValue {
            label: "work".into(),
            value: site,
        });
    }
    card
}

const SELECT_COLS: &str = "contact_id, name, display_name, mail_address, website, card_json, created_at, updated_at";

async fn upsert_card(pool: &SqlitePool, id: Option<i64>, card: &ContactCard) -> Result<i64, AppError> {
    let card_json = serde_json::to_string(card).unwrap_or_else(|_| "{}".to_string());
    let categories_json = serde_json::to_string(&card.categories).unwrap_or_else(|_| "[]".to_string());
    let display_name = card.effective_display_name();
    let primary_email = card.primary_email();
    let primary_phone = card
        .phones
        .iter()
        .map(|p| p.value.trim())
        .find(|v| !v.is_empty())
        .unwrap_or("")
        .to_string();
    let website = card
        .websites
        .iter()
        .map(|w| w.value.trim())
        .find(|v| !v.is_empty())
        .unwrap_or("")
        .to_string();

    if let Some(id) = id {
        sqlx::query(
            r#"
            UPDATE contacts SET
                name = ?, display_name = ?, mail_address = ?, phone_number_country_code = ?,
                website = ?, uid = ?, first_name = ?, last_name = ?, company = ?, job_title = ?,
                is_favorite = ?, categories = ?, card_json = ?, updated_at = CURRENT_TIMESTAMP
            WHERE contact_id = ?
            "#,
        )
        .bind(&display_name)
        .bind(&display_name)
        .bind(&primary_email)
        .bind(&primary_phone)
        .bind(&website)
        .bind(&card.uid)
        .bind(&card.name.first)
        .bind(&card.name.last)
        .bind(&card.organization.company)
        .bind(&card.organization.job_title)
        .bind(card.is_favorite as i64)
        .bind(&categories_json)
        .bind(&card_json)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(id)
    } else {
        let res = sqlx::query(
            r#"
            INSERT INTO contacts
                (name, display_name, mail_address, phone_number_country_code, website, uid,
                 first_name, last_name, company, job_title, is_favorite, categories, card_json,
                 created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(&display_name)
        .bind(&display_name)
        .bind(&primary_email)
        .bind(&primary_phone)
        .bind(&website)
        .bind(&card.uid)
        .bind(&card.name.first)
        .bind(&card.name.last)
        .bind(&card.organization.company)
        .bind(&card.organization.job_title)
        .bind(card.is_favorite as i64)
        .bind(&categories_json)
        .bind(&card_json)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }
}

async fn fetch_record(pool: &SqlitePool, id: i64) -> Result<Option<ContactRecord>, AppError> {
    let row = sqlx::query(&format!(
        "SELECT {SELECT_COLS} FROM contacts WHERE contact_id = ?"
    ))
    .bind(id)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r| row_to_record(&r)))
}

/// Upsert a contact keyed on its stored UID — used by external (Google) sync so
/// re-running a sync updates the same rows instead of creating duplicates.
pub async fn upsert_contact_by_uid(pool: &SqlitePool, card: &ContactCard) -> Result<i64, AppError> {
    let existing: Option<i64> = if card.uid.trim().is_empty() {
        None
    } else {
        sqlx::query_scalar("SELECT contact_id FROM contacts WHERE uid = ? LIMIT 1")
            .bind(&card.uid)
            .fetch_optional(pool)
            .await?
    };
    let id = upsert_card(pool, existing, card).await?;
    reconcile_membership(pool, id, &card.categories).await?;
    Ok(id)
}

// ─────────────────────────── Handlers ───────────────────────────

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub search: Option<String>,
    #[serde(default)]
    pub favorites: Option<bool>,
    /// Filter to members of a single list.
    #[serde(default)]
    pub list: Option<i64>,
}

pub async fn list_contacts(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<ContactRecord>>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;

    let mut sql = format!("SELECT {SELECT_COLS} FROM contacts");
    let mut clauses: Vec<String> = Vec::new();
    if q.favorites.unwrap_or(false) {
        clauses.push("is_favorite = 1".to_string());
    }
    if let Some(list_id) = q.list {
        clauses.push(format!(
            "contact_id IN (SELECT contact_id FROM contact_list_members WHERE list_id = {list_id})"
        ));
    }
    let search = q.search.as_deref().map(str::trim).unwrap_or("").to_string();
    if !search.is_empty() {
        clauses.push(
            "(LOWER(COALESCE(display_name,'')) LIKE ? OR LOWER(COALESCE(mail_address,'')) LIKE ? \
             OR LOWER(COALESCE(company,'')) LIKE ? OR LOWER(COALESCE(card_json,'')) LIKE ?)"
                .to_string(),
        );
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    sql.push_str(" ORDER BY is_favorite DESC, LOWER(COALESCE(display_name, mail_address, '')) ASC");

    let mut query = sqlx::query(&sql);
    if !search.is_empty() {
        let pattern = format!("%{}%", search.to_lowercase());
        query = query
            .bind(pattern.clone())
            .bind(pattern.clone())
            .bind(pattern.clone())
            .bind(pattern);
    }
    let rows = query.fetch_all(&pool).await?;
    let mut records: Vec<ContactRecord> = rows.iter().map(row_to_record).collect();
    attach_categories(&pool, &mut records).await?;
    Ok(Json(records))
}

pub async fn get_contact(
    State(state): State<Arc<AppState>>,
    Path((account_id, contact_id)): Path<(i64, i64)>,
) -> Result<Json<ContactRecord>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let mut rec = fetch_record(&pool, contact_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Contact not found".into()))?;
    attach_categories(&pool, std::slice::from_mut(&mut rec)).await?;
    Ok(Json(rec))
}

/// Overwrite each record's `categories` with the authoritative list names from the
/// membership join, so renames/removals are always reflected on read.
async fn attach_categories(pool: &SqlitePool, records: &mut [ContactRecord]) -> Result<(), AppError> {
    for rec in records.iter_mut() {
        let names: Vec<String> = sqlx::query_scalar(
            r#"
            SELECT l.name FROM contact_list_members m
            JOIN contact_lists l ON l.list_id = m.list_id
            WHERE m.contact_id = ?
            ORDER BY LOWER(l.name)
            "#,
        )
        .bind(rec.id)
        .fetch_all(pool)
        .await
        .unwrap_or_default();
        rec.card.categories = names;
    }
    Ok(())
}

/// Reconcile a contact's list membership to exactly the given (case-insensitive,
/// de-duplicated) list names, auto-creating any list that does not exist yet.
async fn reconcile_membership(
    pool: &SqlitePool,
    contact_id: i64,
    names: &[String],
) -> Result<(), AppError> {
    let mut seen: Vec<String> = Vec::new();
    let mut ids: Vec<i64> = Vec::new();
    for raw in names {
        let name = raw.trim();
        if name.is_empty() || seen.iter().any(|s| s.eq_ignore_ascii_case(name)) {
            continue;
        }
        seen.push(name.to_string());
        let id = get_or_create_list(pool, name).await?;
        ids.push(id);
    }

    // Rebuild membership for this contact from scratch.
    sqlx::query("DELETE FROM contact_list_members WHERE contact_id = ?")
        .bind(contact_id)
        .execute(pool)
        .await?;
    for list_id in ids {
        sqlx::query(
            "INSERT OR IGNORE INTO contact_list_members (list_id, contact_id) VALUES (?, ?)",
        )
        .bind(list_id)
        .bind(contact_id)
        .execute(pool)
        .await?;
    }
    Ok(())
}

async fn get_or_create_list(pool: &SqlitePool, name: &str) -> Result<i64, AppError> {
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT list_id FROM contact_lists WHERE LOWER(name) = LOWER(?) LIMIT 1",
    )
    .bind(name)
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let res = sqlx::query("INSERT INTO contact_lists (name) VALUES (?)")
        .bind(name)
        .execute(pool)
        .await?;
    Ok(res.last_insert_rowid())
}

pub async fn create_contact(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(mut card): Json<ContactCard>,
) -> Result<Json<ContactRecord>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    if card.uid.trim().is_empty() {
        card.uid = new_uid();
    }
    let id = upsert_card(&pool, None, &card).await?;
    reconcile_membership(&pool, id, &card.categories).await?;
    let mut rec = fetch_record(&pool, id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Failed to load created contact".into()))?;
    attach_categories(&pool, std::slice::from_mut(&mut rec)).await?;
    Ok(Json(rec))
}

pub async fn update_contact(
    State(state): State<Arc<AppState>>,
    Path((account_id, contact_id)): Path<(i64, i64)>,
    Json(mut card): Json<ContactCard>,
) -> Result<Json<ContactRecord>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let existing = fetch_record(&pool, contact_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Contact not found".into()))?;
    if card.uid.trim().is_empty() {
        card.uid = if existing.card.uid.trim().is_empty() {
            new_uid()
        } else {
            existing.card.uid.clone()
        };
    }
    upsert_card(&pool, Some(contact_id), &card).await?;
    reconcile_membership(&pool, contact_id, &card.categories).await?;
    let mut rec = fetch_record(&pool, contact_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Failed to load updated contact".into()))?;
    attach_categories(&pool, std::slice::from_mut(&mut rec)).await?;
    Ok(Json(rec))
}

pub async fn delete_contact(
    State(state): State<Arc<AppState>>,
    Path((account_id, contact_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    sqlx::query("DELETE FROM contacts WHERE contact_id = ?")
        .bind(contact_id)
        .execute(&pool)
        .await?;
    Ok(Json(json!({ "status": "ok" })))
}

// ─────────────────────────── Import / Export ───────────────────────────

#[derive(Deserialize)]
pub struct ImportBody {
    /// Raw contents of a `.vcf` file (may contain many vCards).
    pub vcf: String,
    /// When true, contacts whose UID or primary email already exists are updated
    /// in place instead of inserted as duplicates.
    #[serde(default)]
    pub merge: bool,
}

#[derive(Serialize)]
pub struct ImportResult {
    pub imported: usize,
    pub updated: usize,
    pub skipped: usize,
}

pub async fn import_contacts(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<ImportBody>,
) -> Result<Json<ImportResult>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let cards = parse_vcards(&body.vcf);
    if cards.is_empty() {
        return Err(AppError::BadRequest(
            "No valid contacts found in the file.".into(),
        ));
    }

    let mut imported = 0usize;
    let mut updated = 0usize;
    let mut skipped = 0usize;

    for mut card in cards {
        if card.effective_display_name().is_empty() && card.primary_email().is_empty() {
            skipped += 1;
            continue;
        }
        if card.uid.trim().is_empty() {
            card.uid = new_uid();
        }

        let existing_id: Option<i64> = if body.merge {
            find_existing_id(&pool, &card).await
        } else {
            None
        };

        if let Some(id) = existing_id {
            upsert_card(&pool, Some(id), &card).await?;
            reconcile_membership(&pool, id, &card.categories).await?;
            updated += 1;
        } else {
            let id = upsert_card(&pool, None, &card).await?;
            reconcile_membership(&pool, id, &card.categories).await?;
            imported += 1;
        }
    }

    Ok(Json(ImportResult {
        imported,
        updated,
        skipped,
    }))
}

/// Find an existing contact matching by UID first, then by primary email.
async fn find_existing_id(pool: &SqlitePool, card: &ContactCard) -> Option<i64> {
    if !card.uid.trim().is_empty() {
        if let Ok(Some(id)) =
            sqlx::query_scalar::<_, i64>("SELECT contact_id FROM contacts WHERE uid = ? LIMIT 1")
                .bind(card.uid.trim())
                .fetch_optional(pool)
                .await
        {
            return Some(id);
        }
    }
    let email = card.primary_email();
    if !email.is_empty() {
        if let Ok(Some(id)) = sqlx::query_scalar::<_, i64>(
            "SELECT contact_id FROM contacts WHERE LOWER(mail_address) = LOWER(?) LIMIT 1",
        )
        .bind(&email)
        .fetch_optional(pool)
        .await
        {
            return Some(id);
        }
    }
    None
}

#[derive(Deserialize)]
pub struct ExportQuery {
    /// Optional comma-separated list of contact ids. When omitted, exports all.
    #[serde(default)]
    pub ids: Option<String>,
}

pub async fn export_contacts(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Query(q): Query<ExportQuery>,
) -> Result<Response, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;

    let rows = if let Some(ids_raw) = q.ids.as_deref().filter(|s| !s.trim().is_empty()) {
        let ids: Vec<i64> = ids_raw
            .split(',')
            .filter_map(|s| s.trim().parse::<i64>().ok())
            .collect();
        if ids.is_empty() {
            Vec::new()
        } else {
            let placeholders = ids.iter().map(|_| "?").collect::<Vec<_>>().join(",");
            let sql = format!(
                "SELECT {SELECT_COLS} FROM contacts WHERE contact_id IN ({placeholders}) \
                 ORDER BY LOWER(COALESCE(display_name, mail_address, '')) ASC"
            );
            let mut query = sqlx::query(&sql);
            for id in &ids {
                query = query.bind(id);
            }
            query.fetch_all(&pool).await?
        }
    } else {
        sqlx::query(&format!(
            "SELECT {SELECT_COLS} FROM contacts ORDER BY LOWER(COALESCE(display_name, mail_address, '')) ASC"
        ))
        .fetch_all(&pool)
        .await?
    };

    let mut records: Vec<ContactRecord> = rows.iter().map(row_to_record).collect();
    attach_categories(&pool, &mut records).await?;
    let mut out = String::new();
    for rec in &records {
        out.push_str(&card_to_vcard(&rec.card));
    }

    let filename = "contacts.vcf";
    Ok((
        StatusCode::OK,
        [
            (CONTENT_TYPE, "text/vcard; charset=utf-8".to_string()),
            (
                CONTENT_DISPOSITION,
                format!("attachment; filename=\"{filename}\""),
            ),
        ],
        out,
    )
        .into_response())
}

// ─────────────────────── Suggestions from mail history ───────────────────────

#[derive(Serialize)]
pub struct ContactSuggestion {
    pub name: String,
    pub email: String,
    pub count: i64,
}

/// Frequent correspondents (by From address) who are not already in the address
/// book — offered when creating a new contact.
pub async fn suggest_contacts(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Vec<ContactSuggestion>>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;

    // The local mail cache may not exist yet on a brand-new account; degrade to an
    // empty list rather than erroring.
    let rows = match sqlx::query(
        r#"
        SELECT sender_address AS email,
               COALESCE(MAX(sender_name), '') AS name,
               COUNT(*) AS cnt
        FROM local_mail_cache
        WHERE sender_address IS NOT NULL AND TRIM(sender_address) <> ''
          AND LOWER(sender_address) NOT IN (SELECT LOWER(COALESCE(mail_address,'')) FROM contacts)
        GROUP BY LOWER(sender_address)
        ORDER BY cnt DESC
        LIMIT 30
        "#,
    )
    .fetch_all(&pool)
    .await
    {
        Ok(rows) => rows,
        Err(_) => Vec::new(),
    };

    let suggestions = rows
        .iter()
        .map(|r| ContactSuggestion {
            email: r.try_get::<String, _>("email").unwrap_or_default(),
            name: r.try_get::<String, _>("name").unwrap_or_default(),
            count: r.try_get::<i64, _>("cnt").unwrap_or_default(),
        })
        .filter(|s| s.email.contains('@'))
        .collect();

    Ok(Json(suggestions))
}

// ─────────────────────────── Lists ───────────────────────────

#[derive(Serialize)]
pub struct ContactList {
    pub list_id: i64,
    pub name: String,
    pub count: i64,
}

#[derive(Serialize)]
pub struct ListsResponse {
    pub total: i64,
    pub favorites: i64,
    pub lists: Vec<ContactList>,
}

pub async fn get_lists(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<ListsResponse>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;

    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contacts")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let favorites: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contacts WHERE is_favorite = 1")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

    let rows = sqlx::query(
        r#"
        SELECT l.list_id AS list_id, l.name AS name,
               (SELECT COUNT(*) FROM contact_list_members m WHERE m.list_id = l.list_id) AS cnt
        FROM contact_lists l
        ORDER BY LOWER(l.name)
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let lists = rows
        .iter()
        .map(|r| ContactList {
            list_id: r.try_get("list_id").unwrap_or_default(),
            name: r.try_get("name").unwrap_or_default(),
            count: r.try_get("cnt").unwrap_or_default(),
        })
        .collect();

    Ok(Json(ListsResponse {
        total,
        favorites,
        lists,
    }))
}

#[derive(Deserialize)]
pub struct ListNameBody {
    pub name: String,
}

pub async fn create_list(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<ListNameBody>,
) -> Result<Json<ContactList>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("List name cannot be empty.".into()));
    }
    let existing: Option<i64> =
        sqlx::query_scalar("SELECT list_id FROM contact_lists WHERE LOWER(name) = LOWER(?)")
            .bind(name)
            .fetch_optional(&pool)
            .await?;
    if existing.is_some() {
        return Err(AppError::BadRequest("A list with that name already exists.".into()));
    }
    let id = get_or_create_list(&pool, name).await?;
    Ok(Json(ContactList {
        list_id: id,
        name: name.to_string(),
        count: 0,
    }))
}

pub async fn rename_list(
    State(state): State<Arc<AppState>>,
    Path((account_id, list_id)): Path<(i64, i64)>,
    Json(body): Json<ListNameBody>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let name = body.name.trim();
    if name.is_empty() {
        return Err(AppError::BadRequest("List name cannot be empty.".into()));
    }
    let clash: Option<i64> = sqlx::query_scalar(
        "SELECT list_id FROM contact_lists WHERE LOWER(name) = LOWER(?) AND list_id <> ?",
    )
    .bind(name)
    .bind(list_id)
    .fetch_optional(&pool)
    .await?;
    if clash.is_some() {
        return Err(AppError::BadRequest("A list with that name already exists.".into()));
    }
    let res = sqlx::query("UPDATE contact_lists SET name = ? WHERE list_id = ?")
        .bind(name)
        .bind(list_id)
        .execute(&pool)
        .await?;
    if res.rows_affected() == 0 {
        return Err(AppError::BadRequest("List not found.".into()));
    }
    Ok(Json(json!({ "status": "ok" })))
}

pub async fn delete_list(
    State(state): State<Arc<AppState>>,
    Path((account_id, list_id)): Path<(i64, i64)>,
) -> Result<Json<serde_json::Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    // Membership rows are removed via ON DELETE CASCADE.
    sqlx::query("DELETE FROM contact_lists WHERE list_id = ?")
        .bind(list_id)
        .execute(&pool)
        .await?;
    Ok(Json(json!({ "status": "ok" })))
}

// ─────────────────────────── vCard support ───────────────────────────

fn new_uid() -> String {
    format!(
        "guvercin-{}-{}",
        chrono::Utc::now().timestamp_millis(),
        fastrand_hex()
    )
}

fn fastrand_hex() -> String {
    // Cheap unique-ish suffix without pulling an extra dependency.
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("{nanos:08x}")
}

/// Split raw vCard file text into logical (unfolded) lines, honoring both
/// RFC-2425 whitespace folding and vCard-2.1 QUOTED-PRINTABLE `=` continuations.
fn unfold_lines(input: &str) -> Vec<String> {
    let mut logical: Vec<String> = Vec::new();
    for raw in input.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        if line.starts_with(' ') || line.starts_with('\t') {
            // RFC-2425 continuation: append without the single leading whitespace.
            if let Some(last) = logical.last_mut() {
                last.push_str(&line[1..]);
                continue;
            }
        }
        if let Some(last) = logical.last_mut() {
            // vCard 2.1 QUOTED-PRINTABLE soft line break: previous line ends with '='.
            if last.ends_with('=') && last.to_ascii_uppercase().contains("QUOTED-PRINTABLE") {
                last.pop(); // drop the trailing '='
                last.push_str(line);
                continue;
            }
        }
        logical.push(line.to_string());
    }
    logical
}

struct VProp {
    name: String,
    params: Vec<(String, String)>,
    value: String,
}

fn parse_prop_line(line: &str) -> Option<VProp> {
    let colon = line.find(':')?;
    let (head, value) = line.split_at(colon);
    let value = &value[1..];

    // Strip an optional grouping prefix ("item1.EMAIL").
    let head = match head.rsplit_once('.') {
        Some((_group, rest)) if !rest.is_empty() => rest,
        _ => head,
    };

    let mut parts = head.split(';');
    let name = parts.next().unwrap_or("").trim().to_ascii_uppercase();
    if name.is_empty() {
        return None;
    }
    let mut params = Vec::new();
    for p in parts {
        let p = p.trim();
        if p.is_empty() {
            continue;
        }
        if let Some((k, v)) = p.split_once('=') {
            params.push((k.trim().to_ascii_uppercase(), v.trim().to_string()));
        } else {
            // Bare parameter (vCard 2.1): treat as a TYPE token.
            params.push(("TYPE".to_string(), p.to_string()));
        }
    }

    Some(VProp {
        name,
        params,
        value: value.to_string(),
    })
}

fn is_quoted_printable(prop: &VProp) -> bool {
    prop.params
        .iter()
        .any(|(k, v)| k == "ENCODING" && v.to_ascii_uppercase().contains("QUOTED-PRINTABLE"))
}

fn decode_quoted_printable(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out: Vec<u8> = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'=' && i + 2 < bytes.len() {
            let hi = (bytes[i + 1] as char).to_digit(16);
            let lo = (bytes[i + 2] as char).to_digit(16);
            if let (Some(hi), Some(lo)) = (hi, lo) {
                out.push((hi * 16 + lo) as u8);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// Decode a raw property value into its final text form.
fn decode_value(prop: &VProp) -> String {
    if is_quoted_printable(prop) {
        decode_quoted_printable(&prop.value)
    } else {
        prop.value.clone()
    }
}

/// Split a structured value (e.g. `N`, `ADR`) on unescaped semicolons.
fn split_components(value: &str) -> Vec<String> {
    split_unescaped(value, ';')
}

fn split_unescaped(value: &str, sep: char) -> Vec<String> {
    let mut parts = Vec::new();
    let mut cur = String::new();
    let mut chars = value.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\\' {
            if let Some(&next) = chars.peek() {
                match next {
                    'n' | 'N' => cur.push('\n'),
                    other => cur.push(other),
                }
                chars.next();
            }
        } else if c == sep {
            parts.push(cur.clone());
            cur.clear();
        } else {
            cur.push(c);
        }
    }
    parts.push(cur);
    parts
}

fn unescape_text(value: &str) -> String {
    let parts = split_unescaped(value, '\u{0}'); // no split; reuse escape handling
    parts.join("")
}

/// Map a property's TYPE parameters to a simple label used by the UI.
fn label_from_types(prop: &VProp, kind: &str) -> String {
    let mut types: Vec<String> = Vec::new();
    for (k, v) in &prop.params {
        if k == "TYPE" {
            for t in v.split(',') {
                let t = t.trim().to_ascii_lowercase();
                if !t.is_empty() {
                    types.push(t);
                }
            }
        }
    }
    let has = |needle: &str| types.iter().any(|t| t == needle);

    match kind {
        "email" => {
            if has("home") {
                "home".into()
            } else if has("work") {
                "work".into()
            } else {
                "other".into()
            }
        }
        "tel" => {
            if has("cell") || has("mobile") {
                "mobile".into()
            } else if has("fax") && has("home") {
                "homeFax".into()
            } else if has("fax") {
                "workFax".into()
            } else if has("pager") {
                "pager".into()
            } else if has("main") {
                "main".into()
            } else if has("home") {
                "home".into()
            } else if has("work") || has("voice") {
                "work".into()
            } else {
                "other".into()
            }
        }
        "adr" => {
            if has("home") {
                "home".into()
            } else if has("work") {
                "work".into()
            } else {
                "other".into()
            }
        }
        _ => "other".into(),
    }
}

/// Parse a `.vcf` payload into contact cards.
pub fn parse_vcards(input: &str) -> Vec<ContactCard> {
    let lines = unfold_lines(input);
    let mut cards = Vec::new();
    let mut current: Option<ContactCard> = None;

    for line in &lines {
        let upper = line.trim().to_ascii_uppercase();
        if upper.starts_with("BEGIN:VCARD") {
            current = Some(ContactCard::default());
            continue;
        }
        if upper.starts_with("END:VCARD") {
            if let Some(card) = current.take() {
                cards.push(card);
            }
            continue;
        }
        let Some(card) = current.as_mut() else {
            continue;
        };
        let Some(prop) = parse_prop_line(line) else {
            continue;
        };
        apply_prop(card, &prop);
    }
    cards
}

fn apply_prop(card: &mut ContactCard, prop: &VProp) {
    let value = decode_value(prop);
    match prop.name.as_str() {
        "FN" => card.display_name = unescape_text(&value),
        "N" => {
            let c = split_components(&value);
            card.name.last = c.first().cloned().unwrap_or_default();
            card.name.first = c.get(1).cloned().unwrap_or_default();
            card.name.middle = c.get(2).cloned().unwrap_or_default();
            card.name.prefix = c.get(3).cloned().unwrap_or_default();
            card.name.suffix = c.get(4).cloned().unwrap_or_default();
        }
        "NICKNAME" => card.name.nickname = unescape_text(&value),
        "EMAIL" => {
            let v = value.trim().to_string();
            if !v.is_empty() {
                card.emails.push(LabeledValue {
                    label: label_from_types(prop, "email"),
                    value: v,
                });
            }
        }
        "TEL" => {
            let v = value.trim().to_string();
            if !v.is_empty() {
                card.phones.push(LabeledValue {
                    label: label_from_types(prop, "tel"),
                    value: v,
                });
            }
        }
        "ADR" => {
            let c = split_components(&value);
            let addr = ContactAddress {
                label: label_from_types(prop, "adr"),
                po_box: c.first().cloned().unwrap_or_default(),
                // c[1] = extended address (apartment/suite) — fold into street.
                street: {
                    let ext = c.get(1).cloned().unwrap_or_default();
                    let street = c.get(2).cloned().unwrap_or_default();
                    [ext, street]
                        .into_iter()
                        .filter(|s| !s.trim().is_empty())
                        .collect::<Vec<_>>()
                        .join(" ")
                },
                city: c.get(3).cloned().unwrap_or_default(),
                state: c.get(4).cloned().unwrap_or_default(),
                postal_code: c.get(5).cloned().unwrap_or_default(),
                country: c.get(6).cloned().unwrap_or_default(),
            };
            if [
                &addr.street,
                &addr.city,
                &addr.state,
                &addr.postal_code,
                &addr.country,
                &addr.po_box,
            ]
            .iter()
            .any(|s| !s.trim().is_empty())
            {
                card.addresses.push(addr);
            }
        }
        "ORG" => {
            let c = split_components(&value);
            card.organization.company = c.first().cloned().unwrap_or_default();
            card.organization.department = c.get(1).cloned().unwrap_or_default();
        }
        "TITLE" => card.organization.job_title = unescape_text(&value),
        "ROLE" => {
            if card.organization.profession.is_empty() {
                card.organization.profession = unescape_text(&value);
            }
        }
        "URL" => {
            let v = value.trim().to_string();
            if !v.is_empty() {
                card.websites.push(LabeledValue {
                    label: label_from_types(prop, "email"),
                    value: v,
                });
            }
        }
        "BDAY" => card.personal.birthday = normalize_date(&value),
        "NOTE" => card.personal.notes = unescape_text(&value),
        "CATEGORIES" => {
            card.categories = split_unescaped(&value, ',')
                .into_iter()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect();
        }
        "UID" => card.uid = value.trim().to_string(),
        "PHOTO" => {
            // Only keep inline base64 photos; ignore URI references.
            let is_uri = prop
                .params
                .iter()
                .any(|(k, v)| k == "VALUE" && v.eq_ignore_ascii_case("uri"));
            let is_b64 = prop.params.iter().any(|(k, v)| {
                k == "ENCODING" && (v.eq_ignore_ascii_case("b") || v.eq_ignore_ascii_case("base64"))
            });
            if is_b64 && !is_uri {
                let mime = prop
                    .params
                    .iter()
                    .find(|(k, _)| k == "TYPE")
                    .map(|(_, v)| v.to_ascii_lowercase())
                    .map(|t| {
                        if t.contains('/') {
                            t
                        } else {
                            format!("image/{t}")
                        }
                    })
                    .unwrap_or_else(|| "image/jpeg".to_string());
                let data = value.split_whitespace().collect::<String>();
                if !data.is_empty() {
                    card.photo = format!("data:{mime};base64,{data}");
                }
            } else if value.starts_with("data:") {
                card.photo = value.trim().to_string();
            }
        }
        "X-SPOUSE" | "X-MS-SPOUSE" => card.personal.spouse = unescape_text(&value),
        "X-MANAGERSNAME" | "X-MS-MANAGER" => {
            card.organization.manager_name = unescape_text(&value)
        }
        "X-ASSISTANTNAME" | "X-MS-ASSISTANT" => {
            card.organization.assistant_name = unescape_text(&value)
        }
        "X-MS-IMADDRESS" | "IMPP" | "X-JABBER" | "X-SKYPE" => {
            let v = value.trim().trim_start_matches("xmpp:").to_string();
            if !v.is_empty() {
                card.im.push(LabeledValue {
                    label: "im".into(),
                    value: v,
                });
            }
        }
        _ => {}
    }
}

fn normalize_date(value: &str) -> String {
    // Accept 19850413, 1985-04-13, 1985-04-13T00:00:00Z … → YYYY-MM-DD.
    let v = value.trim();
    let digits: String = v.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() >= 8 {
        format!("{}-{}-{}", &digits[0..4], &digits[4..6], &digits[6..8])
    } else {
        v.to_string()
    }
}

// ─────────────────────── vCard serialization (3.0) ───────────────────────

fn escape_text(value: &str) -> String {
    value
        .replace('\\', "\\\\")
        .replace('\n', "\\n")
        .replace(',', "\\,")
        .replace(';', "\\;")
}

/// Fold a logical line to 75 octets per RFC-2426.
fn fold_line(line: &str) -> String {
    let bytes = line.as_bytes();
    if bytes.len() <= 75 {
        return line.to_string();
    }
    let mut out = String::new();
    let mut count = 0;
    for ch in line.chars() {
        let ch_len = ch.len_utf8();
        if count + ch_len > 75 {
            out.push_str("\r\n ");
            count = 1; // the leading space
        }
        out.push(ch);
        count += ch_len;
    }
    out
}

fn push_line(out: &mut String, line: String) {
    out.push_str(&fold_line(&line));
    out.push_str("\r\n");
}

fn tel_type(label: &str) -> &'static str {
    match label {
        "mobile" => "CELL",
        "home" => "HOME",
        "work" => "WORK",
        "main" => "MAIN",
        "homeFax" => "HOME,FAX",
        "workFax" => "WORK,FAX",
        "fax" => "FAX",
        "pager" => "PAGER",
        _ => "VOICE",
    }
}

fn simple_type(label: &str) -> &'static str {
    match label {
        "home" => "HOME",
        "work" => "WORK",
        _ => "OTHER",
    }
}

/// Serialize a single contact to a vCard 3.0 record (CRLF-terminated).
pub fn card_to_vcard(card: &ContactCard) -> String {
    let mut out = String::new();
    push_line(&mut out, "BEGIN:VCARD".to_string());
    push_line(&mut out, "VERSION:3.0".to_string());

    // N: Last;First;Middle;Prefix;Suffix
    push_line(
        &mut out,
        format!(
            "N:{};{};{};{};{}",
            escape_text(&card.name.last),
            escape_text(&card.name.first),
            escape_text(&card.name.middle),
            escape_text(&card.name.prefix),
            escape_text(&card.name.suffix),
        ),
    );

    let fn_value = card.effective_display_name();
    push_line(&mut out, format!("FN:{}", escape_text(&fn_value)));

    if !card.name.nickname.trim().is_empty() {
        push_line(
            &mut out,
            format!("NICKNAME:{}", escape_text(&card.name.nickname)),
        );
    }

    if !card.organization.company.trim().is_empty()
        || !card.organization.department.trim().is_empty()
    {
        push_line(
            &mut out,
            format!(
                "ORG:{};{}",
                escape_text(&card.organization.company),
                escape_text(&card.organization.department)
            ),
        );
    }
    if !card.organization.job_title.trim().is_empty() {
        push_line(
            &mut out,
            format!("TITLE:{}", escape_text(&card.organization.job_title)),
        );
    }
    if !card.organization.profession.trim().is_empty() {
        push_line(
            &mut out,
            format!("ROLE:{}", escape_text(&card.organization.profession)),
        );
    }

    for email in &card.emails {
        if email.value.trim().is_empty() {
            continue;
        }
        push_line(
            &mut out,
            format!(
                "EMAIL;TYPE={},INTERNET:{}",
                simple_type(&email.label),
                escape_text(email.value.trim())
            ),
        );
    }

    for phone in &card.phones {
        if phone.value.trim().is_empty() {
            continue;
        }
        push_line(
            &mut out,
            format!(
                "TEL;TYPE={}:{}",
                tel_type(&phone.label),
                escape_text(phone.value.trim())
            ),
        );
    }

    for addr in &card.addresses {
        if [
            &addr.po_box,
            &addr.street,
            &addr.city,
            &addr.state,
            &addr.postal_code,
            &addr.country,
        ]
        .iter()
        .all(|s| s.trim().is_empty())
        {
            continue;
        }
        push_line(
            &mut out,
            format!(
                "ADR;TYPE={}:{};;{};{};{};{};{}",
                simple_type(&addr.label),
                escape_text(&addr.po_box),
                escape_text(&addr.street),
                escape_text(&addr.city),
                escape_text(&addr.state),
                escape_text(&addr.postal_code),
                escape_text(&addr.country),
            ),
        );
    }

    for site in &card.websites {
        if site.value.trim().is_empty() {
            continue;
        }
        push_line(&mut out, format!("URL:{}", escape_text(site.value.trim())));
    }

    for im in &card.im {
        if im.value.trim().is_empty() {
            continue;
        }
        push_line(
            &mut out,
            format!("IMPP:{}", escape_text(im.value.trim())),
        );
        push_line(
            &mut out,
            format!("X-MS-IMADDRESS:{}", escape_text(im.value.trim())),
        );
    }

    if !card.personal.birthday.trim().is_empty() {
        push_line(
            &mut out,
            format!("BDAY:{}", escape_text(card.personal.birthday.trim())),
        );
    }
    if !card.personal.spouse.trim().is_empty() {
        push_line(
            &mut out,
            format!("X-SPOUSE:{}", escape_text(card.personal.spouse.trim())),
        );
    }
    if !card.organization.manager_name.trim().is_empty() {
        push_line(
            &mut out,
            format!(
                "X-MANAGERSNAME:{}",
                escape_text(card.organization.manager_name.trim())
            ),
        );
    }
    if !card.organization.assistant_name.trim().is_empty() {
        push_line(
            &mut out,
            format!(
                "X-ASSISTANTNAME:{}",
                escape_text(card.organization.assistant_name.trim())
            ),
        );
    }
    if !card.personal.notes.trim().is_empty() {
        push_line(&mut out, format!("NOTE:{}", escape_text(&card.personal.notes)));
    }
    if !card.categories.is_empty() {
        let cats = card
            .categories
            .iter()
            .map(|c| escape_text(c))
            .collect::<Vec<_>>()
            .join(",");
        push_line(&mut out, format!("CATEGORIES:{cats}"));
    }

    // Inline photo (data URL → PHOTO;ENCODING=b).
    if let Some((mime, b64)) = parse_data_url(&card.photo) {
        push_line(
            &mut out,
            format!("PHOTO;ENCODING=b;TYPE={}:{}", mime.to_ascii_uppercase(), b64),
        );
    }

    let uid = if card.uid.trim().is_empty() {
        new_uid()
    } else {
        card.uid.trim().to_string()
    };
    push_line(&mut out, format!("UID:{uid}"));

    push_line(&mut out, "END:VCARD".to_string());
    out
}

fn parse_data_url(photo: &str) -> Option<(String, String)> {
    let p = photo.trim();
    if !p.starts_with("data:") {
        return None;
    }
    let rest = &p[5..];
    let (meta, data) = rest.split_once(',')?;
    if !meta.to_ascii_lowercase().contains("base64") {
        return None;
    }
    let mime = meta.split(';').next().unwrap_or("image/jpeg");
    let subtype = mime.split('/').nth(1).unwrap_or("jpeg").to_string();
    let cleaned: String = data.split_whitespace().collect();
    if cleaned.is_empty() {
        return None;
    }
    Some((subtype, cleaned))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sqlx::sqlite::SqlitePoolOptions;

    async fn test_pool() -> SqlitePool {
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .connect("sqlite::memory:")
            .await
            .unwrap();
        // Mirror the contacts schema created in db::init_user_db.
        sqlx::query(
            r#"
            CREATE TABLE contacts (
                contact_id    INTEGER PRIMARY KEY AUTOINCREMENT,
                name          TEXT,
                display_name  TEXT,
                mail_address  TEXT,
                phone_number_country_code TEXT,
                phone_number  INTEGER,
                fax_number    INTEGER,
                website       TEXT,
                last_contact_time DATETIME,
                avatar_data   BLOB,
                uid           TEXT,
                first_name    TEXT,
                last_name     TEXT,
                company       TEXT,
                job_title     TEXT,
                is_favorite   INTEGER NOT NULL DEFAULT 0,
                categories    TEXT,
                card_json     TEXT,
                created_at    DATETIME DEFAULT CURRENT_TIMESTAMP,
                updated_at    DATETIME DEFAULT CURRENT_TIMESTAMP
            )
            "#,
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE contact_lists (list_id INTEGER PRIMARY KEY AUTOINCREMENT, name TEXT NOT NULL UNIQUE, created_at DATETIME DEFAULT CURRENT_TIMESTAMP)",
        )
        .execute(&pool)
        .await
        .unwrap();
        sqlx::query(
            "CREATE TABLE contact_list_members (list_id INTEGER NOT NULL, contact_id INTEGER NOT NULL, PRIMARY KEY (list_id, contact_id))",
        )
        .execute(&pool)
        .await
        .unwrap();
        pool
    }

    #[tokio::test]
    async fn crud_and_denormalized_columns() {
        let pool = test_pool().await;
        let mut card = ContactCard::default();
        card.name.first = "Ada".into();
        card.name.last = "Lovelace".into();
        card.emails.push(LabeledValue { label: "work".into(), value: "ada@math.test".into() });
        card.is_favorite = true;

        let id = upsert_card(&pool, None, &card).await.unwrap();
        let rec = fetch_record(&pool, id).await.unwrap().unwrap();
        assert_eq!(rec.card.name.first, "Ada");
        assert_eq!(rec.card.emails[0].value, "ada@math.test");
        assert!(rec.card.is_favorite);

        // Denormalized columns are populated for cheap listing/search.
        let (dn, mail, fav): (String, String, i64) = sqlx::query_as(
            "SELECT display_name, mail_address, is_favorite FROM contacts WHERE contact_id = ?",
        )
        .bind(id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(dn, "Ada Lovelace");
        assert_eq!(mail, "ada@math.test");
        assert_eq!(fav, 1);

        // Update in place.
        card.organization.company = "Analytical Engines".into();
        upsert_card(&pool, Some(id), &card).await.unwrap();
        let rec = fetch_record(&pool, id).await.unwrap().unwrap();
        assert_eq!(rec.card.organization.company, "Analytical Engines");

        // Match existing by primary email (import merge path).
        let found = find_existing_id(&pool, &card).await;
        assert_eq!(found, Some(id));
    }

    #[tokio::test]
    async fn import_export_roundtrip_through_db() {
        let pool = test_pool().await;
        let vcf = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Grace Hopper\r\nN:Hopper;Grace;;;\r\nEMAIL;TYPE=WORK:grace@navy.test\r\nTEL;TYPE=CELL:+1555\r\nEND:VCARD\r\n";
        for card in parse_vcards(vcf) {
            upsert_card(&pool, None, &card).await.unwrap();
        }
        let rows = sqlx::query(&format!("SELECT {SELECT_COLS} FROM contacts"))
            .fetch_all(&pool)
            .await
            .unwrap();
        assert_eq!(rows.len(), 1);
        let rec = row_to_record(&rows[0]);
        assert_eq!(rec.card.name.last, "Hopper");

        // Export the stored card back to vCard and re-parse it.
        let exported = card_to_vcard(&rec.card);
        let reparsed = parse_vcards(&exported);
        assert_eq!(reparsed[0].emails[0].value, "grace@navy.test");
        assert_eq!(reparsed[0].phones[0].label, "mobile");
    }

    #[tokio::test]
    async fn list_membership_reconcile_and_rename() {
        let pool = test_pool().await;
        let mut card = ContactCard::default();
        card.name.first = "Bob".into();
        card.categories = vec!["Friends".into(), "VIP".into(), "friends".into()]; // dup, diff case
        let id = upsert_card(&pool, None, &card).await.unwrap();
        reconcile_membership(&pool, id, &card.categories).await.unwrap();

        // De-duplicated to two lists.
        let list_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_lists")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(list_count, 2);
        let member_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_list_members WHERE contact_id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(member_count, 2);

        // attach_categories reflects membership.
        let mut rec = fetch_record(&pool, id).await.unwrap().unwrap();
        attach_categories(&pool, std::slice::from_mut(&mut rec)).await.unwrap();
        assert_eq!(rec.card.categories.len(), 2);
        assert!(rec.card.categories.iter().any(|c| c == "VIP"));

        // Rename a list → read reflects the new name.
        let vip_id: i64 = sqlx::query_scalar("SELECT list_id FROM contact_lists WHERE name = 'VIP'")
            .fetch_one(&pool)
            .await
            .unwrap();
        sqlx::query("UPDATE contact_lists SET name = 'Very Important' WHERE list_id = ?")
            .bind(vip_id)
            .execute(&pool)
            .await
            .unwrap();
        let mut rec = fetch_record(&pool, id).await.unwrap().unwrap();
        attach_categories(&pool, std::slice::from_mut(&mut rec)).await.unwrap();
        assert!(rec.card.categories.iter().any(|c| c == "Very Important"));
        assert!(!rec.card.categories.iter().any(|c| c == "VIP"));

        // Removing all categories clears membership.
        reconcile_membership(&pool, id, &[]).await.unwrap();
        let member_count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM contact_list_members WHERE contact_id = ?")
            .bind(id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(member_count, 0);
    }

    #[test]
    fn parses_vcard_30() {
        let vcf = "BEGIN:VCARD\r\nVERSION:3.0\r\nFN:Jane Doe\r\nN:Doe;Jane;;;\r\nEMAIL;TYPE=WORK,INTERNET:jane@example.com\r\nTEL;TYPE=CELL:+15551234567\r\nORG:Acme;Sales\r\nTITLE:Manager\r\nEND:VCARD\r\n";
        let cards = parse_vcards(vcf);
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!(c.display_name, "Jane Doe");
        assert_eq!(c.name.first, "Jane");
        assert_eq!(c.name.last, "Doe");
        assert_eq!(c.emails.len(), 1);
        assert_eq!(c.emails[0].value, "jane@example.com");
        assert_eq!(c.emails[0].label, "work");
        assert_eq!(c.phones[0].label, "mobile");
        assert_eq!(c.organization.company, "Acme");
        assert_eq!(c.organization.department, "Sales");
        assert_eq!(c.organization.job_title, "Manager");
    }

    #[test]
    fn parses_outlook_21_quoted_printable() {
        // Outlook 2.1 export with a QUOTED-PRINTABLE UTF-8 name (Ş = C5 9E).
        let vcf = "BEGIN:VCARD\r\nVERSION:2.1\r\nN;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:=C5=9Een;Ali\r\nFN;ENCODING=QUOTED-PRINTABLE;CHARSET=UTF-8:Ali =C5=9Een\r\nTEL;WORK;VOICE:+902121234567\r\nEND:VCARD\r\n";
        let cards = parse_vcards(vcf);
        assert_eq!(cards.len(), 1);
        let c = &cards[0];
        assert_eq!(c.display_name, "Ali Şen");
        assert_eq!(c.name.last, "Şen");
        assert_eq!(c.name.first, "Ali");
        assert_eq!(c.phones[0].label, "work");
    }

    #[test]
    fn roundtrip_export_import() {
        let mut card = ContactCard::default();
        card.name.first = "John".into();
        card.name.last = "Smith".into();
        card.emails.push(LabeledValue {
            label: "home".into(),
            value: "john@home.test".into(),
        });
        card.phones.push(LabeledValue {
            label: "mobile".into(),
            value: "+1000".into(),
        });
        card.addresses.push(ContactAddress {
            label: "work".into(),
            street: "1 Main St".into(),
            city: "Town".into(),
            postal_code: "12345".into(),
            country: "USA".into(),
            ..Default::default()
        });
        let vcf = card_to_vcard(&card);
        let parsed = parse_vcards(&vcf);
        assert_eq!(parsed.len(), 1);
        let p = &parsed[0];
        assert_eq!(p.name.first, "John");
        assert_eq!(p.name.last, "Smith");
        assert_eq!(p.emails[0].value, "john@home.test");
        assert_eq!(p.emails[0].label, "home");
        assert_eq!(p.phones[0].label, "mobile");
        assert_eq!(p.addresses[0].city, "Town");
        assert_eq!(p.addresses[0].country, "USA");
    }
}
