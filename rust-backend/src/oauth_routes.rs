//! HTTP handlers for the Gmail (Google OAuth) onboarding flow.
//!
//! The browser never sees any OAuth token: the loopback exchange happens
//! entirely in the backend and only the resulting email / display name are
//! surfaced to the UI. Tokens live in `oauth`'s in-memory state until the
//! account is finalized, at which point the refresh token is persisted in the
//! (SQLCipher-encrypted) accounts table.

use std::sync::Arc;

use axum::{extract::Path, extract::State, http::StatusCode, response::IntoResponse, Json};
use serde::Deserialize;
use serde_json::json;

use crate::db::AppState;
use crate::i18n::tr;
use crate::models::OfflineSetupPayload;
use crate::{imap_client, oauth, offline_routes};

/// GET /api/oauth/google/config — lets the UI know whether to offer Google.
pub async fn google_config() -> impl IntoResponse {
    Json(json!({ "configured": oauth::is_configured() }))
}

/// POST /api/oauth/google/begin — start consent, open the browser.
pub async fn google_begin() -> impl IntoResponse {
    if !oauth::is_configured() {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({
                "configured": false,
                "message": tr("Google sign-in is not configured in this build."),
            })),
        )
            .into_response();
    }

    match oauth::begin_flow().await {
        Ok(begin) => (
            StatusCode::OK,
            Json(json!({
                "configured": true,
                "flow_id": begin.flow_id,
                "auth_url": begin.auth_url,
            })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "configured": true, "message": e })),
        )
            .into_response(),
    }
}

/// GET /api/oauth/google/status/:flow_id — poll for completion.
pub async fn google_status(Path(flow_id): Path<String>) -> impl IntoResponse {
    match oauth::flow_status(&flow_id) {
        oauth::FlowStatus::Pending => {
            (StatusCode::OK, Json(json!({ "status": "pending" }))).into_response()
        }
        oauth::FlowStatus::Ready {
            email,
            display_name,
        } => (
            StatusCode::OK,
            Json(json!({
                "status": "ready",
                "email": email,
                "display_name": display_name,
            })),
        )
            .into_response(),
        oauth::FlowStatus::Error(message) => (
            StatusCode::OK,
            Json(json!({ "status": "error", "message": message })),
        )
            .into_response(),
        oauth::FlowStatus::Unknown => (
            StatusCode::NOT_FOUND,
            Json(json!({ "status": "unknown", "message": tr("Sign-in session expired.") })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct GooglePreviewBody {
    pub flow_id: String,
}

/// POST /api/oauth/google/mailboxes-preview — folder/label tree for onboarding,
/// fetched over IMAP XOAUTH2 using the just-authorized (not-yet-saved) token.
pub async fn google_mailboxes_preview(Json(body): Json<GooglePreviewBody>) -> impl IntoResponse {
    let Some((email, access_token)) = oauth::peek_ready(&body.flow_id) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({ "status": "error", "message": tr("Sign-in session expired.") })),
        )
            .into_response();
    };

    match imap_client::preview_mailboxes_xoauth2(
        oauth::GMAIL_IMAP_HOST,
        &email,
        &access_token,
        oauth::GMAIL_IMAP_PORT as u16,
        oauth::GMAIL_SSL_MODE,
    )
    .await
    {
        Ok(mailboxes) => {
            let (folders, labels) = split_mailboxes(&mailboxes);
            (
                StatusCode::OK,
                Json(json!({ "mailboxes": mailboxes, "folders": folders, "labels": labels })),
            )
                .into_response()
        }
        Err(err) => (
            StatusCode::BAD_GATEWAY,
            Json(json!({ "status": "error", "message": err })),
        )
            .into_response(),
    }
}

#[derive(Deserialize)]
pub struct GoogleFinalizeBody {
    pub flow_id: String,
    pub language: Option<String>,
    pub font: Option<String>,
    pub theme: Option<String>,
    pub offline: Option<OfflineSetupPayload>,
}

/// POST /api/oauth/google/finalize — create/update the Gmail account.
pub async fn google_finalize(
    State(state): State<Arc<AppState>>,
    Json(body): Json<GoogleFinalizeBody>,
) -> impl IntoResponse {
    let Some(completed) = oauth::take_completed(&body.flow_id) else {
        return (
            StatusCode::CONFLICT,
            Json(json!({
                "status": "error",
                "message": tr("Sign-in session expired. Please sign in with Google again."),
            })),
        )
            .into_response();
    };

    let language = body.language.unwrap_or_else(|| "en".to_string());
    let font = body.font.unwrap_or_else(|| "Arial".to_string());
    let theme = body.theme.unwrap_or_else(|| "SYSTEM".to_string());

    let inner = match state.ensure_ready(true).await {
        Ok(inner) => inner,
        Err(e) => return e.into_response(),
    };

    let email = completed.email.trim().to_string();
    let display_name = completed.display_name.trim().to_string();

    let existing: Result<Option<i64>, _> =
        sqlx::query_scalar("SELECT account_id FROM accounts WHERE email_address = ?")
            .bind(&email)
            .fetch_optional(&inner.general_pool)
            .await;
    let existing = match existing {
        Ok(v) => v,
        Err(e) => {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "status": "error", "message": format!("Database error: {e}") })),
            )
                .into_response()
        }
    };

    let account_id: i64 = if let Some(id) = existing {
        let res = sqlx::query(
            r#"
            UPDATE accounts
            SET display_name = ?, provider_type = ?, imap_host = ?, imap_port = ?,
                smtp_host = ?, smtp_port = ?, language = ?, theme = ?, font = ?,
                auth_token = ?, ssl_mode = ?
            WHERE account_id = ?
            "#,
        )
        .bind(&display_name)
        .bind(oauth::PROVIDER_GMAIL)
        .bind(oauth::GMAIL_IMAP_HOST)
        .bind(oauth::GMAIL_IMAP_PORT)
        .bind(oauth::GMAIL_SMTP_HOST)
        .bind(oauth::GMAIL_SMTP_PORT)
        .bind(&language)
        .bind(&theme)
        .bind(&font)
        .bind(&completed.refresh_token)
        .bind(oauth::GMAIL_SSL_MODE)
        .bind(id)
        .execute(&inner.general_pool)
        .await;
        if let Err(e) = res {
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({ "status": "error", "message": format!("Database error: {e}") })),
            )
                .into_response();
        }
        id
    } else {
        let res = sqlx::query(
            r#"
            INSERT INTO accounts
                (email_address, display_name, provider_type,
                 imap_host, imap_port, smtp_host, smtp_port, language, theme, font,
                 auth_token, ssl_mode)
            VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
            "#,
        )
        .bind(&email)
        .bind(&display_name)
        .bind(oauth::PROVIDER_GMAIL)
        .bind(oauth::GMAIL_IMAP_HOST)
        .bind(oauth::GMAIL_IMAP_PORT)
        .bind(oauth::GMAIL_SMTP_HOST)
        .bind(oauth::GMAIL_SMTP_PORT)
        .bind(&language)
        .bind(&theme)
        .bind(&font)
        .bind(&completed.refresh_token)
        .bind(oauth::GMAIL_SSL_MODE)
        .execute(&inner.general_pool)
        .await;
        match res {
            Ok(r) => r.last_insert_rowid(),
            Err(e) => {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({ "status": "error", "message": format!("Database error: {e}") })),
                )
                    .into_response()
            }
        }
    };

    // Seed the token cache so the first sync avoids a refresh round-trip.
    oauth::seed_account_token(
        account_id,
        completed.access_token,
        completed.access_expires_at,
    );

    if let Err(e) = crate::db::get_user_db_pool(&state, account_id).await {
        return e.into_response();
    }
    if let Err(e) = offline_routes::save_offline_setup(&state, account_id, body.offline).await {
        return e.into_response();
    }
    offline_routes::spawn_initial_sync(state.clone(), account_id);

    (
        StatusCode::OK,
        Json(json!({
            "status": "success",
            "account_id": account_id,
            "email_address": email,
            "display_name": display_name,
            "provider_type": oauth::PROVIDER_GMAIL,
            "imap_host": oauth::GMAIL_IMAP_HOST,
            "imap_port": oauth::GMAIL_IMAP_PORT,
            "smtp_host": oauth::GMAIL_SMTP_HOST,
            "smtp_port": oauth::GMAIL_SMTP_PORT,
            "ssl_mode": oauth::GMAIL_SSL_MODE,
            "language": language,
            "font": font,
            "theme": theme,
        })),
    )
        .into_response()
}

fn split_mailboxes(mailboxes: &[String]) -> (Vec<String>, Vec<String>) {
    let mut folders = Vec::new();
    let mut labels = Vec::new();
    for mailbox in mailboxes {
        let lower = mailbox.to_lowercase();
        if lower.starts_with("labels/") || lower.starts_with("[labels]/") {
            labels.push(mailbox.clone());
        } else {
            folders.push(mailbox.clone());
        }
    }
    (folders, labels)
}
