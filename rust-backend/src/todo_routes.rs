//! Tasks (the "Todo" tab): lists of to-do items with due dates, priorities,
//! starring, subtasks and completion — mirroring the Contacts/Calendar design.
//!
//! The canonical task is [`TaskCard`], serialized into `tasks.task_json`. Flat
//! columns (title, due, priority, completed, …) are kept in sync on every write so
//! listing, sorting and the overdue/today/upcoming bucketing the frontend does
//! stay cheap. `due_ms` is naive wall-clock epoch millis, matching the calendar.

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    Json,
};
use chrono::{NaiveDate, NaiveDateTime};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sqlx::{Row, SqlitePool};

use crate::{db, db::AppState, error::AppError};

// ─────────────────────────── Data model ───────────────────────────

const DAY_MS: i64 = 86_400_000;

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct Subtask {
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub done: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct TaskCard {
    #[serde(default)]
    pub uid: String,
    #[serde(default, rename = "listId")]
    pub list_id: Option<i64>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub notes: String,
    /// "" | "YYYY-MM-DD" | "YYYY-MM-DDTHH:MM"
    #[serde(default)]
    pub due: String,
    #[serde(default, rename = "hasDueTime")]
    pub has_due_time: bool,
    /// "none" | "low" | "medium" | "high"
    #[serde(default)]
    pub priority: String,
    #[serde(default)]
    pub completed: bool,
    #[serde(default)]
    pub starred: bool,
    #[serde(default)]
    pub subtasks: Vec<Subtask>,
}

impl Default for TaskCard {
    fn default() -> Self {
        TaskCard {
            uid: String::new(),
            list_id: None,
            title: String::new(),
            notes: String::new(),
            due: String::new(),
            has_due_time: false,
            priority: "none".to_string(),
            completed: false,
            starred: false,
            subtasks: Vec::new(),
        }
    }
}

impl TaskCard {
    fn priority_int(&self) -> i64 {
        match self.priority.as_str() {
            "low" => 1,
            "medium" => 2,
            "high" => 3,
            _ => 0,
        }
    }
    fn due_ms(&self) -> Option<i64> {
        parse_iso_ms(&self.due)
    }
    fn has_time(&self) -> bool {
        self.due.contains('T')
    }
}

#[derive(Serialize)]
pub struct TaskRecord {
    pub id: i64,
    pub card: TaskCard,
    #[serde(rename = "createdAt")]
    pub created_at: Option<String>,
    #[serde(rename = "updatedAt")]
    pub updated_at: Option<String>,
}

#[derive(Serialize)]
pub struct TaskListSummary {
    #[serde(rename = "listId")]
    pub list_id: i64,
    pub name: String,
    pub color: String,
    #[serde(rename = "isDefault")]
    pub is_default: bool,
    /// Count of incomplete tasks.
    pub count: i64,
}

fn parse_iso_ms(s: &str) -> Option<i64> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }
    for fmt in ["%Y-%m-%dT%H:%M:%S", "%Y-%m-%dT%H:%M"] {
        if let Ok(dt) = NaiveDateTime::parse_from_str(s, fmt) {
            return Some(dt.and_utc().timestamp_millis());
        }
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return d.and_hms_opt(0, 0, 0).map(|dt| dt.and_utc().timestamp_millis());
    }
    None
}

// ─────────────────────────── DB helpers ───────────────────────────

const SELECT_COLS: &str =
    "task_id, list_id, uid, title, notes, due_ms, has_due_time, priority, completed, starred, position, task_json, created_at, updated_at";

fn row_to_record(row: &sqlx::sqlite::SqliteRow) -> TaskRecord {
    let id: i64 = row.try_get("task_id").unwrap_or_default();
    let task_json: Option<String> = row.try_get("task_json").ok().flatten();
    let mut card = task_json
        .as_deref()
        .and_then(|s| serde_json::from_str::<TaskCard>(s).ok())
        .unwrap_or_default();
    if let Ok(lid) = row.try_get::<Option<i64>, _>("list_id") {
        card.list_id = lid;
    }
    TaskRecord {
        id,
        card,
        created_at: row.try_get("created_at").ok().flatten(),
        updated_at: row.try_get("updated_at").ok().flatten(),
    }
}

async fn ensure_default_list(pool: &SqlitePool) -> Result<i64, AppError> {
    if let Some(id) = sqlx::query_scalar::<_, i64>(
        "SELECT list_id FROM task_lists ORDER BY is_default DESC, list_id ASC LIMIT 1",
    )
    .fetch_optional(pool)
    .await?
    {
        return Ok(id);
    }
    let res = sqlx::query(
        "INSERT INTO task_lists (name, color, is_default, sort_order) VALUES (?, ?, 1, 0)",
    )
    .bind("Tasks")
    .bind("#246bce")
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

async fn resolve_list_id(pool: &SqlitePool, requested: Option<i64>) -> Result<i64, AppError> {
    if let Some(id) = requested {
        let exists = sqlx::query_scalar::<_, i64>("SELECT 1 FROM task_lists WHERE list_id = ?")
            .bind(id)
            .fetch_optional(pool)
            .await?;
        if exists.is_some() {
            return Ok(id);
        }
    }
    ensure_default_list(pool).await
}

async fn upsert_task(pool: &SqlitePool, id: Option<i64>, card: &TaskCard) -> Result<i64, AppError> {
    let task_json = serde_json::to_string(card).unwrap_or_else(|_| "{}".to_string());
    let due_ms = card.due_ms();
    let has_time = card.has_time() as i64;
    let priority = card.priority_int();

    if let Some(id) = id {
        sqlx::query(
            r#"
            UPDATE tasks SET
                list_id = ?, uid = ?, title = ?, notes = ?, due_ms = ?, has_due_time = ?,
                priority = ?, completed = ?, starred = ?, task_json = ?,
                completed_at = CASE WHEN ? = 1 AND completed = 0 THEN CURRENT_TIMESTAMP
                                    WHEN ? = 0 THEN NULL ELSE completed_at END,
                updated_at = CURRENT_TIMESTAMP
            WHERE task_id = ?
            "#,
        )
        .bind(card.list_id)
        .bind(&card.uid)
        .bind(&card.title)
        .bind(&card.notes)
        .bind(due_ms)
        .bind(has_time)
        .bind(priority)
        .bind(card.completed as i64)
        .bind(card.starred as i64)
        .bind(&task_json)
        .bind(card.completed as i64)
        .bind(card.completed as i64)
        .bind(id)
        .execute(pool)
        .await?;
        Ok(id)
    } else {
        let res = sqlx::query(
            r#"
            INSERT INTO tasks
                (list_id, uid, title, notes, due_ms, has_due_time, priority, completed, starred,
                 completed_at, position, task_json, created_at, updated_at)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?,
                    (SELECT COALESCE(MAX(position), 0) + 1 FROM tasks), ?, CURRENT_TIMESTAMP, CURRENT_TIMESTAMP)
            "#,
        )
        .bind(card.list_id)
        .bind(&card.uid)
        .bind(&card.title)
        .bind(&card.notes)
        .bind(due_ms)
        .bind(has_time)
        .bind(priority)
        .bind(card.completed as i64)
        .bind(card.starred as i64)
        .bind(if card.completed { Some(chrono::Utc::now().to_rfc3339()) } else { None })
        .bind(&task_json)
        .execute(pool)
        .await?;
        Ok(res.last_insert_rowid())
    }
}

async fn fetch_record(pool: &SqlitePool, id: i64) -> Result<Option<TaskRecord>, AppError> {
    let row = sqlx::query(&format!("SELECT {SELECT_COLS} FROM tasks WHERE task_id = ?"))
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(row.map(|r| row_to_record(&r)))
}

/// Upsert a task keyed on its stored UID (used by Google Tasks sync).
pub async fn upsert_task_by_uid(pool: &SqlitePool, card: &TaskCard) -> Result<i64, AppError> {
    let existing: Option<i64> = if card.uid.trim().is_empty() {
        None
    } else {
        sqlx::query_scalar("SELECT task_id FROM tasks WHERE uid = ? LIMIT 1")
            .bind(&card.uid)
            .fetch_optional(pool)
            .await?
    };
    upsert_task(pool, existing, card).await
}

/// Find or create a task list by name (used to mirror Google task lists).
pub async fn ensure_named_list(pool: &SqlitePool, name: &str) -> Result<i64, AppError> {
    if let Some(id) = sqlx::query_scalar::<_, i64>("SELECT list_id FROM task_lists WHERE name = ? LIMIT 1")
        .bind(name)
        .fetch_optional(pool)
        .await?
    {
        return Ok(id);
    }
    let res = sqlx::query(
        "INSERT INTO task_lists (name, color, is_default, sort_order) VALUES (?, '#2e7d32', 0, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM task_lists))",
    )
    .bind(name)
    .execute(pool)
    .await?;
    Ok(res.last_insert_rowid())
}

fn sanitize_color(input: &str) -> String {
    let s = input.trim();
    if s.starts_with('#') && (s.len() == 7 || s.len() == 4) && s[1..].chars().all(|c| c.is_ascii_hexdigit()) {
        s.to_string()
    } else {
        "#246bce".to_string()
    }
}

fn new_uid() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    format!("guvercin-task-{}-{:08x}", chrono::Utc::now().timestamp_millis(), nanos)
}

// ─────────────────────────── Task handlers ───────────────────────────

#[derive(Deserialize)]
pub struct ListQuery {
    #[serde(default)]
    pub list: Option<i64>,
    #[serde(default)]
    pub completed: Option<bool>,
    #[serde(default)]
    pub starred: Option<bool>,
    #[serde(default)]
    pub search: Option<String>,
}

pub async fn list_tasks(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<TaskRecord>>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    ensure_default_list(&pool).await?;

    let mut sql = format!("SELECT {SELECT_COLS} FROM tasks");
    let mut clauses: Vec<String> = Vec::new();
    if let Some(list_id) = q.list {
        clauses.push(format!("list_id = {list_id}"));
    }
    if let Some(done) = q.completed {
        clauses.push(format!("completed = {}", if done { 1 } else { 0 }));
    }
    if q.starred.unwrap_or(false) {
        clauses.push("starred = 1".to_string());
    }
    let search = q.search.as_deref().map(str::trim).unwrap_or("").to_lowercase();
    if !search.is_empty() {
        clauses.push(
            "(LOWER(COALESCE(title,'')) LIKE ? OR LOWER(COALESCE(notes,'')) LIKE ?)".to_string(),
        );
    }
    if !clauses.is_empty() {
        sql.push_str(" WHERE ");
        sql.push_str(&clauses.join(" AND "));
    }
    // Incomplete first; dated before undated; earliest due first; higher priority first.
    sql.push_str(
        " ORDER BY completed ASC, (due_ms IS NULL) ASC, due_ms ASC, priority DESC, position ASC, LOWER(COALESCE(title,'')) ASC",
    );

    let mut query = sqlx::query(&sql);
    if !search.is_empty() {
        let pat = format!("%{search}%");
        query = query.bind(pat.clone()).bind(pat);
    }
    let rows = query.fetch_all(&pool).await?;
    Ok(Json(rows.iter().map(row_to_record).collect()))
}

pub async fn get_task(
    State(state): State<Arc<AppState>>,
    Path((account_id, task_id)): Path<(i64, i64)>,
) -> Result<Json<TaskRecord>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let rec = fetch_record(&pool, task_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Task not found".into()))?;
    Ok(Json(rec))
}

pub async fn create_task(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(mut card): Json<TaskCard>,
) -> Result<Json<TaskRecord>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    if card.uid.trim().is_empty() {
        card.uid = new_uid();
    }
    card.list_id = Some(resolve_list_id(&pool, card.list_id).await?);
    let id = upsert_task(&pool, None, &card).await?;
    let rec = fetch_record(&pool, id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Failed to load created task".into()))?;
    Ok(Json(rec))
}

pub async fn update_task(
    State(state): State<Arc<AppState>>,
    Path((account_id, task_id)): Path<(i64, i64)>,
    Json(mut card): Json<TaskCard>,
) -> Result<Json<TaskRecord>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let existing = fetch_record(&pool, task_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Task not found".into()))?;
    if card.uid.trim().is_empty() {
        card.uid = if existing.card.uid.trim().is_empty() {
            new_uid()
        } else {
            existing.card.uid.clone()
        };
    }
    card.list_id = Some(resolve_list_id(&pool, card.list_id).await?);
    upsert_task(&pool, Some(task_id), &card).await?;
    let rec = fetch_record(&pool, task_id)
        .await?
        .ok_or_else(|| AppError::BadRequest("Failed to load updated task".into()))?;
    Ok(Json(rec))
}

pub async fn delete_task(
    State(state): State<Arc<AppState>>,
    Path((account_id, task_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    sqlx::query("DELETE FROM tasks WHERE task_id = ?")
        .bind(task_id)
        .execute(&pool)
        .await?;
    Ok(Json(json!({ "status": "success" })))
}

#[derive(Deserialize)]
pub struct ClearBody {
    #[serde(default)]
    pub list: Option<i64>,
}

pub async fn clear_completed(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<ClearBody>,
) -> Result<Json<Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let affected = if let Some(list_id) = body.list {
        sqlx::query("DELETE FROM tasks WHERE completed = 1 AND list_id = ?")
            .bind(list_id)
            .execute(&pool)
            .await?
    } else {
        sqlx::query("DELETE FROM tasks WHERE completed = 1")
            .execute(&pool)
            .await?
    };
    Ok(Json(json!({ "status": "success", "removed": affected.rows_affected() })))
}

// ─────────────────────────── List handlers ───────────────────────────

pub async fn get_lists(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
) -> Result<Json<Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    ensure_default_list(&pool).await?;

    let rows = sqlx::query(
        r#"
        SELECT l.list_id, l.name, l.color, l.is_default,
               (SELECT COUNT(*) FROM tasks t WHERE t.list_id = l.list_id AND t.completed = 0) AS cnt
        FROM task_lists l
        ORDER BY l.is_default DESC, l.sort_order ASC, LOWER(l.name) ASC
        "#,
    )
    .fetch_all(&pool)
    .await?;

    let mut lists: Vec<TaskListSummary> = Vec::new();
    for r in &rows {
        lists.push(TaskListSummary {
            list_id: r.try_get("list_id").unwrap_or_default(),
            name: r.try_get("name").unwrap_or_default(),
            color: r.try_get("color").unwrap_or_default(),
            is_default: r.try_get::<i64, _>("is_default").unwrap_or(0) != 0,
            count: r.try_get("cnt").unwrap_or_default(),
        });
    }

    // Cross-cutting counters used by the sidebar's smart views.
    let now = chrono::Utc::now().timestamp_millis();
    let today_end = (now / DAY_MS) * DAY_MS + DAY_MS;
    let total: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE completed = 0")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    let today: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE completed = 0 AND due_ms IS NOT NULL AND due_ms < ?")
            .bind(today_end)
            .fetch_one(&pool)
            .await
            .unwrap_or(0);
    let starred: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tasks WHERE completed = 0 AND starred = 1")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);

    Ok(Json(json!({
        "lists": lists,
        "total": total,
        "today": today,
        "starred": starred,
    })))
}

#[derive(Deserialize)]
pub struct ListBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub color: Option<String>,
}

pub async fn create_list(
    State(state): State<Arc<AppState>>,
    Path(account_id): Path<i64>,
    Json(body): Json<ListBody>,
) -> Result<Json<TaskListSummary>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let name = body.name.as_deref().map(str::trim).unwrap_or("").to_string();
    if name.is_empty() {
        return Err(AppError::BadRequest("List name is required".into()));
    }
    let color = sanitize_color(body.color.as_deref().unwrap_or("#8e44ad"));
    let res = sqlx::query(
        "INSERT INTO task_lists (name, color, is_default, sort_order) VALUES (?, ?, 0, (SELECT COALESCE(MAX(sort_order), 0) + 1 FROM task_lists))",
    )
    .bind(&name)
    .bind(&color)
    .execute(&pool)
    .await?;
    Ok(Json(TaskListSummary {
        list_id: res.last_insert_rowid(),
        name,
        color,
        is_default: false,
        count: 0,
    }))
}

pub async fn update_list(
    State(state): State<Arc<AppState>>,
    Path((account_id, list_id)): Path<(i64, i64)>,
    Json(body): Json<ListBody>,
) -> Result<Json<Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    if let Some(name) = body.name.as_deref().map(str::trim) {
        if !name.is_empty() {
            sqlx::query("UPDATE task_lists SET name = ? WHERE list_id = ?")
                .bind(name)
                .bind(list_id)
                .execute(&pool)
                .await?;
        }
    }
    if let Some(color) = body.color.as_deref() {
        sqlx::query("UPDATE task_lists SET color = ? WHERE list_id = ?")
            .bind(sanitize_color(color))
            .bind(list_id)
            .execute(&pool)
            .await?;
    }
    Ok(Json(json!({ "status": "success" })))
}

pub async fn delete_list(
    State(state): State<Arc<AppState>>,
    Path((account_id, list_id)): Path<(i64, i64)>,
) -> Result<Json<Value>, AppError> {
    let pool = db::get_user_db_pool(&state, account_id).await?;
    let count: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM task_lists")
        .fetch_one(&pool)
        .await
        .unwrap_or(0);
    if count <= 1 {
        return Err(AppError::BadRequest("You can't delete your only list.".into()));
    }
    sqlx::query("DELETE FROM tasks WHERE list_id = ?")
        .bind(list_id)
        .execute(&pool)
        .await?;
    sqlx::query("DELETE FROM task_lists WHERE list_id = ?")
        .bind(list_id)
        .execute(&pool)
        .await?;
    let has_default: Option<i64> =
        sqlx::query_scalar("SELECT list_id FROM task_lists WHERE is_default = 1 LIMIT 1")
            .fetch_optional(&pool)
            .await?;
    if has_default.is_none() {
        let _ = sqlx::query(
            "UPDATE task_lists SET is_default = 1 WHERE list_id = (SELECT list_id FROM task_lists ORDER BY list_id ASC LIMIT 1)",
        )
        .execute(&pool)
        .await;
    }
    Ok(Json(json!({ "status": "success" })))
}
