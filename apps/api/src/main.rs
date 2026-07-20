mod auth;
mod extraction;
mod invoices;
mod storage;

use std::{env, net::SocketAddr};

use anyhow::{Context, Result};
use auth::JwtVerifier;
use axum::{
    Json, Router,
    extract::{DefaultBodyLimit, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, postgres::PgPoolOptions};
use storage::ObjectStorage;
use tokio::net::TcpListener;
use tower_http::{
    cors::{AllowOrigin, CorsLayer},
    trace::TraceLayer,
};
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Clone)]
struct AppState {
    pool: PgPool,
    verifier: JwtVerifier,
    storage: ObjectStorage,
}

#[derive(Serialize)]
struct HealthResponse {
    status: &'static str,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Restaurant {
    id: uuid::Uuid,
    name: String,
    city: String,
    service_style: String,
    role: String,
}

#[derive(Serialize)]
struct MeResponse {
    restaurant: Option<Restaurant>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CreateRestaurant {
    name: String,
    city: String,
    service_style: String,
}

#[derive(Serialize)]
struct ErrorBody {
    error: &'static str,
}

#[derive(Debug)]
struct ApiError(StatusCode, &'static str);

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, Json(ErrorBody { error: self.1 })).into_response()
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    init_tracing();

    let database_url = required_env("DATABASE_URL")?;
    let host = env::var("API_HOST").unwrap_or_else(|_| "0.0.0.0".into());
    let port = env::var("API_PORT")
        .unwrap_or_else(|_| "8080".into())
        .parse::<u16>()
        .context("API_PORT must be a valid port")?;
    let web_origin = env::var("WEB_ORIGIN").unwrap_or_else(|_| "http://localhost:5173".into());
    let verifier = JwtVerifier::new(
        required_env("WORKOS_ISSUER")?,
        required_env("WORKOS_JWKS_URL")?,
    )
    .context("failed to configure WorkOS JWT verifier")?;
    let storage = ObjectStorage::from_env()
        .await
        .context("failed to configure private Cloudflare R2 storage")?;

    let pool = PgPoolOptions::new()
        .max_connections(10)
        .connect(&database_url)
        .await
        .context("failed to connect to PostgreSQL")?;

    sqlx::migrate!()
        .run(&pool)
        .await
        .context("failed to run database migrations")?;

    let gemini = extraction::GeminiClient::from_env().context("failed to configure Gemini")?;
    tokio::spawn(extraction::run_worker(
        pool.clone(),
        storage.clone(),
        gemini,
    ));

    let app = router(
        AppState {
            pool,
            verifier,
            storage,
        },
        web_origin
            .parse::<HeaderValue>()
            .context("WEB_ORIGIN must be a valid HTTP header value")?,
    );
    let address = format!("{host}:{port}")
        .parse::<SocketAddr>()
        .context("API_HOST and API_PORT must form a valid socket address")?;
    let listener = TcpListener::bind(address)
        .await
        .context("failed to bind API listener")?;

    info!(%address, "restaurant API listening");
    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("API server failed")
}

fn router(state: AppState, web_origin: HeaderValue) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(AllowOrigin::exact(web_origin))
        .allow_methods([Method::GET, Method::POST, Method::PUT])
        .allow_headers([
            axum::http::header::AUTHORIZATION,
            axum::http::header::CONTENT_TYPE,
        ]);

    Router::new()
        .route("/health/live", get(live))
        .route("/health/ready", get(ready))
        .route("/v1/me", get(me))
        .route("/v1/restaurants", post(create_restaurant))
        .route(
            "/v1/invoices",
            get(invoices::list)
                .post(invoices::create)
                .layer(DefaultBodyLimit::max(11 * 1024 * 1024)),
        )
        .route("/v1/invoices/{id}/file", get(invoices::file_url))
        .route(
            "/v1/invoices/{id}/review",
            get(invoices::get_review).put(invoices::put_review),
        )
        .route("/v1/invoices/{id}/retry", post(invoices::retry))
        .route(
            "/v1/invoices/{id}/price-changes",
            get(invoices::price_changes),
        )
        .with_state(state)
        .layer(cors)
        .layer(TraceLayer::new_for_http())
}

async fn me(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<MeResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let restaurant = sqlx::query_as::<_, Restaurant>(
        "SELECT r.id, r.name, r.city, r.service_style, m.role
         FROM users u JOIN restaurant_memberships m ON m.user_id = u.id
         JOIN restaurants r ON r.id = m.restaurant_id WHERE u.auth_subject = $1",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(|_| {
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "We couldn't load your restaurant. Please try again.",
        )
    })?;
    Ok(Json(MeResponse { restaurant }))
}

async fn create_restaurant(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateRestaurant>,
) -> Result<(StatusCode, Json<Restaurant>), ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let input = input.validated()?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let user_id = uuid::Uuid::now_v7();
    let user_id: uuid::Uuid = sqlx::query_scalar(
        "INSERT INTO users (id, auth_subject) VALUES ($1, $2)
         ON CONFLICT (auth_subject) DO UPDATE SET updated_at = NOW() RETURNING id",
    )
    .bind(user_id)
    .bind(subject)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    let restaurant_id = uuid::Uuid::now_v7();
    sqlx::query("INSERT INTO restaurants (id, name, city, service_style) VALUES ($1, $2, $3, $4)")
        .bind(restaurant_id)
        .bind(&input.name)
        .bind(&input.city)
        .bind(&input.service_style)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    if let Err(error) = sqlx::query(
        "INSERT INTO restaurant_memberships (restaurant_id, user_id, role) VALUES ($1, $2, 'owner')",
    ).bind(restaurant_id).bind(user_id).execute(&mut *tx).await {
        let membership_exists = is_one_membership_violation(&error);
        tx.rollback().await.map_err(database_error)?;
        if membership_exists {
            return Err(ApiError(StatusCode::CONFLICT, "You already belong to a restaurant. Reload your Daybook."));
        }
        return Err(database_error(error));
    }
    tx.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(Restaurant {
            id: restaurant_id,
            name: input.name,
            city: input.city,
            service_style: input.service_style,
            role: "owner".into(),
        }),
    ))
}

async fn authenticated_subject(state: &AppState, headers: &HeaderMap) -> Result<String, ApiError> {
    state.verifier.subject(headers).await.map_err(|()| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "Your session could not be verified. Please sign in again.",
        )
    })
}

fn database_error(_: sqlx::Error) -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "We couldn't save your changes. Please try again.",
    )
}

fn is_one_membership_violation(error: &sqlx::Error) -> bool {
    error.as_database_error().is_some_and(|error| {
        error.code().as_deref() == Some("23505")
            && error.constraint() == Some("restaurant_memberships_one_restaurant_per_user")
    })
}

impl CreateRestaurant {
    fn validated(mut self) -> Result<Self, ApiError> {
        self.name = self.name.trim().to_owned();
        self.city = self.city.trim().to_owned();
        self.service_style = self.service_style.trim().to_owned();
        if self.name.is_empty() || self.name.chars().count() > 50 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Restaurant name must be between 1 and 50 characters.",
            ));
        }
        if self.city.is_empty() || self.city.chars().count() > 50 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "City must be between 1 and 50 characters.",
            ));
        }
        if !matches!(
            self.service_style.as_str(),
            "counter_service" | "full_service" | "fast_casual" | "cafe_bakery" | "bar"
        ) {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose a listed service style.",
            ));
        }
        Ok(self)
    }
}

async fn live() -> Json<HealthResponse> {
    Json(HealthResponse { status: "ok" })
}

async fn ready(State(state): State<AppState>) -> (StatusCode, Json<HealthResponse>) {
    match sqlx::query_scalar::<_, i32>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(1) => (StatusCode::OK, Json(HealthResponse { status: "ready" })),
        _ => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(HealthResponse {
                status: "unavailable",
            }),
        ),
    }
}

fn required_env(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} must be set"))
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| "restaurant_api=info".into());
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

async fn shutdown_signal() {
    if let Err(error) = tokio::signal::ctrl_c().await {
        tracing::error!(%error, "failed to install shutdown signal handler");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(name: &str, city: &str, style: &str) -> CreateRestaurant {
        CreateRestaurant {
            name: name.into(),
            city: city.into(),
            service_style: style.into(),
        }
    }

    #[test]
    fn trims_and_accepts_closed_service_style() {
        let value = input("  Marigold  ", " Dallas ", "fast_casual")
            .validated()
            .unwrap();
        assert_eq!(value.name, "Marigold");
        assert_eq!(value.city, "Dallas");
    }

    #[test]
    fn rejects_blank_long_and_unknown_values() {
        assert!(input(" ", "Dallas", "bar").validated().is_err());
        assert!(input("Cafe", &"x".repeat(101), "bar").validated().is_err());
        assert!(input("Cafe", "Dallas", "other").validated().is_err());
    }
}
