//! Google OAuth 2.0 (installed-app / loopback) support for Gmail.
//!
//! Gmail is accessed through the *existing* IMAP/SMTP pipeline using the
//! SASL `XOAUTH2` mechanism, so almost all of the mail engine is reused. This
//! module owns only the OAuth bits:
//!   * the PKCE loopback authorization flow (opens the system browser, catches
//!     the redirect on `127.0.0.1:<ephemeral port>`),
//!   * exchanging the authorization code for tokens and refreshing them,
//!   * a small in-memory cache of short-lived access tokens keyed by account.
//!
//! ## Credentials
//! The app ships with a default OAuth client so Google sign-in works out of the
//! box for end-users. This is expected for an *installed / desktop* app: Google
//! documents that such clients are not confidential — the "secret" necessarily
//! ships inside the distributed binary. Resolution order for each value:
//!   1. the `GOOGLE_CLIENT_ID` / `GOOGLE_CLIENT_SECRET` environment variable
//!      (loaded from `.env` in development),
//!   2. a value baked in at build time via `option_env!`,
//!   3. the shipped `DEFAULT_CLIENT_*` constant below.
//! A fork can override the defaults with its own client via (1) or (2) without
//! touching this file.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64URL;
use base64::Engine as _;
use once_cell::sync::Lazy;
use rand::RngCore;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use sqlx::{Row, SqlitePool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, warn};

const AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";
const USERINFO_ENDPOINT: &str = "https://openidconnect.googleapis.com/v1/userinfo";

/// Full mailbox access (required for IMAP/SMTP), Calendar and Contacts (for the
/// optional one-way sync in `google_sync`), plus identity to learn the address we
/// just authorized. Existing accounts keep their old (mail-only) refresh token
/// until the user signs in again — Calendar/Contacts sync simply reports that a
/// reconnect is needed until then.
const SCOPES: &str = "https://mail.google.com/ https://www.googleapis.com/auth/calendar https://www.googleapis.com/auth/contacts openid email profile";

/// Gmail connection defaults used when creating an account.
pub const GMAIL_IMAP_HOST: &str = "imap.gmail.com";
pub const GMAIL_IMAP_PORT: i64 = 993;
pub const GMAIL_SMTP_HOST: &str = "smtp.gmail.com";
pub const GMAIL_SMTP_PORT: i64 = 465;
pub const GMAIL_SSL_MODE: &str = "SSL";
pub const PROVIDER_GMAIL: &str = "gmail";

/// How long a not-yet-completed authorization flow is retained.
const FLOW_TTL: Duration = Duration::from_secs(15 * 60);
/// How long we wait for the user to complete consent in the browser.
const CONSENT_TIMEOUT: Duration = Duration::from_secs(5 * 60);
/// Refresh access tokens this long before their real expiry.
const TOKEN_SKEW: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------
// Credential loading
// ---------------------------------------------------------------------------

/// Default OAuth client shipped with the app (installed/desktop client — not
/// confidential). Override via env or `option_env!` for forks; see module docs.
const DEFAULT_CLIENT_ID: &str =
    "531714045390-4d8bq7fcj4ko4vkg9i81e49okhhhgrph.apps.googleusercontent.com";
const DEFAULT_CLIENT_SECRET: &str = "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";

fn env_or_baked(
    runtime: &str,
    baked: Option<&'static str>,
    default: &'static str,
) -> Option<String> {
    std::env::var(runtime)
        .ok()
        .filter(|s| !s.trim().is_empty())
        .or_else(|| baked.map(|s| s.to_string()))
        .filter(|s| !s.trim().is_empty())
        .or_else(|| Some(default.to_string()))
        .filter(|s| !s.trim().is_empty())
}

fn client_id() -> Option<String> {
    env_or_baked(
        "GOOGLE_CLIENT_ID",
        option_env!("GOOGLE_CLIENT_ID"),
        DEFAULT_CLIENT_ID,
    )
}

fn client_secret() -> Option<String> {
    env_or_baked(
        "GOOGLE_CLIENT_SECRET",
        option_env!("GOOGLE_CLIENT_SECRET"),
        DEFAULT_CLIENT_SECRET,
    )
}

/// Whether Google sign-in is available in this build/environment.
pub fn is_configured() -> bool {
    client_id().is_some() && client_secret().is_some()
}

// ---------------------------------------------------------------------------
// Flow + token state
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct ReadyTokens {
    email: String,
    display_name: String,
    refresh_token: String,
    access_token: String,
    /// Instant at which `access_token` stops being valid.
    access_expires_at: Instant,
}

enum FlowState {
    Pending,
    Ready(ReadyTokens),
    Error(String),
}

struct PendingFlow {
    state: FlowState,
    created_at: Instant,
}

static FLOWS: Lazy<Mutex<HashMap<String, PendingFlow>>> = Lazy::new(|| Mutex::new(HashMap::new()));

#[derive(Clone)]
struct CachedToken {
    access_token: String,
    expires_at: Instant,
}

static ACCOUNT_TOKENS: Lazy<Mutex<HashMap<i64, CachedToken>>> =
    Lazy::new(|| Mutex::new(HashMap::new()));

/// Public snapshot of a flow's progress for the frontend.
pub enum FlowStatus {
    Pending,
    Ready { email: String, display_name: String },
    Error(String),
    Unknown,
}

fn prune_flows(map: &mut HashMap<String, PendingFlow>) {
    let now = Instant::now();
    map.retain(|_, f| now.duration_since(f.created_at) < FLOW_TTL);
}

fn set_flow(flow_id: &str, state: FlowState) {
    let mut map = FLOWS.lock().unwrap();
    if let Some(f) = map.get_mut(flow_id) {
        f.state = state;
    }
}

// ---------------------------------------------------------------------------
// Token response shapes
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
    #[serde(default)]
    expires_in: Option<u64>,
}

#[derive(Deserialize)]
struct UserInfo {
    #[serde(default)]
    email: Option<String>,
    #[serde(default)]
    name: Option<String>,
}

// ---------------------------------------------------------------------------
// PKCE helpers
// ---------------------------------------------------------------------------

fn random_b64url(bytes: usize) -> String {
    let mut buf = vec![0u8; bytes];
    rand::thread_rng().fill_bytes(&mut buf);
    B64URL.encode(buf)
}

fn code_challenge_s256(verifier: &str) -> String {
    let digest = Sha256::digest(verifier.as_bytes());
    B64URL.encode(digest)
}

// ---------------------------------------------------------------------------
// Public API: begin / status / finalize consumption
// ---------------------------------------------------------------------------

pub struct BeginResult {
    pub flow_id: String,
    pub auth_url: String,
}

/// Start an authorization flow: bind a loopback listener, build the consent
/// URL, open the system browser, and spawn a task that completes the exchange
/// when Google redirects back. Returns immediately with the flow id and URL.
pub async fn begin_flow() -> Result<BeginResult, String> {
    let client_id = client_id().ok_or_else(|| "Google sign-in is not configured".to_string())?;

    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|e| format!("Could not open loopback listener: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Could not read loopback address: {e}"))?
        .port();
    let redirect_uri = format!("http://127.0.0.1:{port}");

    let flow_id = random_b64url(18);
    let csrf_state = random_b64url(18);
    let verifier = random_b64url(48);
    let challenge = code_challenge_s256(&verifier);

    let auth_url = reqwest::Url::parse_with_params(
        AUTH_ENDPOINT,
        &[
            ("client_id", client_id.as_str()),
            ("redirect_uri", redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", SCOPES),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            ("state", csrf_state.as_str()),
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map_err(|e| format!("Could not build authorization URL: {e}"))?
    .to_string();

    {
        let mut map = FLOWS.lock().unwrap();
        prune_flows(&mut map);
        map.insert(
            flow_id.clone(),
            PendingFlow {
                state: FlowState::Pending,
                created_at: Instant::now(),
            },
        );
    }

    // Open the browser. If this fails the frontend still shows the URL.
    if let Err(e) = open::that_detached(&auth_url) {
        warn!("Could not open system browser for Google sign-in: {e}");
    }

    // Spawn the redirect catcher.
    let flow_id_task = flow_id.clone();
    tokio::spawn(async move {
        let result = wait_for_redirect_and_exchange(
            listener,
            &client_id,
            &redirect_uri,
            &verifier,
            &csrf_state,
        )
        .await;
        match result {
            Ok(tokens) => set_flow(&flow_id_task, FlowState::Ready(tokens)),
            Err(e) => {
                warn!("Google OAuth flow failed: {e}");
                set_flow(&flow_id_task, FlowState::Error(e));
            }
        }
    });

    Ok(BeginResult { flow_id, auth_url })
}

/// Non-consuming progress check for the frontend.
pub fn flow_status(flow_id: &str) -> FlowStatus {
    let map = FLOWS.lock().unwrap();
    match map.get(flow_id) {
        None => FlowStatus::Unknown,
        Some(f) => match &f.state {
            FlowState::Pending => FlowStatus::Pending,
            FlowState::Error(e) => FlowStatus::Error(e.clone()),
            FlowState::Ready(t) => FlowStatus::Ready {
                email: t.email.clone(),
                display_name: t.display_name.clone(),
            },
        },
    }
}

/// Read a ready flow's access token without consuming it (used for the
/// mailbox preview during onboarding, before the account exists).
pub fn peek_ready(flow_id: &str) -> Option<(String, String)> {
    let map = FLOWS.lock().unwrap();
    match map.get(flow_id) {
        Some(PendingFlow {
            state: FlowState::Ready(t),
            ..
        }) => Some((t.email.clone(), t.access_token.clone())),
        _ => None,
    }
}

/// Details captured by a completed flow, consumed exactly once at finalize.
pub struct CompletedFlow {
    pub email: String,
    pub display_name: String,
    pub refresh_token: String,
    pub access_token: String,
    pub access_expires_at: Instant,
}

/// Consume a ready flow so the account can be created.
pub fn take_completed(flow_id: &str) -> Option<CompletedFlow> {
    let mut map = FLOWS.lock().unwrap();
    let is_ready = matches!(
        map.get(flow_id),
        Some(PendingFlow {
            state: FlowState::Ready(_),
            ..
        })
    );
    if !is_ready {
        return None;
    }
    let removed = map.remove(flow_id)?;
    if let FlowState::Ready(t) = removed.state {
        Some(CompletedFlow {
            email: t.email,
            display_name: t.display_name,
            refresh_token: t.refresh_token,
            access_token: t.access_token,
            access_expires_at: t.access_expires_at,
        })
    } else {
        None
    }
}

// ---------------------------------------------------------------------------
// Loopback redirect handling + token exchange
// ---------------------------------------------------------------------------

async fn wait_for_redirect_and_exchange(
    listener: TcpListener,
    client_id: &str,
    redirect_uri: &str,
    verifier: &str,
    expected_state: &str,
) -> Result<ReadyTokens, String> {
    let (code, got_state) = tokio::time::timeout(CONSENT_TIMEOUT, accept_redirect(listener))
        .await
        .map_err(|_| "Timed out waiting for Google sign-in".to_string())??;

    if got_state != expected_state {
        return Err("Sign-in state mismatch (possible CSRF); aborted".to_string());
    }

    let client_secret =
        client_secret().ok_or_else(|| "Google sign-in is not configured".to_string())?;

    let http = reqwest::Client::new();
    let token: TokenResponse = http
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("code", code.as_str()),
            ("client_id", client_id),
            ("client_secret", client_secret.as_str()),
            ("redirect_uri", redirect_uri),
            ("grant_type", "authorization_code"),
            ("code_verifier", verifier),
        ])
        .send()
        .await
        .map_err(|e| format!("Token request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Token request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Could not parse token response: {e}"))?;

    let refresh_token = token
        .refresh_token
        .clone()
        .ok_or_else(|| "Google did not return a refresh token".to_string())?;

    let userinfo: UserInfo = http
        .get(USERINFO_ENDPOINT)
        .bearer_auth(&token.access_token)
        .send()
        .await
        .map_err(|e| format!("Userinfo request failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Userinfo request rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Could not parse userinfo: {e}"))?;

    let email = userinfo
        .email
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Google account has no email address".to_string())?;
    let display_name = userinfo.name.unwrap_or_else(|| email.clone());

    info!("Google OAuth completed for {}", email);

    Ok(ReadyTokens {
        email,
        display_name,
        refresh_token,
        access_expires_at: expiry_from(token.expires_in),
        access_token: token.access_token,
    })
}

/// Accept a single loopback connection and pull `code` + `state` from the
/// request line, replying with a small confirmation page.
async fn accept_redirect(listener: TcpListener) -> Result<(String, String), String> {
    loop {
        let (mut stream, _) = listener
            .accept()
            .await
            .map_err(|e| format!("Loopback accept failed: {e}"))?;

        let mut buf = [0u8; 8192];
        let n = stream
            .read(&mut buf)
            .await
            .map_err(|e| format!("Loopback read failed: {e}"))?;
        let request = String::from_utf8_lossy(&buf[..n]);

        // First line looks like: GET /?code=...&state=... HTTP/1.1
        let path = request
            .lines()
            .next()
            .and_then(|line| line.split_whitespace().nth(1))
            .unwrap_or("/");

        // Favicon and other stray requests: ignore and keep waiting.
        if !path.contains("code=") && !path.contains("error=") {
            let _ = respond(&mut stream, "Waiting for Google sign-in…").await;
            continue;
        }

        let parsed = reqwest::Url::parse(&format!("http://127.0.0.1{path}"))
            .map_err(|e| format!("Could not parse redirect: {e}"))?;

        if let Some(err) = parsed
            .query_pairs()
            .find(|(k, _)| k == "error")
            .map(|(_, v)| v.into_owned())
        {
            let _ = respond(
                &mut stream,
                "Sign-in was cancelled. You can close this tab.",
            )
            .await;
            return Err(format!("Google returned an error: {err}"));
        }

        let mut code = None;
        let mut state = None;
        for (k, v) in parsed.query_pairs() {
            match k.as_ref() {
                "code" => code = Some(v.into_owned()),
                "state" => state = Some(v.into_owned()),
                _ => {}
            }
        }

        match (code, state) {
            (Some(code), Some(state)) => {
                let _ = respond(
                    &mut stream,
                    "Signed in to Güvercin. You can close this tab and return to the app.",
                )
                .await;
                return Ok((code, state));
            }
            _ => {
                let _ = respond(&mut stream, "Missing authorization code.").await;
                return Err("Redirect missing authorization code".to_string());
            }
        }
    }
}

async fn respond(stream: &mut tokio::net::TcpStream, message: &str) -> std::io::Result<()> {
    let body = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\">\
         <title>Güvercin</title></head>\
         <body style=\"font-family:system-ui,sans-serif;text-align:center;padding:48px;color:#333\">\
         <h2>{message}</h2></body></html>"
    );
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    stream.write_all(response.as_bytes()).await?;
    stream.flush().await
}

fn expiry_from(expires_in: Option<u64>) -> Instant {
    let secs = expires_in.unwrap_or(3600);
    Instant::now() + Duration::from_secs(secs)
}

// ---------------------------------------------------------------------------
// Per-account access tokens (used by the IMAP/SMTP layers at runtime)
// ---------------------------------------------------------------------------

/// Seed the cache with the access token obtained during onboarding so the
/// first connection after account creation doesn't need a refresh round-trip.
pub fn seed_account_token(account_id: i64, access_token: String, expires_at: Instant) {
    ACCOUNT_TOKENS.lock().unwrap().insert(
        account_id,
        CachedToken {
            access_token,
            expires_at,
        },
    );
}

/// Drop any cached token for an account (e.g. on deletion).
pub fn invalidate_account_token(account_id: i64) {
    ACCOUNT_TOKENS.lock().unwrap().remove(&account_id);
}

/// Return a currently-valid access token for a Gmail account, refreshing from
/// the stored refresh token when the cached one is missing or near expiry.
pub async fn access_token_for_account(
    pool: &SqlitePool,
    account_id: i64,
) -> Result<String, String> {
    if let Some(tok) = ACCOUNT_TOKENS.lock().unwrap().get(&account_id) {
        if tok.expires_at.saturating_duration_since(Instant::now()) > TOKEN_SKEW {
            return Ok(tok.access_token.clone());
        }
    }

    let row = sqlx::query("SELECT auth_token FROM accounts WHERE account_id = ?")
        .bind(account_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("Database error: {e}"))?
        .ok_or_else(|| "Account not found".to_string())?;
    let refresh_token: String = row
        .try_get::<Option<String>, _>("auth_token")
        .ok()
        .flatten()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "No Google refresh token stored for this account".to_string())?;

    let (access_token, expires_at) = refresh_access_token(&refresh_token).await?;
    seed_account_token(account_id, access_token.clone(), expires_at);
    Ok(access_token)
}

async fn refresh_access_token(refresh_token: &str) -> Result<(String, Instant), String> {
    let client_id = client_id().ok_or_else(|| "Google sign-in is not configured".to_string())?;
    let client_secret =
        client_secret().ok_or_else(|| "Google sign-in is not configured".to_string())?;

    let token: TokenResponse = reqwest::Client::new()
        .post(TOKEN_ENDPOINT)
        .form(&[
            ("client_id", client_id.as_str()),
            ("client_secret", client_secret.as_str()),
            ("refresh_token", refresh_token),
            ("grant_type", "refresh_token"),
        ])
        .send()
        .await
        .map_err(|e| format!("Token refresh failed: {e}"))?
        .error_for_status()
        .map_err(|e| format!("Token refresh rejected: {e}"))?
        .json()
        .await
        .map_err(|e| format!("Could not parse refresh response: {e}"))?;

    Ok((token.access_token, expiry_from(token.expires_in)))
}
