use std::collections::HashMap;

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

    fn api_base(&self) -> &'static str {
        if self.environment.eq_ignore_ascii_case("production") {
            "https://connect.squareup.com"
        } else {
            "https://connect.squareupsandbox.com"
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

fn encrypt_secret(key: &[u8; 32], plain: &str) -> Result<String, ApiError> {
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

#[derive(sqlx::FromRow)]
struct Member {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
    #[allow(dead_code)]
    timezone: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ConnectionView {
    id: Uuid,
    provider: String,
    status: String,
    external_merchant_id: Option<String>,
    external_location_id: Option<String>,
    last_sync_at: Option<DateTime<Utc>>,
    last_success_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    configured: bool,
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

async fn member(state: &AppState, headers: &HeaderMap) -> Result<Member, ApiError> {
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

fn manager(m: &Member) -> Result<(), ApiError> {
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
    let m = member(&state, &headers).await?;
    manager(&m)?;
    let configured = state.square.is_some();
    let mut rows = sqlx::query_as::<_, ConnectionView>(
        "SELECT id,provider,status,external_merchant_id,external_location_id,
                last_sync_at,last_success_at,last_error,created_at,updated_at,
                FALSE AS configured
         FROM source_connections
         WHERE restaurant_id=$1 AND provider=$2 AND status<>'disconnected'
         ORDER BY updated_at DESC",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    for row in &mut rows {
        row.configured = configured;
    }
    if rows.is_empty() && configured {
        // Synthetic “available to connect” is handled by the UI via configured flag on empty list.
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

fn urlencoding_encode(value: &str) -> String {
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
        "DELETE FROM oauth_states
         WHERE state=$1 AND provider=$2 AND expires_at > NOW()
         RETURNING restaurant_id,user_id",
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
    sqlx::query(
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
    .execute(&state.pool)
    .await
    .map_err(|_| "We couldn't save the Square connection.")?;
    let connection_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM source_connections WHERE restaurant_id=$1 AND provider=$2",
    )
    .bind(restaurant_id)
    .bind(PROVIDER)
    .fetch_one(&state.pool)
    .await
    .map_err(|_| "We couldn't save the Square connection.")?;
    enqueue_sync(&state.pool, connection_id, restaurant_id, "full")
        .await
        .ok();
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
    let client = reqwest::Client::new();
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
    let client = reqwest::Client::new();
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
    let client = reqwest::Client::new();
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

async fn enqueue_sync(
    pool: &PgPool,
    connection_id: Uuid,
    restaurant_id: Uuid,
    kind: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO source_sync_runs(id,connection_id,restaurant_id,kind,status)
         VALUES($1,$2,$3,$4,'queued')",
    )
    .bind(Uuid::now_v7())
    .bind(connection_id)
    .bind(restaurant_id)
    .bind(kind)
    .execute(pool)
    .await
    .map_err(database_error)?;
    Ok(())
}

pub(crate) async fn sync_now(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    require_config(&state)?;
    let connection_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM source_connections
         WHERE restaurant_id=$1 AND provider=$2 AND status IN ('connected','error','needs_reauth','syncing')",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Connect Square before syncing.",
    ))?;
    enqueue_sync(&state.pool, connection_id, m.restaurant_id, "incremental").await?;
    Ok(StatusCode::ACCEPTED)
}

pub(crate) async fn disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
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

#[derive(sqlx::FromRow)]
struct SyncJob {
    id: Uuid,
    connection_id: Uuid,
    restaurant_id: Uuid,
    kind: String,
}

pub(crate) async fn run_worker(pool: PgPool, config: Option<SquareConfig>) {
    let Some(config) = config else {
        tracing::info!("Square sync worker idle (not configured)");
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(3600)).await;
        }
    };
    loop {
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

async fn claim_job(pool: &PgPool) -> Result<Option<SyncJob>, sqlx::Error> {
    let mut tx = pool.begin().await?;
    let job = sqlx::query_as::<_, SyncJob>(
        "SELECT run.id,run.connection_id,run.restaurant_id,run.kind
         FROM source_sync_runs run
         JOIN source_connections connection
           ON connection.id=run.connection_id AND connection.restaurant_id=run.restaurant_id
         WHERE run.status='queued' AND connection.provider='square'
           AND connection.status<>'disconnected'
         ORDER BY run.created_at
         FOR UPDATE SKIP LOCKED
         LIMIT 1",
    )
    .fetch_optional(&mut *tx)
    .await?;
    let Some(job) = job else {
        tx.commit().await?;
        return Ok(None);
    };
    sqlx::query("UPDATE source_sync_runs SET status='running',started_at=NOW() WHERE id=$1")
        .bind(job.id)
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

    let result = run_sync(pool, config, &connection, &job, &timezone).await;
    match result {
        Ok(stats) => {
            sqlx::query(
                "UPDATE source_sync_runs
                 SET status='succeeded',stats=$2,finished_at=NOW(),error=NULL
                 WHERE id=$1",
            )
            .bind(job.id)
            .bind(stats)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
            sqlx::query(
                "UPDATE source_connections
                 SET status='connected',last_sync_at=NOW(),last_success_at=NOW(),
                     last_error=NULL,updated_at=NOW()
                 WHERE id=$1 AND restaurant_id=$2 AND status<>'disconnected'",
            )
            .bind(job.connection_id)
            .bind(job.restaurant_id)
            .execute(pool)
            .await
            .map_err(|e| e.to_string())?;
        }
        Err(error) => {
            let message = error.chars().take(500).collect::<String>();
            sqlx::query(
                "UPDATE source_sync_runs
                 SET status='failed',error=$2,finished_at=NOW() WHERE id=$1",
            )
            .bind(job.id)
            .bind(&message)
            .execute(pool)
            .await
            .ok();
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
            .execute(pool)
            .await
            .ok();
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
    let client = reqwest::Client::new();
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
    let client = reqwest::Client::new();
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

async fn sync_catalog(
    pool: &PgPool,
    config: &SquareConfig,
    access: &str,
    restaurant_id: Uuid,
) -> Result<Value, String> {
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
            let category = item
                .get("categories")
                .and_then(|v| v.as_array())
                .and_then(|arr| arr.first())
                .and_then(|c| c.get("id"))
                .and_then(|id| id.as_str());
            let _ = category; // category names need separate lookup; leave null for v1
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
                match upsert_menu_item(
                    pool,
                    restaurant_id,
                    external_id,
                    &name,
                    &price,
                    &currency,
                    active,
                )
                .await
                {
                    Ok(true) => upserted += 1,
                    Ok(false) => skipped += 1,
                    Err(error) => {
                        tracing::warn!(%error, external_id, "menu upsert failed");
                        skipped += 1;
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
    external_id: &str,
    name: &str,
    price: &BigDecimal,
    currency: &str,
    active: bool,
) -> Result<bool, String> {
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
             SET name=$3,selling_price=$4,currency=$5,active=$6,updated_at=NOW()
             WHERE id=$1 AND restaurant_id=$2",
        )
        .bind(id)
        .bind(restaurant_id)
        .bind(name)
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
         VALUES($1,$2,$3,NULL,$4,$5,$6,'square',$7)",
    )
    .bind(id)
    .bind(restaurant_id)
    .bind(&final_name)
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

async fn sync_orders(
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

    for (menu_item_id, (qty, net_cents, currency)) in lines {
        let name = sqlx::query_scalar::<_, String>(
            "SELECT name FROM menu_items WHERE id=$1 AND restaurant_id=$2",
        )
        .bind(menu_item_id)
        .bind(restaurant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
        let Some(name) = name else {
            continue;
        };
        let name = truncate_chars(&name, MENU_NAME_MAX);
        let (net, cur) = match (net_cents, currency) {
            (Some(cents), Some(c)) if *cents >= 0 && c.len() == 3 => {
                (Some(cents_to_decimal(*cents)), Some(c.clone()))
            }
            _ => (None, None),
        };
        sqlx::query(
            "INSERT INTO sales_lines
             (sales_day_id,restaurant_id,menu_item_id,menu_item_name,quantity,reported_net_sales,currency)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(day_id)
        .bind(restaurant_id)
        .bind(menu_item_id)
        .bind(&name)
        .bind(qty)
        .bind(net)
        .bind(cur)
        .execute(&mut *tx)
        .await
        .map_err(|e| e.to_string())?;
    }
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
