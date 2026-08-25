use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::inventory::{membership, require_manager};
use crate::{ApiError, AppState, database_error};

const MAX_ITEMS: usize = 500;
const MAX_NAME_CHARS: usize = 60;
const PREVIEW_NAMES: usize = 5;

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Template {
    id: Uuid,
    name: String,
    item_count: i64,
    preview_names: Vec<String>,
    created_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CreateTemplate {
    name: String,
    item_ids: Vec<Uuid>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Template>>, ApiError> {
    let m = membership(&state, &headers).await?;
    let mut templates = sqlx::query_as::<_, (Uuid, String, i64, DateTime<Utc>)>(
        "SELECT t.id,t.name,
                (SELECT COUNT(*) FROM count_template_items ti
                 WHERE ti.template_id=t.id)::bigint item_count,
                t.created_at
         FROM count_templates t WHERE t.restaurant_id=$1
         ORDER BY t.created_at DESC,t.id DESC",
    )
    .bind(m.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    let ids: Vec<Uuid> = templates.iter().map(|(id, ..)| *id).collect();
    let previews = if ids.is_empty() {
        Vec::new()
    } else {
        sqlx::query_as::<_, (Uuid, String)>(
            "SELECT template_id,name FROM (
                 SELECT ti.template_id,i.name,
                        ROW_NUMBER() OVER (PARTITION BY ti.template_id ORDER BY ti.position,i.name) rank
                 FROM count_template_items ti
                 JOIN inventory_items i ON i.id=ti.inventory_item_id
                 WHERE ti.template_id = ANY($1)
             ) ranked WHERE rank <= $2",
        )
        .bind(&ids)
        .bind(PREVIEW_NAMES as i32)
        .fetch_all(&state.pool)
        .await
        .map_err(database_error)?
    };
    Ok(Json(
        templates
            .drain(..)
            .map(|(id, name, item_count, created_at)| Template {
                preview_names: previews
                    .iter()
                    .filter(|(template_id, _)| *template_id == id)
                    .map(|(_, name)| name.clone())
                    .collect(),
                id,
                name,
                item_count,
                created_at,
            })
            .collect(),
    ))
}

pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<CreateTemplate>,
) -> Result<(StatusCode, Json<Template>), ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    let name = input.name.trim().to_owned();
    if name.is_empty() || name.chars().count() > MAX_NAME_CHARS {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Give the template a name up to 60 characters.",
        ));
    }
    // Dedupe while keeping the caller's order.
    let mut seen = std::collections::HashSet::new();
    let item_ids: Vec<Uuid> = input
        .item_ids
        .into_iter()
        .filter(|id| seen.insert(*id))
        .collect();
    if item_ids.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose at least one item to save in this template.",
        ));
    }
    if item_ids.len() > MAX_ITEMS {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "A template can hold at most 500 items.",
        ));
    }

    let mut tx = state.pool.begin().await.map_err(database_error)?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(m.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let taken = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM count_templates
         WHERE restaurant_id=$1 AND LOWER(BTRIM(name))=LOWER($2))",
    )
    .bind(m.restaurant_id)
    .bind(&name)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if taken {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A template with this name already exists.",
        ));
    }
    let found = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM inventory_items WHERE restaurant_id=$1 AND active AND id = ANY($2)",
    )
    .bind(m.restaurant_id)
    .bind(&item_ids)
    .fetch_all(&mut *tx)
    .await
    .map_err(database_error)?;
    if found.len() != item_ids.len() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose active inventory items from this restaurant.",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO count_templates(id,restaurant_id,name,created_by) VALUES($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(&name)
    .bind(m.user_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    for (position, item_id) in item_ids.iter().enumerate() {
        sqlx::query("INSERT INTO count_template_items(template_id,inventory_item_id,position) VALUES($1,$2,$3)")
            .bind(id)
            .bind(item_id)
            .bind(position as i32)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
    }
    tx.commit().await.map_err(database_error)?;

    Ok((
        StatusCode::CREATED,
        Json(Template {
            id,
            name,
            item_count: item_ids.len() as i64,
            preview_names: Vec::new(),
            created_at: Utc::now(),
        }),
    ))
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    let n = sqlx::query("DELETE FROM count_templates WHERE id=$1 AND restaurant_id=$2")
        .bind(id)
        .bind(m.restaurant_id)
        .execute(&state.pool)
        .await
        .map_err(database_error)?
        .rows_affected();
    if n == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "This template doesn't exist.",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

/// Loads one template's saved item ids for starting a focused count.
/// Inactive or removed items are skipped rather than failing the whole start;
/// `inventory::start` still validates whatever remains.
pub(crate) async fn template_item_ids(
    state: &AppState,
    restaurant_id: Uuid,
    template_id: Uuid,
) -> Result<Vec<Uuid>, ApiError> {
    sqlx::query_scalar(
        "SELECT ti.inventory_item_id FROM count_template_items ti
         JOIN inventory_items i ON i.id=ti.inventory_item_id
         WHERE ti.template_id=$1 AND i.restaurant_id=$2 AND i.active
         ORDER BY ti.position,i.name,i.id",
    )
    .bind(template_id)
    .bind(restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)
}
