use axum::{Json, extract::State, http::HeaderMap};
use serde::Serialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error};

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PlaceOption {
    country: String,
    region: String,
    city: String,
}

/// Available to any signed-in user: the onboarding form needs it before a
/// restaurant (and therefore a membership) exists.
pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<PlaceOption>>, ApiError> {
    let _subject = authenticated_subject(&state, &headers).await?;
    let rows = sqlx::query_as::<_, PlaceOption>(
        "SELECT country,region,city FROM location_options
         ORDER BY country,region,city",
    )
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(rows))
}

/// Best-effort capture of a restaurant's location so the next person picking
/// a city sees it in the dropdown. Never fails the caller's request.
pub(crate) async fn record(state: &AppState, country: &str, region: &str, city: &str) {
    let country = country.trim();
    let region = region.trim();
    let city = city.trim();
    if country.is_empty() || region.is_empty() || city.is_empty() {
        return;
    }
    let result = sqlx::query(
        "INSERT INTO location_options(id,country,region,city)
         VALUES($1,$2,$3,$4) ON CONFLICT DO NOTHING",
    )
    .bind(Uuid::now_v7())
    .bind(country)
    .bind(region)
    .bind(city)
    .execute(&state.pool)
    .await;
    if let Err(error) = result {
        tracing::warn!(%error, %country, %region, %city, "could not record location option");
    }
}
