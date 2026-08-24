use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
    ApiError, AppState, database_error,
    square::{encrypt_secret, manager, member, urlencoding_encode},
};

const PROVIDER: &str = "clover";

#[derive(Clone)]
pub(crate) struct CloverConfig {
    application_id: String,
    application_secret: String,
    redirect_uri: String,
    environment: String,
    token_key: [u8; 32],
    web_origin: String,
}

impl CloverConfig {
    pub(crate) fn from_env(web_origin: &str) -> Option<Self> {
        let application_id = std::env::var("CLOVER_APPLICATION_ID").ok()?;
        let application_secret = std::env::var("CLOVER_APPLICATION_SECRET").ok()?;
        let redirect_uri = std::env::var("CLOVER_REDIRECT_URI").ok()?;
        if application_id.trim().is_empty()
            || application_secret.trim().is_empty()
            || redirect_uri.trim().is_empty()
        {
            return None;
        }
        let token_key = token_key_from_env().ok()?;
        Some(Self {
            application_id: application_id.trim().to_owned(),
            application_secret: application_secret.trim().to_owned(),
            redirect_uri: redirect_uri.trim().to_owned(),
            environment: std::env::var("CLOVER_ENVIRONMENT")
                .unwrap_or_else(|_| "sandbox".into())
                .trim()
                .to_owned(),
            token_key,
            web_origin: web_origin.trim_end_matches('/').to_owned(),
        })
    }

    /// Clover's OAuth v2 endpoints issue refresh tokens; the legacy flow does not.
    fn oauth_base(&self) -> String {
        if let Ok(base) = std::env::var("CLOVER_API_BASE_URL")
            && !self.environment.eq_ignore_ascii_case("production")
        {
            let base = base.trim().trim_end_matches('/').to_owned();
            if !base.is_empty() {
                return base;
            }
        }
        if self.environment.eq_ignore_ascii_case("production") {
            "https://clover.com".to_owned()
        } else {
            "https://sandbox.dev.clover.com".to_owned()
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

fn require_config(state: &AppState) -> Result<CloverConfig, ApiError> {
    state.clover.clone().ok_or(ApiError(
        StatusCode::NOT_IMPLEMENTED,
        "Clover connect is not configured on this server yet.",
    ))
}

#[derive(Deserialize)]
pub(crate) struct CallbackQuery {
    #[serde(default)]
    merchant_id: Option<String>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    state: Option<String>,
    #[serde(default)]
    error: Option<String>,
}

pub(crate) async fn status(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    Ok(Json(json!({
        "configured": state.clover.is_some(),
        "provider": PROVIDER,
    })))
}

pub(crate) async fn authorize(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Value>, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    let config = require_config(&state)?;
    let state_token = base64_url(Uuid::now_v7().as_bytes());
    sqlx::query(
        "INSERT INTO oauth_states(state,restaurant_id,user_id,provider,expires_at)
         VALUES($1,$2,$3,$4,$5)",
    )
    .bind(&state_token)
    .bind(m.restaurant_id)
    .bind(m.user_id)
    .bind(PROVIDER)
    .bind(Utc::now() + Duration::minutes(15))
    .execute(&state.pool)
    .await
    .map_err(database_error)?;
    let url = format!(
        "{}/oauth_v2/authorize?client_id={}&redirect_uri={}&state={}",
        config.oauth_base(),
        urlencoding_encode(&config.application_id),
        urlencoding_encode(&config.redirect_uri),
        urlencoding_encode(&state_token),
    );
    Ok(Json(json!({ "url": url, "configured": true })))
}

pub(crate) async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> Response {
    match complete_callback(&state, query).await {
        Ok(path) => axum::response::Redirect::temporary(&path).into_response(),
        Err(message) => {
            let origin = state
                .clover
                .as_ref()
                .map(|c| c.web_origin.as_str())
                .unwrap_or("http://localhost:5173");
            let path = format!(
                "{origin}/sources?square=error&message={}",
                urlencoding_encode(message)
            );
            axum::response::Redirect::temporary(&path).into_response()
        }
    }
}

async fn complete_callback(state: &AppState, query: CallbackQuery) -> Result<String, &'static str> {
    let config = state.clover.as_ref().ok_or("Clover is not configured.")?;
    if query.error.is_some() {
        return Err("Clover authorization was denied or failed.");
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
    let merchant_id = query
        .merchant_id
        .as_deref()
        .filter(|v| !v.is_empty())
        .ok_or("Missing Clover merchant id.")?;
    let row = sqlx::query_as::<_, (Uuid, Uuid)>(
        "SELECT restaurant_id,user_id FROM oauth_states
         WHERE state=$1 AND provider=$2 AND expires_at > NOW()",
    )
    .bind(state_token)
    .bind(PROVIDER)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| "We couldn't verify the connection request.")?
    .ok_or("This connection link expired. Try Connect Clover again.")?;
    let (restaurant_id, user_id) = row;
    let token = obtain_token(config, code)
        .await
        .map_err(|_| "Clover did not return access tokens. Try again.")?;
    let access_enc = encrypt_secret(&config.token_key, &token.access_token)
        .map_err(|_| "We couldn't secure the Clover tokens.")?;
    let refresh_enc = match &token.refresh_token {
        Some(refresh) => Some(
            encrypt_secret(&config.token_key, refresh)
                .map_err(|_| "We couldn't secure the Clover tokens.")?,
        ),
        None => None,
    };
    sqlx::query(
        "INSERT INTO source_connections
         (id,restaurant_id,provider,status,external_merchant_id,
          access_token_encrypted,refresh_token_encrypted,scopes,created_by)
         VALUES($1,$2,$3,'connected',$4,$5,$6,'',$7)
         ON CONFLICT (restaurant_id,provider) DO UPDATE SET
           status='connected',external_merchant_id=EXCLUDED.external_merchant_id,
           access_token_encrypted=EXCLUDED.access_token_encrypted,
           refresh_token_encrypted=EXCLUDED.refresh_token_encrypted,
           last_error=NULL,updated_at=NOW()",
    )
    .bind(Uuid::now_v7())
    .bind(restaurant_id)
    .bind(PROVIDER)
    .bind(merchant_id)
    .bind(access_enc)
    .bind(refresh_enc)
    .bind(user_id)
    .execute(&state.pool)
    .await
    .map_err(|_| "We couldn't save the Clover connection.")?;
    Ok(format!("{}/sources?square=connected", config.web_origin))
}

#[derive(Deserialize, Debug)]
struct TokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: Option<String>,
}

async fn obtain_token(config: &CloverConfig, code: &str) -> Result<TokenResponse, ()> {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .map_err(|_| ())?;
    let response = client
        .post(format!("{}/oauth_v2/token", config.oauth_base()))
        .header("accept", "application/json")
        .form(&[
            ("client_id", config.application_id.as_str()),
            ("client_secret", config.application_secret.as_str()),
            ("code", code),
        ])
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        return Err(());
    }
    response.json().await.map_err(|_| ())
}

pub(crate) async fn disconnect(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<StatusCode, ApiError> {
    let m = member(&state, &headers).await?;
    manager(&m)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    sqlx::query("DELETE FROM oauth_states WHERE restaurant_id=$1 AND provider=$2")
        .bind(m.restaurant_id)
        .bind(PROVIDER)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let n = sqlx::query(
        "UPDATE source_connections SET status='disconnected',
           access_token_encrypted=NULL,refresh_token_encrypted=NULL,updated_at=NOW()
         WHERE restaurant_id=$1 AND provider=$2 AND status<>'disconnected'",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?
    .rows_affected();
    tx.commit().await.map_err(database_error)?;
    if n == 0 {
        return Err(ApiError(StatusCode::NOT_FOUND, "Clover is not connected."));
    }
    Ok(StatusCode::NO_CONTENT)
}

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}
