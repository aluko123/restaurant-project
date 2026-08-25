use std::collections::HashMap;

use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Duration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use uuid::Uuid;

use crate::{
    ApiError, AppState, database_error,
    square::{
        CatalogUpsertRow, ConnectionRow, MENU_CATEGORY_MAX, MENU_NAME_MAX, cents_to_decimal,
        encrypt_secret, manager, member, truncate_chars, urlencoding_encode,
    },
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
    /// REST host: token exchange lives here, not on the OAuth host.
    fn api_base(&self) -> String {
        if let Some(base) = self.test_base_override() {
            return base;
        }
        if self.environment.eq_ignore_ascii_case("production") {
            "https://api.clover.com".to_owned()
        } else {
            "https://apisandbox.dev.clover.com".to_owned()
        }
    }

    /// Sandbox-only override so integration tests can point the connector
    /// at a local mock server. Production always talks to Clover directly.
    fn test_base_override(&self) -> Option<String> {
        if self.environment.eq_ignore_ascii_case("production") {
            return None;
        }
        let base = std::env::var("CLOVER_API_BASE_URL").ok()?;
        let base = base.trim().trim_end_matches('/').to_owned();
        (!base.is_empty()).then_some(base)
    }

    fn oauth_base(&self) -> String {
        if let Some(base) = self.test_base_override() {
            return base;
        }
        if self.environment.eq_ignore_ascii_case("production") {
            "https://clover.com".to_owned()
        } else {
            "https://sandbox.dev.clover.com".to_owned()
        }
    }
}

const TOKEN_LIFETIME_DAYS: i64 = 30;

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
        "{}/oauth/v2/authorize?client_id={}&redirect_uri={}&state={}",
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
                "{origin}/sources?clover=error&message={}",
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
    let merchant = merchant_id.to_owned();
    // Same lifecycle guards as the Square adapter: one use per state link,
    // restaurant locked against concurrent connector changes, connector still
    // selected, and no silent merchant swaps.
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|_| "We couldn't save the Clover connection.")?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(|_| "We couldn't save the Clover connection.")?;
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
        return Err("This connection link expired. Try Connect Clover again.");
    }
    let selected = sqlx::query_scalar::<_, bool>(
        "SELECT COUNT(*)=2 FROM restaurant_setup_streams
         WHERE restaurant_id=$1 AND stream IN ('menu','sales')
           AND method='connector' AND connector_provider='clover'",
    )
    .bind(restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| "We couldn't verify the connection request.")?;
    if !selected {
        return Err("Clover is no longer selected.");
    }
    let existing_merchant = sqlx::query_scalar::<_, Option<String>>(
        "SELECT external_merchant_id FROM source_connections
         WHERE restaurant_id=$1 AND provider=$2",
    )
    .bind(restaurant_id)
    .bind(PROVIDER)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|_| "We couldn't verify the Clover account.")?
    .flatten();
    if existing_merchant.is_some() && existing_merchant.as_deref() != Some(merchant.as_str()) {
        return Err("This restaurant is linked to a different Clover account.");
    }
    // OAuth completion imports until the first sync succeeds, exactly like
    // the Square adapter: land as 'importing' with a queued full run and let
    // the shared worker flip the status once real data arrives.
    let connection_id = sqlx::query_scalar::<_, Uuid>(
        "INSERT INTO source_connections
         (id,restaurant_id,provider,status,external_merchant_id,
          access_token_encrypted,refresh_token_encrypted,access_token_expires_at,
          scopes,created_by)
         VALUES($1,$2,$3,'importing',$4,$5,$6,$7,'',$8)
         ON CONFLICT (restaurant_id,provider) DO UPDATE SET
           status='importing',external_merchant_id=EXCLUDED.external_merchant_id,
           access_token_encrypted=EXCLUDED.access_token_encrypted,
           refresh_token_encrypted=EXCLUDED.refresh_token_encrypted,
           access_token_expires_at=EXCLUDED.access_token_expires_at,
           last_error=NULL,last_sync_at=NULL,last_success_at=NULL,
           menu_last_success_at=NULL,sales_last_success_at=NULL,updated_at=NOW()
         RETURNING id",
    )
    .bind(Uuid::now_v7())
    .bind(restaurant_id)
    .bind(PROVIDER)
    .bind(merchant)
    .bind(access_enc)
    .bind(refresh_enc)
    .bind(Utc::now() + Duration::days(TOKEN_LIFETIME_DAYS))
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|_| "We couldn't save the Clover connection.")?;
    sqlx::query(
        "INSERT INTO source_sync_runs(id,connection_id,restaurant_id,kind,status)
         VALUES($1,$2,$3,'full','queued')",
    )
    .bind(Uuid::now_v7())
    .bind(connection_id)
    .bind(restaurant_id)
    .execute(&mut *tx)
    .await
    .map_err(|_| "We couldn't start the Clover sync.")?;
    tx.commit()
        .await
        .map_err(|_| "We couldn't save the Clover connection.")?;
    Ok(format!("{}/sources?clover=connected", config.web_origin))
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
        .post(format!("{}/oauth/v2/token", config.api_base()))
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
            "Wait for the Clover sync to finish.",
        ));
    }
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
    if n == 0 {
        tx.rollback().await.map_err(database_error)?;
        return Err(ApiError(StatusCode::NOT_FOUND, "Clover is not connected."));
    }
    sqlx::query(
        "UPDATE source_sync_runs run
         SET status='failed',error='Clover was disconnected before this sync started.',
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

fn base64_url(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<crate::square::ConnectionView>>, ApiError> {
    crate::square::connections_list(&state, &headers, PROVIDER, state.clover.is_some()).await
}

// --- Sync pipeline ---------------------------------------------------------
//
// Mirrors the Square adapter: a queued run is claimed by the shared worker,
// this module pulls menu and paid orders from the Clover REST API, and the
// per-domain outcomes land in source_connections so menu and sales report
// separately.

async fn http_client() -> reqwest::Client {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .build()
        .unwrap_or_default()
}

async fn clover_get(config: &CloverConfig, access: &str, path: &str) -> Result<Value, String> {
    let response = http_client()
        .await
        .get(format!("{}{path}", config.api_base()))
        .header("authorization", format!("Bearer {access}"))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if response.status() == StatusCode::UNAUTHORIZED {
        return Err("reauth required".into());
    }
    if !response.status().is_success() {
        return Err(format!("Clover GET {path} failed: {}", response.status()));
    }
    response.json().await.map_err(|e| e.to_string())
}

async fn refresh_access_token(
    config: &CloverConfig,
    refresh_token: &str,
) -> Result<(String, Option<String>), ()> {
    let response = http_client()
        .await
        .post(format!("{}/oauth/v2/token", config.api_base()))
        .form(&[
            ("client_id", config.application_id.as_str()),
            ("client_secret", config.application_secret.as_str()),
            ("refresh_token", refresh_token),
        ])
        .send()
        .await
        .map_err(|_| ())?;
    if !response.status().is_success() {
        tracing::warn!(status = %response.status(), "Clover token refresh failed");
        return Err(());
    }
    #[derive(Deserialize)]
    struct Refreshed {
        access_token: String,
        #[serde(default)]
        refresh_token: Option<String>,
    }
    let body: Refreshed = response.json().await.map_err(|_| ())?;
    Ok((body.access_token, body.refresh_token))
}

/// Decrypts the stored access token, refreshing first when expiry is close.
async fn ensure_access_token(
    pool: &PgPool,
    config: &CloverConfig,
    connection: &ConnectionRow,
) -> Result<String, String> {
    let access_enc = connection
        .access_token_encrypted
        .as_deref()
        .ok_or_else(|| "reauth required".to_owned())?;
    let access =
        crate::square::decrypt_secret(&config.token_key, access_enc).map_err(|e| e.1.to_owned())?;
    if connection
        .access_token_expires_at
        .is_none_or(|at| at > Utc::now() + Duration::hours(24))
    {
        return Ok(access);
    }
    let Some(refresh_enc) = connection.refresh_token_encrypted.as_deref() else {
        return Err("reauth required".to_owned());
    };
    let stored_refresh = crate::square::decrypt_secret(&config.token_key, refresh_enc)
        .map_err(|e| e.1.to_owned())?;
    let (next_access, next_refresh) = refresh_access_token(config, &stored_refresh)
        .await
        .map_err(|_| "reauth required".to_owned())?;
    let access_enc = crate::square::encrypt_secret(&config.token_key, &next_access)
        .map_err(|e| e.1.to_owned())?;
    let refresh_enc = match next_refresh {
        Some(token) if !token.is_empty() => Some(
            crate::square::encrypt_secret(&config.token_key, &token).map_err(|e| e.1.to_owned())?,
        ),
        _ => None,
    };
    sqlx::query(
        "UPDATE source_connections
         SET access_token_encrypted=COALESCE($2,access_token_encrypted),
             refresh_token_encrypted=COALESCE($3,refresh_token_encrypted),
             access_token_expires_at=$4,updated_at=NOW()
         WHERE id=$1",
    )
    .bind(connection.id)
    .bind(access_enc)
    .bind(refresh_enc)
    .bind(Utc::now() + Duration::days(TOKEN_LIFETIME_DAYS))
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;
    Ok(next_access)
}

fn merchant_id(connection: &ConnectionRow) -> Result<String, String> {
    connection
        .external_merchant_id
        .clone()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| "The Clover connection has no merchant.".to_owned())
}

/// Menu items carry no currency in the Clover API; fall back to the market.
fn currency_for_country(country: Option<&str>) -> &'static str {
    match country.unwrap_or("").to_ascii_uppercase().as_str() {
        "CA" => "CAD",
        "GB" => "GBP",
        "IE" | "DE" | "FR" | "ES" | "IT" | "NL" => "EUR",
        "AU" => "AUD",
        _ => "USD",
    }
}

/// Pages any `/v3/merchants/{m}/...` list endpoint by offset until it returns
/// fewer elements than requested.
async fn paged_elements(
    config: &CloverConfig,
    access: &str,
    base_path: &str,
    extra_query: &str,
) -> Result<Vec<Value>, String> {
    const PAGE: u32 = 100;
    let mut all = Vec::new();
    loop {
        let path = format!(
            "{base_path}?limit={PAGE}&offset={}&{extra_query}",
            all.len(),
        );
        let body = clover_get(config, access, &path).await?;
        let page = body
            .get("elements")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        let fetched = page.len();
        all.extend(page);
        if (fetched as u32) < PAGE {
            break;
        }
    }
    Ok(all)
}

async fn fetch_category_names(
    config: &CloverConfig,
    access: &str,
    merchant: &str,
) -> Result<HashMap<String, String>, String> {
    let mut names = HashMap::new();
    for element in paged_elements(
        config,
        access,
        &format!("/v3/merchants/{merchant}/categories"),
        "",
    )
    .await?
    {
        let id = element.get("id").and_then(|v| v.as_str());
        let name = element
            .get("name")
            .and_then(|v| v.as_str())
            .map(str::trim)
            .filter(|v| !v.is_empty());
        if let (Some(id), Some(name)) = (id, name) {
            names.insert(id.to_owned(), name.to_owned());
        }
    }
    Ok(names)
}

/// Resolves an item's first Clover category id to a display name, truncated
/// to Parline's menu-category limit. Pure so it stays unit-testable.
fn catalog_category_name(category_names: &HashMap<String, String>, item: &Value) -> Option<String> {
    let category_id = item
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

/// Same collision rule as Square's catalog: keep names unique inside the
/// restaurant by suffixing the external id tail.
fn dedupe_name(
    used_names: &mut std::collections::HashSet<String>,
    name: String,
    external_id: &str,
) -> String {
    if used_names.contains(&name.to_lowercase()) {
        let suffix = format!(" · {}", &external_id[external_id.len().saturating_sub(6)..]);
        let base_max = MENU_NAME_MAX.saturating_sub(suffix.chars().count());
        format!("{}{suffix}", truncate_chars(&name, base_max))
    } else {
        name
    }
}

const INITIAL_SALES_WINDOW_DAYS: i64 = 90;

pub(crate) async fn run_sync(
    pool: &PgPool,
    config: &CloverConfig,
    connection: &ConnectionRow,
    job: &crate::square::SyncJob,
    timezone: &str,
) -> Result<Value, String> {
    let merchant = merchant_id(connection)?;
    let access = ensure_access_token(pool, config, connection).await?;
    let country: Option<String> = sqlx::query_scalar("SELECT country FROM restaurants WHERE id=$1")
        .bind(job.restaurant_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| e.to_string())?
        .flatten();

    let menu_stats = sync_catalog(
        pool,
        config,
        &access,
        &merchant,
        job.restaurant_id,
        country.as_deref(),
    )
    .await?;
    sqlx::query(
        "UPDATE source_connections SET menu_last_success_at=NOW(),updated_at=NOW() WHERE id=$1",
    )
    .bind(connection.id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    let sales_stats = sync_orders(
        pool,
        config,
        &access,
        &merchant,
        job,
        timezone,
        country.as_deref(),
    )
    .await?;
    sqlx::query(
        "UPDATE source_connections SET sales_last_success_at=NOW(),updated_at=NOW() WHERE id=$1",
    )
    .bind(connection.id)
    .execute(pool)
    .await
    .map_err(|e| e.to_string())?;

    Ok(json!({
        "menu": menu_stats,
        "sales": sales_stats,
        "merchantId": merchant,
    }))
}

pub(crate) async fn sync_catalog(
    pool: &PgPool,
    config: &CloverConfig,
    access: &str,
    merchant: &str,
    restaurant_id: Uuid,
    country: Option<&str>,
) -> Result<Value, String> {
    let category_names = fetch_category_names(config, access, merchant).await?;
    let currency = currency_for_country(country);
    let mut used_names = crate::square::load_menu_name_set(pool, restaurant_id).await?;
    let items = paged_elements(
        config,
        access,
        &format!("/v3/merchants/{merchant}/items"),
        "expand=categories",
    )
    .await?;

    let mut rows: Vec<CatalogUpsertRow> = Vec::new();
    let mut skipped = 0u64;
    for item in items {
        let Some(external_id) = item.get("id").and_then(|v| v.as_str()) else {
            skipped += 1;
            continue;
        };
        let raw_name = item
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("Untitled")
            .trim();
        let name = truncate_chars(raw_name, MENU_NAME_MAX);
        if name.is_empty() {
            skipped += 1;
            continue;
        }
        // Hidden items still import; they only flip the active flag so past
        // sales keep matching while the menu stays truthful.
        let active = item.get("hidden").and_then(|v| v.as_bool()) != Some(true);
        let price_cents = item.get("price").and_then(|v| v.as_i64()).unwrap_or(0);
        if price_cents <= 0 || currency.len() != 3 {
            skipped += 1;
            continue;
        }
        let category = catalog_category_name(&category_names, &item);
        let final_name = dedupe_name(&mut used_names, name, external_id);
        used_names.insert(final_name.to_lowercase());
        rows.push(CatalogUpsertRow {
            external_id: external_id.to_owned(),
            name: final_name,
            category,
            price: cents_to_decimal(price_cents),
            currency: currency.to_owned(),
            active,
        });
    }

    let mut upserted = rows.len() as u64;
    if crate::square::bulk_upsert_menu_items(pool, restaurant_id, PROVIDER, &rows)
        .await
        .is_err()
    {
        tracing::warn!("bulk Clover menu upsert failed; falling back per row");
        upserted = 0;
        for row in &rows {
            if crate::square::upsert_menu_item(pool, restaurant_id, PROVIDER, row)
                .await
                .unwrap_or(false)
            {
                upserted += 1;
            }
        }
    }
    Ok(json!({ "upserted": upserted, "skipped": skipped }))
}

type SalesLineTotal = (BigDecimal, Option<i64>, Option<String>);

pub(crate) async fn sync_orders(
    pool: &PgPool,
    config: &CloverConfig,
    access: &str,
    merchant: &str,
    job: &crate::square::SyncJob,
    timezone: &str,
    country: Option<&str>,
) -> Result<Value, String> {
    let tz = timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC);
    let end = Utc::now();
    let start = if job.kind == "full" {
        end - Duration::days(INITIAL_SALES_WINDOW_DAYS)
    } else {
        let last = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
            "SELECT last_success_at FROM source_connections WHERE id=$1",
        )
        .bind(job.connection_id)
        .fetch_one(pool)
        .await
        .map_err(|e| e.to_string())?;
        last.map(|t| t - Duration::days(1))
            .unwrap_or_else(|| end - Duration::days(INITIAL_SALES_WINDOW_DAYS))
    };

    let menu_map = crate::square::load_provider_menu_map(pool, job.restaurant_id, PROVIDER).await?;
    let mut days: HashMap<NaiveDate, HashMap<Uuid, SalesLineTotal>> = HashMap::new();
    let mut orders_seen = 0u64;
    let mut lines_matched = 0u64;
    let mut lines_skipped = 0u64;

    let orders = paged_elements(
        config,
        access,
        &format!("/v3/merchants/{merchant}/orders"),
        &format!(
            "expand=lineItems&filter=modifiedTime%3E{}",
            start.timestamp_millis()
        ),
    )
    .await?;

    for order in orders {
        orders_seen += 1;
        // Only closed money counts: OPEN drafts and REFUNDED orders are not sales.
        let state = order
            .get("state")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_ascii_uppercase();
        if !matches!(state.as_str(), "PAID" | "CLOSED") {
            continue;
        }
        let created_ms = order.get("createdTime").and_then(|v| v.as_i64());
        let business_date = created_ms
            .and_then(|ms| Utc.timestamp_millis_opt(ms).single())
            .map(|dt| dt.with_timezone(&tz).date_naive());
        let Some(business_date) = business_date else {
            continue;
        };
        let line_items = match order.get("lineItems") {
            Some(Value::Object(wrapped)) => wrapped
                .get("elements")
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default(),
            Some(Value::Array(flat)) => flat.clone(),
            _ => Vec::new(),
        };
        for line in line_items {
            let Some(item_ref) = line.pointer("/item/id").and_then(|v| v.as_str()) else {
                lines_skipped += 1;
                continue;
            };
            let Some(&menu_item_id) = menu_map.get(item_ref) else {
                lines_skipped += 1;
                continue;
            };
            let qty = line
                .get("unitQty")
                .and_then(|v| v.as_str())
                .and_then(|s| s.parse::<BigDecimal>().ok())
                .unwrap_or_else(|| BigDecimal::from(1));
            if qty <= 0 {
                lines_skipped += 1;
                continue;
            }
            // Clover line items carry the unit price only; derive the line
            // total instead of under-counting every multi-quantity line.
            let unit_cents = line.get("price").and_then(|v| v.as_i64());
            let currency = currency_for_country(country).to_owned();
            let total_cents = unit_cents.map(|unit| {
                let qty_f = qty.to_string().parse::<f64>().unwrap_or(1.0);
                (unit as f64 * qty_f).round() as i64
            });
            let entry = days
                .entry(business_date)
                .or_default()
                .entry(menu_item_id)
                .or_insert_with(|| (BigDecimal::from(0), None, None));
            entry.0 += qty;
            if let Some(total) = total_cents {
                entry.1 = Some(entry.1.unwrap_or(0) + total);
                entry.2 = Some(currency);
            }
            lines_matched += 1;
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
        crate::square::write_sales_day(
            pool,
            job.restaurant_id,
            PROVIDER,
            business_date,
            &lines,
            system_user,
        )
        .await?;
        days_written += 1;
    }

    Ok(json!({
        "ordersSeen": orders_seen,
        "linesMatched": lines_matched,
        "linesSkipped": lines_skipped,
        "daysWritten": days_written,
    }))
}

/// Manual trigger for a queued incremental sync, mirroring Square's route.
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
           AND method='connector' AND connector_provider='clover'",
    )
    .bind(m.restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if !selected {
        return Err(ApiError(StatusCode::CONFLICT, "Select Clover first."));
    }
    let connection_id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM source_connections
         WHERE restaurant_id=$1 AND provider=$2
           AND status IN ('importing','connected','error','needs_reauth','syncing')",
    )
    .bind(m.restaurant_id)
    .bind(PROVIDER)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Connect Clover before syncing.",
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
