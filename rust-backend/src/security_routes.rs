use std::{path::PathBuf, sync::Arc};

use axum::{
    extract::{Json, State},
    http::StatusCode,
    response::IntoResponse,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{db::AppState, error::AppError};

/* ─── Persistent settings file ──────────────────────────────────── */

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecuritySettings {
    /// Always true (encryption is always on). Stored for UI parity.
    #[serde(default = "default_true")]
    pub data_encrypted: bool,
}

fn default_true() -> bool {
    true
}

impl Default for SecuritySettings {
    fn default() -> Self {
        Self {
            data_encrypted: true,
        }
    }
}

fn settings_path(state: &AppState) -> PathBuf {
    state.databases_dir.join("security_settings.json")
}

async fn load_settings(state: &AppState) -> SecuritySettings {
    let path = settings_path(state);
    match tokio::fs::read_to_string(&path).await {
        Ok(s) => serde_json::from_str(&s).unwrap_or_default(),
        Err(_) => SecuritySettings::default(),
    }
}

async fn save_settings(state: &AppState, settings: &SecuritySettings) -> std::io::Result<()> {
    let path = settings_path(state);
    let json = serde_json::to_string_pretty(settings).unwrap_or_default();
    tokio::fs::write(&path, json).await
}

/* ─── Handlers ───────────────────────────────────────────────────── */

pub async fn get_security_settings(State(state): State<Arc<AppState>>) -> Json<SecuritySettings> {
    Json(load_settings(&state).await)
}

pub async fn put_security_settings(
    State(state): State<Arc<AppState>>,
    Json(_body): Json<SecuritySettings>,
) -> impl IntoResponse {
    // Normalize: data_encrypted is always forced true
    let to_save = SecuritySettings {
        data_encrypted: true,
    };

    match save_settings(&state, &to_save).await {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({ "status": "ok", "settings": to_save })),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": e.to_string() })),
        )
            .into_response(),
    }
}



/* ─── AppError compat ────────────────────────────────────────────── */
impl From<AppError> for (StatusCode, Json<serde_json::Value>) {
    fn from(e: AppError) -> Self {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({ "status": "error", "message": e.to_string() })),
        )
    }
}
