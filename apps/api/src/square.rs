use std::collections::{HashMap, HashSet};

use aes_gcm::{
    Aes256Gcm, Nonce,
    aead::{Aead, KeyInit},
};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Redirect, Response},
};
use base64::{
    Engine,
    engine::general_purpose::{STANDARD as B64, URL_SAFE_NO_PAD},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Duration, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error};

const PROVIDER: &str = "square";
const SQUARE_VERSION: &str = "2025-01-23";
const MENU_NAME_MAX: usize = 50;
const MENU_CATEGORY_MAX: usize = 20;
const INITIAL_SALES_DAYS: i64 = 90;

#[derive(Clone)]
pub(crate) struct SquareConfig {
    application_id: String,
    application_secret: String,
    environment: String,
    redirect_uri: String,
    token_key: [u8; 32],
    web_origin: String,
}

impl SquareConfig {
    pub(crate) fn from_env(web_origin: &str) -> Option<Self> {
        let application_id = std::env::var("SQUARE_APPLICATION_ID").ok()?;
        let application_secret = std::env::var("SQUARE_APPLICATION_SECRET").ok()?;
        let redirect_uri = std::env::var("SQUARE_REDIRECT_URI").ok()?;
        if application_id.trim().is_empty()
            || application_secret.trim().is_empty()
            || redirect_uri.trim().is_empty()
        {
            return None;
        }
        let environment = std::env::var("SQUARE_ENVIRONMENT").unwrap_or_else(|_| "sandbox".into());
        let token_key = token_key_from_env().ok()?;
        Some(Self {
            application_id: application_id.trim().to_owned(),
            application_secret: application_secret.trim().to_owned(),
            environment: environment.trim().to_owned(),
            redirect_uri: redirect_uri.trim().to_owned(),
            token_key,
            web_origin: web_origin.trim_end_matches('/').to_owned(),
        })
    }

    fn oauth_base(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("production") {
            "https://connect.squareup.com"
        } else {
            "https://connect.squareupsandbox.com"
        }
    }

    fn api_base(&self) -> String {
        // Sandbox-only override so integration tests can point the connector
        // at a local mock server. Production always talks to Square directly.
        if !self.environment.eq_ignore_ascii_case("production")
            && let Ok(base) = std::env::var("SQUARE_API_BASE_URL")
        {
            let base = base.trim().trim_end_matches('/').to_owned();
            if !base.is_empty() {
                return base;
            }
        }
        if self.environment.eq_ignore_ascii_case("production") {
            "https://connect.squareup.com".to_owned()
        } else {
            "https://connect.squareupsandbox.com".to_owned()
        }
    }

    fn scopes(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("production") {
            "ITEMS_READ ORDERS_READ MERCHANT_PROFILE_READ"
        } else {
            // Sandbox keeps write scopes for local seed tooling. Production sync is read-only.
            "ITEMS_READ ITEMS_WRITE ORDERS_READ ORDERS_WRITE PAYMENTS_WRITE MERCHANT_PROFILE_READ"
        }
    }

    #[cfg(test)]
    pub(crate) fn test_config() -> Self {
        Self {
            application_id: "release-test-app-id".into(),
            application_secret: "release-test-app-secret".into(),
            environment: "sandbox".into(),
            redirect_uri: "http://localhost:8080/v1/connections/square/callback".into(),
            // Same derivation as token_key_from_env(), without touching process env.
            token_key: {
                let hash = Sha256::digest(b"release-test-token-key");
                let mut key = [0u8; 32];
                key.copy_from_slice(&hash);
                key
            },
            web_origin: "http://localhost:5173".into(),
        }
    }
}

fn token_key_from_env() -> Result<[u8; 32], ()> {
    let raw = std::env::var("CONNECTIONS_TOKEN_KEY").map_err(|_| ())?;
    if raw.trim().is_empty() {
        return Err(());
    }
    let hash = Sha256::digest(raw.as_bytes());
    let mut key = [0u8; 32];
    key.copy_from_slice(&hash);
    Ok(key)
}

pub(crate) fn encrypt_secret(key: &[u8; 32], plain: &str) -> Result<String, ApiError> {
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| encrypt_error())?;
    let mut nonce_bytes = [0u8; 12];
    getrandom::getrandom(&mut nonce_bytes).map_err(|_| encrypt_error())?;
    let nonce = Nonce::from_slice(&nonce_bytes);
    let encrypted = cipher
        .encrypt(nonce, plain.as_bytes())
        .map_err(|_| encrypt_error())?;
    let mut out = nonce_bytes.to_vec();
    out.extend(encrypted);
    Ok(B64.encode(out))
}

fn decrypt_secret(key: &[u8; 32], encoded: &str) -> Result<String, ApiError> {
    let bytes = B64.decode(encoded).map_err(|_| encrypt_error())?;
    if bytes.len() < 13 {
        return Err(encrypt_error());
    }
    let (nonce_bytes, ciphertext) = bytes.split_at(12);
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| encrypt_error())?;
    let nonce = Nonce::from_slice(nonce_bytes);
    let plain = cipher
        .decrypt(nonce, ciphertext)
        .map_err(|_| encrypt_error())?;
    String::from_utf8(plain).map_err(|_| encrypt_error())
}

fn encrypt_error() -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "We couldn't secure the connection. Please try again.",
    )
}

fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .expect("static Square HTTP client configuration")
}

#[derive(sqlx::FromRow)]
pub(crate) struct Member {
    pub(crate) restaurant_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) role: String,
    #[allow(dead_code)]
    timezone: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionView {
    id: Uuid,
    pub(crate) provider: String,
    pub(crate) status: String,
    pub(crate) external_merchant_id: Option<String>,
    pub(crate) external_location_id: Option<String>,
    pub(crate) last_sync_at: Option<DateTime<Utc>>,
    pub(crate) last_success_at: Option<DateTime<Utc>>,
    pub(crate) last_error: Option<String>,
    /// Stats JSON from the newest succeeded run, so the UI can surface
    /// unmatched order lines instead of silently under-counting sales.
    pub(crate) last_sync_stats: Option<Value>,
    pub(crate) created_at: DateTime<Utc>,
    pub(crate) updated_at: DateTime<Utc>,
    pub(crate) configured: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AuthorizeResponse {
    url: String,
    configured: bool,
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    code: Option<String>,
    state: Option<String>,
    error: Option<String>,
    #[allow(dead_code)]
    error_description: Option<String>,
}

pub(crate) async fn member(state: &AppState, headers: &HeaderMap) -> Result<Member, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as(
        "SELECT m.restaurant_id,u.id user_id,m.role,r.timezone
         FROM users u
         JOIN restaurant_memberships m ON m.user_id=u.id
         JOIN restaurants r ON r.id=m.restaurant_id
         WHERE u.auth_subject=$1",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "A restaurant membership is required.",
    ))
}

pub(crate) fn manager(m: &Member) -> Result<(), ApiError> {
    if matches!(m.role.as_str(), "owner" | "manager") {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Owner or manager access is required.",
        ))
    }
}

fn require_config(state: &AppState) -> Result<&SquareConfig, ApiError> {
    state.square.as_ref().ok_or(ApiError(
        StatusCode::SERVICE_UNAVAILABLE,
        "Square connect is not configured on this server.",
    ))
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<ConnectionView>>, ApiError> {
    connections_list(&state, &headers, PROVIDER, state.square.is_some()).await
}

pub(crate) async fn connections_list(
    state: &AppState,
    headers: &HeaderMap,
    provider: &str,
    configured: bool,
) -> Result<Json<Vec<ConnectionView>>, ApiError> {
    let m = member(state, headers).await?;
    let mut rows = sqlx::query_as::<_, ConnectionView>(
        "SELECT connection.id,connection.provider,connection.status,
                connection.external_merchant_id,connection.external_location_id,
                connection.last_sync_at,connection.last_success_at,connection.last_error,
                (SELECT run.stats FROM source_sync_runs run
                  WHERE run.connection_id=connection.id AND run.status='succeeded'
                  ORDER BY run.finished_at DESC LIMIT 1) AS last_sync_stats,
                connection.created_at,connection.updated_at,
                FALSE AS configured
         FROM source_connections connection
         WHERE connection.restaurant_id=$1 AND connection.provider=$2 AND connection.status<>'disconnected'
         ORDER BY connection.updated_at DESC",
    )
    .bind(m.restaurant_id)
    .bind(provider)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    for row in &mut rows {
        row.configured = configured;
    }
    Ok(Json(rows))
}

pub(crate) async fn square_status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    Ok(Json(json!({
        "configured": state.square.is_some(),
        "provider": PROVIDER,
    })))
}

pub(crate) async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AuthorizeResponse>, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    let config = require_config(&state)?;
    let state_token = URL_SAFE_NO_PAD.encode(Uuid::now_v7().as_bytes());
    let expires = Utc::now() + Duration::minutes(15);
    sqlx::query(
        "INSERT INTO oauth_states(state,restaurant_id,user_id,provider,expires_at)
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(&state_token)
    .bind(m.restaurant_id)
    .bind(m.user_id)
    .bind(PROVIDER)
    .bind(expires)
    .execute(&state.pool)
    .await
    .map_err(database_error)?;
    let url = format!(
        "{}/oauth2/authorize?client_id={}&scope={}&session=false&state={}&redirect_uri={}",
        config.oauth_base(),
        urlencoding_encode(&config.application_id),
        urlencoding_encode(config.scopes()),
        urlencoding_encode(&state_token),
        urlencoding_encode(&config.redirect_uri),
    );
    Ok(Json(AuthorizeResponse {
        url,
        configured: true,
    }))
}

pub(crate) fn urlencoding_encode(value: &str) -> String {
    let mut out = String::with_capacity(value.len() * 3);
    for byte in value.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

pub(crate) async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    match complete_callback(&state, query).await {
        Ok(path) => Redirect::temporary(&path).into_response(),
        Err(message) => {
            let origin = state
                .square
                .as_ref()
                .map(|c| c.web_origin.as_str())
                .unwrap_or("http://localhost:5173");
            let path = format!(
                "{origin}/sources?square=error&message={}",
                urlencoding_encode(message)
            );
            Redirect::temporary(&path).into_response()
        }
    }
}

async fn complete_callback(state: &AppState, query: CallbackQuery) -> Result<String, &'static str> {
    let config = state.square.as_ref().ok_or("Square is not configured.")?;
    if query.error.is_some() {
        return Err("Square authorization was denied or failed.");
    }
    let code = query
        .code
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or("Missing authorization code.")?;
    let state_token = query
        .state
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or("Missing OAuth state.")?;
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT restaurant_id,user_id FROM oauth_states
         WHERE state=$1 AND provider=$2 AND expires_at > NOW()",
    )
    .bind(state_token)
    .bind(PROVIDER)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| "We couldn't verify the connection request.")?
    .ok_or("This connection link expired. Try Connect Square again.")?;
    let (restaurant_id, user_id) = row;
    let token = obtain_token(config, code)
        .await
        .map_err(|_| "Square did not return access tokens. Try again.")?;
    let access_enc = encrypt_secret(&config.token_key, &token.access_token)
        .map_err(|_| "We couldn't secure the Square tokens.")?;
    let refresh_enc = encrypt_secret(&config.token_key, &token.refresh_token)
        .map_err(|_| "We couldn't secure the Square tokens.")?;
    let expires_at = Utc::now() + Duration::seconds(token.expires_in.unwrap_or(30 * 24 * 3600));
    let merchant_id = token.merchant_id.clone();
    let location_id = fetch_primary_location(config, &token.access_token)
        .await
        .ok()
        .flatten();
    let connection_id = Uuid::now_v7();
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| "We couldn't save the Square connection.")?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| "We couldn't save the Square connection.")?;
    let consumed = sqlx::query(
        "DELETE FROM oauth_states
         WHERE state=$1 AND provider=$2 AND restaurant_id=$3 AND user_id=$4
           AND expires_at > NOW()",
    )
    .bind(state_token)
    .bind(PROVIDER)
    .bind(restaurant_id)
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| "We couldn't verify the connection request.")?
    .rows_affected();
    if consumed != 1 {
        return Err("This connection link expired. Try Connect Square again.");
    }
    let selected = sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*)=2 FROM restaurant_setup_streams
         WHERE restaurant_id=$1 AND stream IN ('menu','sales')
           AND method='connector' AND connector_provider='square'",
    )
    .bind(restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| "We couldn't verify the connection request.")?;
    if !selected {
        return Err("Square is no longer selected.");
    }
    let existing_merchant = sqlx::query_scalar::<_, Option<String>>(
        "SELECT external_merchant_id FROM source_connections
         WHERE restaurant_id=$1 AND provider=$2",
    )
    .bind(restaurant_id)
    .bind(PROVIDER)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| "We couldn't verify the Square account.")?
    .flatten();
    if existing_merchant.is_some() && existing_merchant.as_deref() != merchant_id.as_deref() {
        return Err("This restaurant is linked to a different Square account.");
    }
    let connection_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO source_connections
         (id,restaurant_id,provider,status,external_merchant_id,external_location_id,
          access_token_encrypted,refresh_token_encrypted,access_token_expires_at,scopes,created_by)
         VALUES($1,$2,$3,'connected',$4,$5,$6,$7,$8,$9,$10)
         ON CONFLICT (restaurant_id,provider) DO UPDATE SET
           status='connected',
           external_merchant_id=EXCLUDED.external_merchant_id,
           external_location_id=EXCLUDED.external_location_id,
           access_token_encrypted=EXCLUDED.access_token_encrypted,
           refresh_token_encrypted=EXCLUDED.refresh_token_encrypted,
           access_token_expires_at=EXCLUDED.access_token_expires_at,
           scopes=EXCLUDED.scopes,
           last_error=NULL,
           updated_at=NOW()
         RETURNING id",
    )
    .bind(connection_id)
    .bind(restaurant_id)
    .bind(PROVIDER)
    .bind(&merchant_id)
    .bind(&location_id)
    .bind(&access_enc)
    .bind(&refresh_enc)
    .bind(expires_at)
    .bind(config.scopes())
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| "We couldn't save the Square connection.")?;
    sqlx::query(
        "INSERT INTO source_sync_runs(id,connection_id,restaurant_id,kind,status)
         VALUES($1,$2,$3,'full','queued')",
    )
    .bind(Uuid::now_v7())
    .bind(connection_id)
    .bind(restaurant_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| "We couldn't start the Square sync.")?;
    tx.commit()
        .await
        .map_err(|_| "We couldn't save the Square connection.")?;
    Ok(format!("{}/sources?square=connected", config.web_origin))
}

#[derive(Deserialize)]
struct TokenResponse {
    access_token: String,
    refresh_token: String,
    expires_in: Option<i64>,
    merchant_id: Option<String>,
}

async fn obtain_token(config: &SquareConfig, code: &str) -> Result<TokenResponse, ()> {
    let client = http_client();
    let response = client
        .post(format!("{}/oauth2/token", config.oauth_base()))
        .header(header::CONTENT_TYPE, "application/json")
        .header("Square-Version", SQUARE_VERSION)
        .json(&json!({
            "client_id": config.application_id,
            "client_secret": config.application_secret,
            "code": code,
            "grant_type": "authorization_code",
            "redirect_uri": config.redirect_uri,
        }))
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "Square ObtainToken failed");
        return Err(());
    }
    response.json::<TokenResponse>().await.map_err(|_| ())
}

async fn refresh_access_token(
    config: &SquareConfig,
    refresh_token: &str,
) -> Result<TokenResponse, ()> {
    let client = http_client();
    let response = client
        .post(format!("{}/oauth2/token", config.oauth_base()))
        .header(header::CONTENT_TYPE, "application/json")
        .header("Square-Version", SQUARE_VERSION)
        .json(&json!({
            "client_id": config.application_id,
            "client_secret": config.application_secret,
            "refresh_token": refresh_token,
            "grant_type": "refresh_token",
        }))
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    response.json::<TokenResponse>().await.map_err(|_| ())
}

async fn fetch_primary_location(
    config: &SquareConfig,
    access_token: &str,
) -> Result<Option<String>, ()> {
    let client = http_client();
    let response = client
        .get(format!("{}/v2/locations", config.api_base()))
        .header(header::AUTHORIZATION, format!("Bearer {access_token}"))
        .header("Square-Version", SQUARE_VERSION)
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    let body: Value = response.json().await.map_err(|_| ())?;
    let locations = body
        .get("locations")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default();
    let active = locations.iter().find(|loc| {
        loc.get("status")
            .and_then(|s| s.as_str())
            .is_some_and(|s| s.eq_ignore_ascii_case("ACTIVE"))
    });
    let chosen = active.or_else(|| locations.first());
    Ok(chosen
        .and_then(|loc| loc.get("id"))
        .and_then(|id| id.as_str())
        .map(str::to_owned))
}

pub(crate) async fn sync_now(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    require_config(&state)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(m.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let selected = sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*)=2 FROM restaurant_setup_streams
         WHERE restaurant_id=$1 AND stream IN ('menu','sales')
           AND method='connector' AND connector_provider='square'",
    )
    .bind(m.restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if !selected {
        return Err(ApiError(StatusCode::CONFLICT, "Select Square first."));
    }
    let connection_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM source_connections
         WHERE restaurant_id=$1 AND provider=$2 AND status IN ('connected','error','needs_reauth','syncing')",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Connect Square before syncing.",
    ))?;
    sqlx::query(
        "INSERT INTO source_sync_runs(id,connection_id,restaurant_id,kind,status)
         VALUES($1,$2,$3,'incremental','queued')",
    )
    .bind(Uuid::now_v7())
    .bind(connection_id)
    .bind(m.restaurant_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(StatusCode::ACCEPTED)
}

pub(crate) async fn disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(m.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    sqlx::query("DELETE FROM oauth_states WHERE restaurant_id=$1 AND provider=$2")
        .bind(m.restaurant_id)
        .bind(PROVIDER)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let running = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM source_sync_runs run
         JOIN source_connections connection
           ON connection.id=run.connection_id AND connection.restaurant_id=run.restaurant_id
         WHERE connection.restaurant_id=$1 AND connection.provider=$2
           AND run.status='running')",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if running {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Wait for the Square sync to finish.",
        ));
    }
    let n = sqlx::query(
        "UPDATE source_connections
         SET status='disconnected',
             access_token_encrypted=NULL,
             refresh_token_encrypted=NULL,
             last_error=NULL,
             updated_at=NOW()
         WHERE restaurant_id=$1 AND provider=$2 AND status<>'disconnected'",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?
    .rows_affected();
    if n == 0 {
        tx.rollback().await.map_err(database_error)?;
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "No Square connection to disconnect.",
        ));
    }
    sqlx::query(
        "UPDATE source_sync_runs run
         SET status='failed',error='Square was disconnected before this sync started.',
             finished_at=NOW()
         FROM source_connections connection
         WHERE run.connection_id=connection.id
           AND run.restaurant_id=connection.restaurant_id
           AND connection.restaurant_id=$1 AND connection.provider=$2
           AND run.status='queued'",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(StatusCode::NO_CONTENT)
}

#[derive(sqlx::FromRow)]
struct ConnectionRow {
    id: Uuid,
    #[allow(dead_code)]
    restaurant_id: Uuid,
    #[allow(dead_code)]
    status: String,
    external_location_id: Option<String>,
    access_token_encrypted: Option<String>,
    refresh_token_encrypted: Option<String>,
    access_token_expires_at: Option<DateTime<Utc>>,
    #[allow(dead_code)]
    last_success_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow, Debug)]
pub(crate) struct SyncJob {
    pub(crate) id: Uuid,
    pub(crate) connection_id: Uuid,
    pub(crate) restaurant_id: Uuid,
    pub(crate) kind: String,
    pub(crate) claim_token: Uuid,
}

/// How often each connected restaurant's sales are refreshed automatically.
const AUTO_SYNC_INTERVAL_MINUTES: i64 = 30;
/// How often the worker looks for connections whose auto-sync is due.
const AUTO_SYNC_TICK_SECS: u64 = 60;

pub(crate) async fn run_worker(pool: PgPool, config: Option<SquareConfig>, runs_scheduler: bool) {
    let Some(config) = config else {
        tracing::info!("Square sync worker idle (not configured)");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    };
    // One tenant's large first sync must not head-of-line block everyone
    // else, so main() spawns several workers. claim_job already refuses a
    // second concurrent run per connection, making this safe.
    let mut last_tick = if runs_scheduler {
        tokio::time::Instant::now() - std::time::Duration::from_secs(AUTO_SYNC_TICK_SECS)
    } else {
        tokio::time::Instant::now()
    };
    loop {
        if last_tick.elapsed().as_secs() >= AUTO_SYNC_TICK_SECS {
            last_tick = tokio::time::Instant::now();
            match enqueue_due_auto_syncs(&pool).await {
                Ok(count) if count > 0 => {
                    tracing::info!(enqueued = count, "scheduled automatic Square syncs");
                }
                Ok(_) => {}
                Err(error) => tracing::error!(%error, "Square auto-sync scheduling failed"),
            }
        }
        match claim_job(&pool).await {
            Ok(Some(job)) => {
                if let Err(error) = process_job(&pool, &config, job).await {
                    tracing::error!(%error, "Square sync job failed");
                }
            }
            Ok(None) => tokio::time::sleep(std::time::Duration::from_secs(3)).await,
            Err(error) => {
                tracing::error!(%error, "Square sync claim failed");
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        }
    }
}

// Queues an incremental refresh for every healthy connected restaurant whose
// newest run is older than the interval. A partial unique index allows at
// most one queued-or-running run per connection, so overlapping worker ticks
// cannot double-enqueue.
async fn enqueue_due_auto_syncs(pool: &PgPool) -> Result<u64, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let inserted = sqlx::query(
        "INSERT INTO source_sync_runs(id,connection_id,restaurant_id,kind,status)
         SELECT gen_random_uuid(), connection.id, connection.restaurant_id, 'incremental', 'queued'
         FROM source_connections connection
         WHERE connection.provider=$1
           AND connection.status='connected'
           AND (SELECT COUNT(*) FROM restaurant_setup_streams setup
                WHERE setup.restaurant_id=connection.restaurant_id
                  AND setup.stream IN ('menu','sales')
                  AND setup.method='connector'
                  AND setup.connector_provider='square')=2
           AND EXISTS (SELECT 1 FROM source_sync_runs done
                       WHERE done.connection_id=connection.id AND done.status='succeeded')
           AND COALESCE((SELECT MAX(run.created_at) FROM source_sync_runs run
                         WHERE run.connection_id=connection.id),
                        '-infinity'::timestamptz) <= NOW() - make_interval(mins => $2::int)
         ON CONFLICT DO NOTHING",
    )
    .bind(PROVIDER)
    .bind(AUTO_SYNC_INTERVAL_MINUTES)
    .execute(&mut *tx)
    .await?
    .rows_affected();
    tx.commit().await?;
    Ok(inserted)
}

async fn claim_job(pool: &PgPool) -> Result<Option<SyncJob>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    sqlx::query(
        "UPDATE source_sync_runs SET status='failed',error='Square sync timed out.',
           finished_at=NOW(),claim_token=NULL,lease_expires_at=NULL
         WHERE status='running' AND claim_token IS NOT NULL AND lease_expires_at<NOW()",
    )
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE source_connections connection SET
           status=COALESCE((SELECT CASE run.status WHEN 'succeeded' THEN 'connected' ELSE 'error' END
                            FROM source_sync_runs run WHERE run.connection_id=connection.id
                              AND run.status IN ('succeeded','failed')
                            ORDER BY run.finished_at DESC NULLS LAST LIMIT 1),'error'),
           last_error=(SELECT run.error FROM source_sync_runs run
                       WHERE run.connection_id=connection.id
                         AND run.status IN ('succeeded','failed')
                       ORDER BY run.finished_at DESC NULLS LAST LIMIT 1),
           updated_at=NOW()
         WHERE connection.provider='square' AND connection.status='syncing'
           AND NOT EXISTS (SELECT 1 FROM source_sync_runs run
                           WHERE run.connection_id=connection.id AND run.status='running')",
    )
    .execute(&mut *tx)
    .await?;
    let candidate = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT run.id,run.restaurant_id FROM source_sync_runs run
         JOIN source_connections connection
           ON connection.id=run.connection_id AND connection.restaurant_id=run.restaurant_id
         WHERE run.status='queued' AND connection.provider='square'
           AND NOT EXISTS (SELECT 1 FROM source_sync_runs active
                           WHERE active.connection_id=run.connection_id
                             AND active.status='running')
         ORDER BY run.created_at LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some((candidate_id, restaurant_id)) = candidate else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(restaurant_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE source_sync_runs run SET status='failed',
           error='Square is not active for setup.',finished_at=NOW()
         FROM source_connections connection
         WHERE run.connection_id=connection.id
           AND run.restaurant_id=connection.restaurant_id
           AND run.restaurant_id=$1
           AND run.status='queued' AND connection.provider='square'
           AND (connection.status='disconnected' OR
             (SELECT COUNT(*) FROM restaurant_setup_streams setup
              WHERE setup.restaurant_id=run.restaurant_id
                AND setup.stream IN ('menu','sales')
                AND setup.method='connector'
                AND setup.connector_provider='square')<>2)",
    )
    .bind(restaurant_id)
    .execute(&mut *tx)
    .await?;
    let job = sqlx::query_as::<_, SyncJob>(
        "SELECT run.id,run.connection_id,run.restaurant_id,run.kind,
                $2::uuid claim_token
         FROM source_sync_runs run
         JOIN source_connections connection
           ON connection.id=run.connection_id AND connection.restaurant_id=run.restaurant_id
         WHERE run.id=$1 AND run.status='queued' AND connection.provider='square'
           AND connection.status<>'disconnected'
           AND (SELECT COUNT(*) FROM restaurant_setup_streams setup
                WHERE setup.restaurant_id=run.restaurant_id
                  AND setup.stream IN ('menu','sales')
                  AND setup.method='connector'
                  AND setup.connector_provider='square')=2
         FOR UPDATE SKIP LOCKED
         ",
    )
    .bind(candidate_id)
    .bind(Uuid::now_v7())
    .fetch_optional(&mut *tx)
    .await?;
    let Some(job) = job else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query(
        "UPDATE source_sync_runs SET status='running',started_at=NOW(),
           claim_token=$2,lease_expires_at=NOW()+INTERVAL '20 minutes' WHERE id=$1",
    )
    .bind(job.id)
    .bind(job.claim_token)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "UPDATE source_connections SET status='syncing',updated_at=NOW()
         WHERE id=$1 AND restaurant_id=$2 AND status<>'disconnected'",
    )
    .bind(job.connection_id)
    .bind(job.restaurant_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

async fn process_job(pool: &PgPool, config: &SquareConfig, job: SyncJob) -> Result<(), String> {
    let connection = sqlx::query_as::<_, ConnectionRow>(
        "SELECT id,restaurant_id,status,external_location_id,access_token_encrypted,
                refresh_token_encrypted,access_token_expires_at,last_success_at
         FROM source_connections WHERE id=$1 AND restaurant_id=$2 AND provider='square'",
    )
    .bind(job.connection_id)
    .bind(job.restaurant_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?
    .ok_or_else(|| "connection missing".to_owned())?;

    let timezone = sqlx::query_scalar::<_, String>("SELECT timezone FROM restaurants WHERE id=$1")
        .bind(job.restaurant_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;

    let result = tokio::time::timeout(
        std::time::Duration::from_secs(15 * 60),
        run_sync(pool, config, &connection, &job, &timezone),
    )
    .await
    .unwrap_or_else(|_| Err("Square sync timed out.".to_owned()));
    match result {
        Ok(stats) => {
            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
            sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
                .bind(job.restaurant_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            let finalized = sqlx::query(
                "UPDATE source_sync_runs
                 SET status='succeeded',stats=$2,finished_at=NOW(),error=NULL,
                     claim_token=NULL,lease_expires_at=NULL
                 WHERE id=$1 AND claim_token=$3",
            )
            .bind(job.id)
            .bind(stats)
            .bind(job.claim_token)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
            if finalized != 1 {
                tx.commit().await.map_err(|e| e.to_string())?;
                return Ok(());
            }
            sqlx::query(
                "UPDATE source_connections
                 SET status='connected',last_sync_at=NOW(),last_success_at=NOW(),
                     last_error=NULL,updated_at=NOW()
                 WHERE id=$1 AND restaurant_id=$2 AND status<>'disconnected'",
            )
            .bind(job.connection_id)
            .bind(job.restaurant_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
        }
        Err(error) => {
            let message = error.chars().take(500).collect::<String>();
            let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
            sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
                .bind(job.restaurant_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            let finalized = sqlx::query(
                "UPDATE source_sync_runs
                 SET status='failed',error=$2,finished_at=NOW(),
                     claim_token=NULL,lease_expires_at=NULL
                 WHERE id=$1 AND claim_token=$3",
            )
            .bind(job.id)
            .bind(&message)
            .bind(job.claim_token)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?
            .rows_affected();
            if finalized != 1 {
                tx.commit().await.map_err(|e| e.to_string())?;
                return Ok(());
            }
            let status = if message.contains("reauth") {
                "needs_reauth"
            } else {
                "error"
            };
            sqlx::query(
                "UPDATE source_connections
                 SET status=$2,last_sync_at=NOW(),last_error=$3,updated_at=NOW()
                 WHERE id=$1 AND restaurant_id=$4 AND status<>'disconnected'",
            )
            .bind(job.connection_id)
            .bind(status)
            .bind(&message)
            .bind(job.restaurant_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            tx.commit().await.map_err(|e| e.to_string())?;
        }
    }
    Ok(())
}

async fn run_sync(
    pool: &PgPool,
    config: &SquareConfig,
    connection: &ConnectionRow,
    job: &SyncJob,
    timezone: &str,
) -> Result<Value, String> {
    let access_enc = connection
        .access_token_encrypted
        .as_deref()
        .ok_or_else(|| "reauth required".to_owned())?;
    let refresh_enc = connection
        .refresh_token_encrypted
        .as_deref()
        .ok_or_else(|| "reauth required".to_owned())?;
    let mut access = decrypt_secret(&config.token_key, access_enc).map_err(|e| e.1.to_owned())?;
    let refresh = decrypt_secret(&config.token_key, refresh_enc).map_err(|e| e.1.to_owned())?;
    if connection
        .access_token_expires_at
        .is_some_and(|at| at <= Utc::now() + Duration::hours(24))
    {
        let token = refresh_access_token(config, &refresh)
            .await
            .map_err(|_| "reauth required".to_owned())?;
        access = token.access_token;
        let access_enc = encrypt_secret(&config.token_key, &access).map_err(|e| e.1.to_owned())?;
        let refresh_enc = encrypt_secret(
            &config.token_key,
            if token.refresh_token.is_empty() {
                &refresh
            } else {
                &token.refresh_token
            },
        )
        .map_err(|e| e.1.to_owned())?;
        let expires_at = Utc::now() + Duration::seconds(token.expires_in.unwrap_or(30 * 24 * 3600));
        sqlx::query(
            "UPDATE source_connections
             SET access_token_encrypted=$2,refresh_token_encrypted=$3,
                 access_token_expires_at=$4,updated_at=NOW()
             WHERE id=$1",
        )
        .bind(connection.id)
        .bind(access_enc)
        .bind(refresh_enc)
        .bind(expires_at)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }

    let location_id = match &connection.external_location_id {
        Some(id) => id.clone(),
        None => fetch_primary_location(config, &access)
            .await
            .map_err(|_| "Could not load Square locations.".to_owned())?
            .ok_or_else(|| "No Square locations found.".to_owned())?,
    };
    if connection.external_location_id.is_none() {
        sqlx::query(
            "UPDATE source_connections SET external_location_id=$2,updated_at=NOW() WHERE id=$1",
        )
        .bind(connection.id)
        .bind(&location_id)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    }

    let menu_stats = sync_catalog(pool, config, &access, job.restaurant_id).await?;
    let sales_stats = sync_orders(pool, config, &access, job, &location_id, timezone).await?;
    Ok(json!({
        "menu": menu_stats,
        "sales": sales_stats,
        "locationId": location_id,
    }))
}

async fn square_get(config: &SquareConfig, access: &str, path: &str) -> Result<Value, String> {
    let client = http_client();
    let response = client
        .get(format!("{}{path}", config.api_base()))
        .header(header::AUTHORIZATION, format!("Bearer {access}"))
        .header("Square-Version", SQUARE_VERSION)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err("reauth required".into());
    }
    if !response.status().is_success() {
        return Err(format!("Square GET {path} failed: {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

async fn square_post(
    config: &SquareConfig,
    access: &str,
    path: &str,
    body: Value,
) -> Result<Value, String> {
    let client = http_client();
    let response = client
        .post(format!("{}{path}", config.api_base()))
        .header(header::AUTHORIZATION, format!("Bearer {access}"))
        .header("Square-Version", SQUARE_VERSION)
        .header(header::CONTENT_TYPE, "application/json")
        .json(&body)
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err("reauth required".into());
    }
    if !response.status().is_success() {
        return Err(format!("Square POST {path} failed: {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

/// Resolves an item's first Square category id to a display name, truncated
/// to Parline's menu-category limit. Pure so it stays unit-testable.
fn catalog_category_name(
    category_names: &HashMap<String, String>,
    item_data: &Value,
) -> Option<String> {
    let category_id = item_data
        .get("categories")?
        .as_array()?
        .first()?
        .get("id")?
        .as_str()?;
    let name = category_names.get(category_id)?.trim();
    if name.is_empty() {
        return None;
    }
    Some(truncate_chars(name, MENU_CATEGORY_MAX))
}

async fn fetch_category_names(
    config: &SquareConfig,
    access: &str,
) -> Result<HashMap<String, String>, String> {
    let mut names = HashMap::new();
    let mut cursor: Option<String> = None;
    loop {
        let path = match &cursor {
            Some(c) => format!(
                "/v2/catalog/list?types=CATEGORY&cursor={}",
                urlencoding_encode(c)
            ),
            None => "/v2/catalog/list?types=CATEGORY".to_owned(),
        };
        let body = square_get(config, access, &path).await?;
        for object in body
            .get("objects")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default()
        {
            if object.get("type").and_then(|t| t.as_str()) != Some("CATEGORY") {
                continue;
            }
            let id = object.get("id").and_then(|v| v.as_str());
            let name = object
                .pointer("/category_data/name")
                .and_then(|v| v.as_str())
                .map(str::trim)
                .filter(|v| !v.is_empty());
            if let (Some(id), Some(name)) = (id, name) {
                names.insert(id.to_owned(), name.to_owned());
            }
        }
        cursor = body
            .get("cursor")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(names)
}

/// One menu row ready for a bulk upsert.
struct CatalogUpsertRow {
    external_id: String,
    name: String,
    category: Option<String>,
    price: BigDecimal,
    currency: String,
    active: bool,
}

async fn load_menu_name_set(pool: &PgPool, restaurant_id: Uuid) -> Result<HashSet<String>, String> {
    let rows: Vec<String> =
        sqlx::query_scalar("SELECT LOWER(name) FROM menu_items WHERE restaurant_id=$1")
            .bind(restaurant_id)
            .fetch_all(pool)
            .await
            .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().collect())
}

/// Inserts or refreshes every variation in one statement. The partial unique
/// index on (restaurant_id, external_source, external_id) arbitrates insert
/// vs update; name collisions were resolved by the caller beforehand.
async fn bulk_upsert_menu_items(
    pool: &PgPool,
    restaurant_id: Uuid,
    rows: &[CatalogUpsertRow],
) -> Result<(), String> {
    if rows.is_empty() {
        return Ok(());
    }
    let mut builder = sqlx::QueryBuilder::new(
        "INSERT INTO menu_items
         (id,restaurant_id,name,category,selling_price,currency,active,external_source,external_id) ",
    );
    builder.push_values(rows.iter(), |mut bind, row| {
        bind.push_bind(Uuid::now_v7())
            .push_bind(restaurant_id)
            .push_bind(&row.name)
            .push_bind(row.category.as_deref())
            .push_bind(&row.price)
            .push_bind(&row.currency)
            .push_bind(row.active)
            .push_bind("square")
            .push_bind(&row.external_id);
    });
    builder.push(
        " ON CONFLICT (restaurant_id, external_source, external_id)
         WHERE external_source IS NOT NULL AND external_id IS NOT NULL
         DO UPDATE SET name=EXCLUDED.name, category=EXCLUDED.category,
           selling_price=EXCLUDED.selling_price, currency=EXCLUDED.currency,
           active=EXCLUDED.active, updated_at=NOW()",
    );
    builder
        .build()
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub(crate) async fn sync_catalog(
    pool: &PgPool,
    config: &SquareConfig,
    access: &str,
    restaurant_id: Uuid,
) -> Result<Value, String> {
    let category_names = fetch_category_names(config, access).await?;
    let mut used_names = load_menu_name_set(pool, restaurant_id).await?;
    let mut cursor: Option<String> = None;
    let mut upserted = 0u64;
    let mut skipped = 0u64;
    loop {
        let path = match &cursor {
            Some(c) => format!(
                "/v2/catalog/list?types=ITEM&cursor={}",
                urlencoding_encode(c)
            ),
            None => "/v2/catalog/list?types=ITEM".to_owned(),
        };
        let body = square_get(config, access, &path).await?;
        let objects = body
            .get("objects")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let mut rows: Vec<CatalogUpsertRow> = Vec::new();
        for object in objects {
            if object.get("type").and_then(|t| t.as_str()) != Some("ITEM") {
                continue;
            }
            let item = object.get("item_data").cloned().unwrap_or(json!({}));
            let item_name = item
                .get("name")
                .and_then(|v| v.as_str())
                .unwrap_or("Untitled")
                .trim();
            let category = catalog_category_name(&category_names, &item);
            let variations = item
                .get("variations")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            if variations.is_empty() {
                skipped += 1;
                continue;
            }
            for variation in variations {
                if variation.get("type").and_then(|t| t.as_str()) != Some("ITEM_VARIATION") {
                    continue;
                }
                let external_id = match variation.get("id").and_then(|v| v.as_str()) {
                    Some(id) => id,
                    None => {
                        skipped += 1;
                        continue;
                    }
                };
                let var_data = variation
                    .get("item_variation_data")
                    .cloned()
                    .unwrap_or(json!({}));
                let var_name = var_data
                    .get("name")
                    .and_then(|v| v.as_str())
                    .unwrap_or("Regular")
                    .trim();
                let mut name = if var_name.is_empty()
                    || var_name.eq_ignore_ascii_case("regular")
                    || var_name.eq_ignore_ascii_case("default")
                {
                    item_name.to_owned()
                } else {
                    format!("{item_name} ({var_name})")
                };
                name = truncate_chars(&name, MENU_NAME_MAX);
                if name.is_empty() {
                    skipped += 1;
                    continue;
                }
                // Resolve any name collision against everything already in the
                // restaurant plus names claimed earlier in this same sync.
                let lower = name.to_lowercase();
                if used_names.contains(&lower) {
                    let suffix =
                        format!(" · {}", &external_id[external_id.len().saturating_sub(6)..]);
                    let base_max = MENU_NAME_MAX.saturating_sub(suffix.chars().count());
                    name = format!("{}{suffix}", truncate_chars(&name, base_max));
                }
                used_names.insert(name.to_lowercase());
                let price_money = var_data.get("price_money").cloned().unwrap_or(json!({}));
                let amount = price_money
                    .get("amount")
                    .and_then(|v| v.as_i64())
                    .unwrap_or(0);
                let currency = price_money
                    .get("currency")
                    .and_then(|v| v.as_str())
                    .unwrap_or("USD")
                    .to_ascii_uppercase();
                if amount <= 0 || currency.len() != 3 {
                    skipped += 1;
                    continue;
                }
                let price = cents_to_decimal(amount);
                let active = object.get("is_deleted").and_then(|v| v.as_bool()) != Some(true);
                rows.push(CatalogUpsertRow {
                    external_id: external_id.to_owned(),
                    name,
                    category: category.clone(),
                    price,
                    currency,
                    active,
                });
            }
        }
        match bulk_upsert_menu_items(pool, restaurant_id, &rows).await {
            Ok(()) => upserted += rows.len() as u64,
            Err(error) => {
                tracing::warn!(%error, "bulk menu upsert failed; falling back per row");
                for row in &rows {
                    match upsert_menu_item(pool, restaurant_id, row).await {
                        Ok(true) => upserted += 1,
                        _ => skipped += 1,
                    }
                }
            }
        }
        cursor = body
            .get("cursor")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }
    Ok(json!({ "upserted": upserted, "skipped": skipped }))
}

async fn upsert_menu_item(
    pool: &PgPool,
    restaurant_id: Uuid,
    row: &CatalogUpsertRow,
) -> Result<bool, String> {
    let CatalogUpsertRow {
        external_id,
        name,
        category,
        price,
        currency,
        active,
    } = row;
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM menu_items
         WHERE restaurant_id=$1 AND external_source='square' AND external_id=$2",
    )
    .bind(restaurant_id)
    .bind(external_id)
    .fetch_optional(pool)
    .await
    .map_err(|e| e.to_string())?;
    if let Some(id) = existing {
        sqlx::query(
            "UPDATE menu_items
             SET name=$3,category=$4,selling_price=$5,currency=$6,active=$7,updated_at=NOW()
             WHERE id=$1 AND restaurant_id=$2",
        )
        .bind(id)
        .bind(restaurant_id)
        .bind(name)
        .bind(category)
        .bind(price)
        .bind(currency)
        .bind(active)
        .execute(pool)
        .await
        .map_err(|e| e.to_string())?;
        return Ok(true);
    }
    let clash = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(
            SELECT 1 FROM menu_items WHERE restaurant_id=$1 AND LOWER(name)=LOWER($2)
         )",
    )
    .bind(restaurant_id)
    .bind(name)
    .fetch_one(pool)
    .await
    .map_err(|e| e.to_string())?;
    let final_name = if clash {
        let suffix = format!(" · {}", &external_id[external_id.len().saturating_sub(6)..]);
        let base_max = MENU_NAME_MAX.saturating_sub(suffix.chars().count());
        format!("{}{suffix}", truncate_chars(name, base_max))
    } else {
        name.to_owned()
    };
    let id = Uuid::now_v7();
    let result = sqlx::query(
        "INSERT INTO menu_items
         (id,restaurant_id,name,category,selling_price,currency,active,external_source,external_id)
         VALUES($1,$2,$3,$4,$5,$6,$7,'square',$8)",
    )
    .bind(id)
    .bind(restaurant_id)
    .bind(&final_name)
    .bind(category)
    .bind(price)
    .bind(currency)
    .bind(active)
    .bind(external_id)
    .execute(pool)
    .await;
    match result {
        Ok(_) => Ok(true),
        Err(error)
            if error
                .as_database_error()
                .and_then(|e| e.code())
                .is_some_and(|c| c == "23505") =>
        {
            Ok(false)
        }
        Err(error) => Err(error.to_string()),
    }
}

fn cents_to_decimal(amount: i64) -> BigDecimal {
    let whole = amount / 100;
    let frac = (amount % 100).unsigned_abs();
    format!("{whole}.{frac:02}")
        .parse()
        .unwrap_or_else(|_| BigDecimal::from(0))
}

fn truncate_chars(value: &str, max: usize) -> String {
    value
        .chars()
        .take(max)
        .collect::<String>()
        .trim()
        .to_owned()
}

type SalesLineTotal = (BigDecimal, Option<i64>, Option<String>);

pub(crate) async fn sync_orders(
    pool: &PgPool,
    config: &SquareConfig,
    access: &str,
    job: &SyncJob,
    location_id: &str,
    timezone: &str,
) -> Result<Value, String> {
    let tz = timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC);
    let end = Utc::now();
    let start = if job.kind == "full" {
        end - Duration::days(INITIAL_SALES_DAYS)
    } else {
        let last = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT last_success_at FROM source_connections WHERE id=$1",
        )
        .bind(job.connection_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
        last.map(|t| t - Duration::days(1))
            .unwrap_or_else(|| end - Duration::days(INITIAL_SALES_DAYS))
    };

    let menu_map = load_square_menu_map(pool, job.restaurant_id).await?;
    let mut cursor: Option<String> = None;
    // business_date -> (menu_item_id -> (qty, net_sales cents, currency))
    let mut days: HashMap<NaiveDate, HashMap<Uuid, SalesLineTotal>> = HashMap::new();
    let mut orders_seen = 0u64;
    let mut lines_matched = 0u64;
    let mut lines_skipped = 0u64;

    loop {
        let mut body = json!({
            "location_ids": [location_id],
            "query": {
                "filter": {
                    "state_filter": { "states": ["COMPLETED"] },
                    "date_time_filter": {
                        "closed_at": {
                            "start_at": start.to_rfc3339(),
                            "end_at": end.to_rfc3339()
                        }
                    }
                },
                "sort": { "sort_field": "CLOSED_AT", "sort_order": "ASC" }
            },
            "limit": 100
        });
        if let Some(c) = &cursor {
            body.as_object_mut()
                .unwrap()
                .insert("cursor".into(), Value::String(c.clone()));
        }
        let response = square_post(config, access, "/v2/orders/search", body).await?;
        let orders = response
            .get("orders")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        for order in orders {
            orders_seen += 1;
            let closed_at = order
                .get("closed_at")
                .and_then(|v| v.as_str())
                .and_then(|s| DateTime::parse_from_rfc3339(s).ok())
                .map(|dt| dt.with_timezone(&Utc));
            let Some(closed_at) = closed_at else {
                continue;
            };
            let business_date = closed_at.with_timezone(&tz).date_naive();
            let line_items = order
                .get("line_items")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            for line in line_items {
                let catalog_id = line.get("catalog_object_id").and_then(|v| v.as_str());
                let Some(catalog_id) = catalog_id else {
                    lines_skipped += 1;
                    continue;
                };
                let Some(&menu_item_id) = menu_map.get(catalog_id) else {
                    lines_skipped += 1;
                    continue;
                };
                let qty = line
                    .get("quantity")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<BigDecimal>().ok())
                    .unwrap_or_else(|| BigDecimal::from(1));
                if qty <= 0 {
                    lines_skipped += 1;
                    continue;
                }
                let total_money = line.get("total_money").cloned().unwrap_or(json!({}));
                let amount = total_money.get("amount").and_then(|v| v.as_i64());
                let currency = total_money
                    .get("currency")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_ascii_uppercase());
                let entry = days
                    .entry(business_date)
                    .or_default()
                    .entry(menu_item_id)
                    .or_insert_with(|| (BigDecimal::from(0), None, None));
                entry.0 += qty;
                if let (Some(a), Some(c)) = (amount, currency.clone()) {
                    entry.1 = Some(entry.1.unwrap_or(0) + a);
                    entry.2 = Some(c);
                }
                lines_matched += 1;
            }
        }
        cursor = response
            .get("cursor")
            .and_then(|v| v.as_str())
            .map(str::to_owned);
        if cursor.is_none() {
            break;
        }
    }

    let mut days_written = 0u64;
    let system_user =
        sqlx::query_scalar::<_, Uuid>("SELECT created_by FROM source_connections WHERE id=$1")
            .bind(job.connection_id)
            .fetch_one(pool)
            .await
            .map_err(|e| e.to_string())?;

    for (business_date, lines) in days {
        if lines.is_empty() {
            continue;
        }
        write_sales_day(pool, job.restaurant_id, business_date, &lines, system_user).await?;
        days_written += 1;
    }

    Ok(json!({
        "ordersSeen": orders_seen,
        "linesMatched": lines_matched,
        "linesSkipped": lines_skipped,
        "daysWritten": days_written,
    }))
}

async fn load_square_menu_map(
    pool: &PgPool,
    restaurant_id: Uuid,
) -> Result<HashMap<String, Uuid>, String> {
    let rows = sqlx::query_as::<_, (String, Uuid)>(
        "SELECT external_id,id FROM menu_items
         WHERE restaurant_id=$1 AND external_source='square' AND external_id IS NOT NULL",
    )
    .bind(restaurant_id)
    .fetch_all(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(rows.into_iter().collect())
}

async fn write_sales_day(
    pool: &PgPool,
    restaurant_id: Uuid,
    business_date: NaiveDate,
    lines: &HashMap<Uuid, (BigDecimal, Option<i64>, Option<String>)>,
    user_id: Uuid,
) -> Result<(), String> {
    let mut tx = pool.begin().await.map_err(|e| e.to_string())?;
    let existing = sqlx::query_as::<_, (Uuid, Option<String>)>(
        "SELECT id,external_source FROM sales_days
         WHERE restaurant_id=$1 AND business_date=$2 FOR UPDATE",
    )
    .bind(restaurant_id)
    .bind(business_date)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|e| e.to_string())?;

    let day_id = match existing {
        Some((id, Some(source))) if source == "square" => {
            sqlx::query("DELETE FROM sales_lines WHERE sales_day_id=$1 AND restaurant_id=$2")
                .bind(id)
                .bind(restaurant_id)
                .execute(&mut *tx)
                .await
                .map_err(|e| e.to_string())?;
            sqlx::query(
                "UPDATE sales_days SET revision=revision+1,updated_by=$3,updated_at=NOW()
                 WHERE id=$1 AND restaurant_id=$2",
            )
            .bind(id)
            .bind(restaurant_id)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            id
        }
        Some((_, Some(_))) | Some((_, None)) => {
            // Manual or other source owns this day — do not overwrite.
            tx.commit().await.map_err(|e| e.to_string())?;
            return Ok(());
        }
        None => {
            let id = Uuid::now_v7();
            sqlx::query(
                "INSERT INTO sales_days
                 (id,restaurant_id,business_date,created_by,updated_by,external_source)
                 VALUES($1,$2,$3,$4,$4,'square')",
            )
            .bind(id)
            .bind(restaurant_id)
            .bind(business_date)
            .bind(user_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| e.to_string())?;
            id
        }
    };

    let item_ids: Vec<Uuid> = lines.keys().copied().collect();
    let name_by_id: HashMap<Uuid, String> = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,name FROM menu_items WHERE restaurant_id=$1 AND id = ANY($2)",
    )
    .bind(restaurant_id)
    .bind(&item_ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(|e| e.to_string())?
    .into_iter()
    .map(|(id, name)| (id, truncate_chars(&name, MENU_NAME_MAX)))
    .collect();

    let mut builder = sqlx::QueryBuilder::new(
        "INSERT INTO sales_lines
         (sales_day_id,restaurant_id,menu_item_id,menu_item_name,quantity,reported_net_sales,currency) ",
    );
    builder.push_values(
        lines.iter().filter_map(|(menu_item_id, line)| {
            let name = name_by_id.get(menu_item_id)?;
            let (net, cur) = match (&line.1, &line.2) {
                (Some(cents), Some(c)) if *cents >= 0 && c.len() == 3 => {
                    (Some(cents_to_decimal(*cents)), Some(c.clone()))
                }
                _ => (None, None),
            };
            Some((menu_item_id, name, &line.0, net, cur))
        }),
        |mut bind, (menu_item_id, name, qty, net, cur)| {
            bind.push_bind(day_id)
                .push_bind(restaurant_id)
                .push_bind(*menu_item_id)
                .push_bind(name)
                .push_bind(qty)
                .push_bind(net)
                .push_bind(cur);
        },
    );
    builder
        .build()
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;

    tx.commit().await.map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn money_and_names() {
        assert_eq!(cents_to_decimal(1234).to_string(), "12.34");
        assert_eq!(cents_to_decimal(100).to_string(), "1.00");
        assert_eq!(
            truncate_chars("Hello world item name that is very long indeed", 10).len(),
            10
        );
        assert!(truncate_chars("  x  ", 50) == "x");
    }

    #[test]
    fn resolves_first_category_through_the_id_map() {
        let mut names = HashMap::new();
        names.insert("CAT1".into(), "  Beverages  ".into());
        names.insert("CAT2".into(), "x".repeat(40));
        let item = json!({"categories":[{"id":"CAT1"},{"id":"CAT2"}]});
        assert_eq!(
            catalog_category_name(&names, &item).as_deref(),
            Some("Beverages")
        );
        let long = json!({"categories":[{"id":"CAT2"}]});
        let resolved = catalog_category_name(&names, &long).unwrap();
        assert_eq!(resolved.chars().count(), MENU_CATEGORY_MAX);
        assert_eq!(catalog_category_name(&names, &json!({})), None);
        assert_eq!(
            catalog_category_name(&names, &json!({"categories":[{"id":"MISSING"}]})),
            None
        );
        let blank = json!({"categories":[]});
        assert_eq!(catalog_category_name(&names, &blank), None);
    }

    #[test]
    fn encrypt_roundtrip() {
        let key = Sha256::digest(b"test-secret");
        let mut arr = [0u8; 32];
        arr.copy_from_slice(&key);
        let enc = encrypt_secret(&arr, "square-token").unwrap();
        let next = encrypt_secret(&arr, "square-token").unwrap();
        assert_eq!(decrypt_secret(&arr, &enc).unwrap(), "square-token");
        assert_ne!(enc, next);
    }

    #[test]
    fn production_oauth_is_read_only() {
        let config = SquareConfig {
            application_id: "app".into(),
            application_secret: "secret".into(),
            environment: "production".into(),
            redirect_uri: "https://example.com/callback".into(),
            token_key: [1; 32],
            web_origin: "https://example.com".into(),
        };
        assert_eq!(
            config.scopes(),
            "ITEMS_READ ORDERS_READ MERCHANT_PROFILE_READ"
        );
        assert!(!config.scopes().contains("WRITE"));
    }
}
