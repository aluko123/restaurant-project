use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder, Transaction};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error, invoices::strict_decimal};

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct StorageArea {
    id: Uuid,
    name: String,
    sort_order: i32,
    active: bool,
    item_count: i64,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StorageAreaInput {
    name: String,
    #[serde(default = "yes")]
    active: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReorderAreasInput {
    area_ids: Vec<Uuid>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InventoryItem {
    id: Uuid,
    name: String,
    category: Option<String>,
    count_unit: String,
    par_level: Option<String>,
    active: bool,
    storage_area_id: Option<Uuid>,
    storage_area_name: Option<String>,
    shelf_order: i32,
    latest_quantity: Option<String>,
    previous_quantity: Option<String>,
    change: Option<String>,
    last_counted_at: Option<DateTime<Utc>>,
    low_stock: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ItemInput {
    pub(crate) name: String,
    pub(crate) category: Option<String>,
    pub(crate) count_unit: String,
    pub(crate) par_level: Option<String>,
    pub(crate) storage_area_id: Option<Uuid>,
    #[serde(default)]
    pub(crate) shelf_order: i32,
    #[serde(default = "yes")]
    pub(crate) active: bool,
}
fn yes() -> bool {
    true
}
pub(crate) struct ValidItem {
    pub(crate) name: String,
    pub(crate) category: Option<String>,
    pub(crate) count_unit: String,
    pub(crate) par_level: Option<BigDecimal>,
    pub(crate) storage_area_id: Option<Uuid>,
    pub(crate) shelf_order: i32,
    active: bool,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct CountEntry {
    id: Uuid,
    inventory_item_id: Uuid,
    name: String,
    category: Option<String>,
    count_unit: String,
    storage_area_name: Option<String>,
    storage_area_sort: i32,
    shelf_order: i32,
    previous_quantity: Option<String>,
    quantity: Option<String>,
    skipped: bool,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct CountHeader {
    id: Uuid,
    status: String,
    scope: String,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Count {
    id: Uuid,
    status: String,
    scope: String,
    storage_area_ids: Vec<Uuid>,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    entries: Vec<CountEntry>,
}

#[derive(Serialize)]
pub(crate) struct DraftResponse {
    count: Option<Count>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CountSummary {
    id: Uuid,
    status: String,
    scope: String,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    completed_at: Option<DateTime<Utc>>,
    entry_count: i64,
    counted_count: i64,
    skipped_count: i64,
    area_names: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct StartInput {
    #[serde(default)]
    storage_area_ids: Option<Vec<Uuid>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SaveInput {
    revision: i64,
    entries: Vec<SaveEntry>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SaveEntry {
    id: Uuid,
    quantity: Option<String>,
    #[serde(default)]
    skipped: bool,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompleteInput {
    #[serde(default)]
    confirm_skipped: bool,
    /// Backward-compatible alias used by older clients.
    #[serde(default)]
    confirm_missing: bool,
    revision: i64,
}

#[derive(Deserialize)]
pub(crate) struct HistoryQuery {
    #[serde(default = "default_history_limit")]
    limit: i64,
}
fn default_history_limit() -> i64 {
    20
}

pub(crate) async fn list_areas(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<StorageArea>>, ApiError> {
    let m = membership(&state, &headers).await?;
    let rows = sqlx::query_as::<_, StorageArea>(
        "SELECT a.id,a.name,a.sort_order,a.active,
                COALESCE((SELECT COUNT(*) FROM inventory_items i
                          WHERE i.restaurant_id=a.restaurant_id AND i.storage_area_id=a.id AND i.active),0)::bigint item_count
         FROM storage_areas a
         WHERE a.restaurant_id=$1
         ORDER BY a.active DESC,a.sort_order,LOWER(a.name),a.id",
    )
    .bind(m.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(rows))
}

pub(crate) async fn create_area(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<StorageAreaInput>,
) -> Result<(StatusCode, Json<StorageArea>), ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    let name = normalize_area_name(&input.name)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let sort = sqlx::query_scalar::<_, i32>(
        "SELECT COALESCE(MAX(sort_order),-1)+1 FROM storage_areas WHERE restaurant_id=$1",
    )
    .bind(m.restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO storage_areas(id,restaurant_id,name,sort_order,active) VALUES($1,$2,$3,$4,$5)",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(&name)
    .bind(sort)
    .bind(input.active)
    .execute(&mut *tx)
    .await
    .map_err(area_write_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(StorageArea {
            id,
            name,
            sort_order: sort,
            active: input.active,
            item_count: 0,
        }),
    ))
}

pub(crate) async fn update_area(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<StorageAreaInput>,
) -> Result<Json<StorageArea>, ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    let name = normalize_area_name(&input.name)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let sort = sqlx::query_scalar::<_, i32>(
        "UPDATE storage_areas SET name=$3,active=$4,updated_at=NOW()
         WHERE id=$1 AND restaurant_id=$2
         RETURNING sort_order",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(&name)
    .bind(input.active)
    .fetch_optional(&mut *tx)
    .await
    .map_err(area_write_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Storage area not found."))?;
    let item_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM inventory_items WHERE restaurant_id=$1 AND storage_area_id=$2 AND active",
    )
    .bind(m.restaurant_id)
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(StorageArea {
        id,
        name,
        sort_order: sort,
        active: input.active,
        item_count,
    }))
}

pub(crate) async fn reorder_areas(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ReorderAreasInput>,
) -> Result<Json<Vec<StorageArea>>, ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    if input.area_ids.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Provide the storage areas in the order you walk them.",
        ));
    }
    let mut seen = HashSet::new();
    if input.area_ids.iter().any(|id| !seen.insert(*id)) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Each storage area may appear only once in the walk order.",
        ));
    }
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM storage_areas WHERE restaurant_id=$1 ORDER BY sort_order,name,id FOR UPDATE",
    )
    .bind(m.restaurant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(database_error)?;
    let existing_set: HashSet<_> = existing.iter().copied().collect();
    if existing_set.len() != input.area_ids.len()
        || input.area_ids.iter().any(|id| !existing_set.contains(id))
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Include every storage area exactly once when reordering.",
        ));
    }
    for (index, area_id) in input.area_ids.iter().enumerate() {
        sqlx::query(
            "UPDATE storage_areas SET sort_order=$3,updated_at=NOW() WHERE id=$1 AND restaurant_id=$2",
        )
        .bind(area_id)
        .bind(m.restaurant_id)
        .bind(index as i32)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    }
    tx.commit().await.map_err(database_error)?;
    list_areas(State(state), headers).await
}

pub(crate) async fn list_items(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<InventoryItem>>, ApiError> {
    let m = membership(&state, &headers).await?;
    let rows = sqlx::query_as::<_, InventoryItem>(
        "WITH history AS (SELECT e.inventory_item_id,e.quantity,s.completed_at,
          ROW_NUMBER() OVER (PARTITION BY e.inventory_item_id ORDER BY s.completed_at DESC,s.id DESC) n
          FROM inventory_count_entries e JOIN inventory_count_sessions s ON s.id=e.session_id
          WHERE s.restaurant_id=$1 AND s.status='completed' AND e.quantity IS NOT NULL),
        latest AS (SELECT inventory_item_id,MAX(quantity) FILTER(WHERE n=1) latest,
          MAX(quantity) FILTER(WHERE n=2) previous,MAX(completed_at) FILTER(WHERE n=1) counted FROM history WHERE n<=2 GROUP BY inventory_item_id)
        SELECT i.id,i.name,i.category,i.count_unit,i.par_level::text par_level,i.active,
          i.storage_area_id,a.name storage_area_name,i.shelf_order,
          l.latest::text latest_quantity,l.previous::text previous_quantity,
          CASE WHEN l.latest IS NOT NULL AND l.previous IS NOT NULL THEN (l.latest-l.previous)::text END change,
          l.counted last_counted_at,(l.latest IS NOT NULL AND i.par_level IS NOT NULL AND l.latest<i.par_level) low_stock
        FROM inventory_items i
        LEFT JOIN storage_areas a ON a.id=i.storage_area_id
        LEFT JOIN latest l ON l.inventory_item_id=i.id
        WHERE i.restaurant_id=$1
        ORDER BY i.active DESC,
          CASE WHEN a.id IS NULL THEN 1 ELSE 0 END,
          a.sort_order NULLS LAST,i.shelf_order,i.category NULLS LAST,LOWER(i.name),i.id",
    )
    .bind(m.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(rows))
}

pub(crate) async fn create_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<ItemInput>,
) -> Result<(StatusCode, Json<InventoryItem>), ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    let v = input.validated()?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let area_name = validate_area(&mut tx, m.restaurant_id, v.storage_area_id).await?;
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO inventory_items(id,restaurant_id,name,category,count_unit,par_level,active,storage_area_id,shelf_order)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(&v.name)
    .bind(&v.category)
    .bind(&v.count_unit)
    .bind(&v.par_level)
    .bind(v.active)
    .bind(v.storage_area_id)
    .bind(v.shelf_order)
    .execute(&mut *tx)
    .await
    .map_err(item_write_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(empty_item(id, v, area_name))))
}

pub(crate) async fn update_item(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ItemInput>,
) -> Result<Json<InventoryItem>, ApiError> {
    let m = membership(&state, &headers).await?;
    require_manager(&m)?;
    let v = input.validated()?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let current_unit = sqlx::query_scalar::<_, String>(
        "SELECT count_unit FROM inventory_items WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Inventory item not found."))?;
    if current_unit != v.count_unit {
        let unit_in_use = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(
                SELECT 1 FROM inventory_count_entries e
                JOIN inventory_count_sessions s ON s.id=e.session_id
                WHERE e.inventory_item_id=$1
                  AND (s.status='draft' OR (s.status='completed' AND e.quantity IS NOT NULL))
                UNION ALL
                SELECT 1 FROM supplier_product_mappings WHERE inventory_item_id=$1
                UNION ALL
                SELECT 1 FROM menu_item_ingredients WHERE inventory_item_id=$1
                UNION ALL
                SELECT 1 FROM loss_events WHERE inventory_item_id=$1
             )",
        )
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if unit_in_use {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Count unit cannot change while a count, saved supplier purchase, menu ingredient setup, or loss log uses this item.",
            ));
        }
    }
    let area_name = validate_area(&mut tx, m.restaurant_id, v.storage_area_id).await?;
    sqlx::query(
        "UPDATE inventory_items SET name=$3,category=$4,count_unit=$5,par_level=$6,active=$7,
         storage_area_id=$8,shelf_order=$9,updated_at=NOW()
         WHERE id=$1 AND restaurant_id=$2",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(&v.name)
    .bind(&v.category)
    .bind(&v.count_unit)
    .bind(&v.par_level)
    .bind(v.active)
    .bind(v.storage_area_id)
    .bind(v.shelf_order)
    .execute(&mut *tx)
    .await
    .map_err(item_write_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(empty_item(id, v, area_name)))
}

pub(crate) async fn draft(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DraftResponse>, ApiError> {
    let m = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let header = sqlx::query_as::<_, CountHeader>(
        "SELECT id,status,scope,revision,created_at,updated_at,completed_at
         FROM inventory_count_sessions
         WHERE restaurant_id=$1 AND status='draft' FOR SHARE",
    )
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?;
    let count = match header {
        Some(header) => Some(load_count_tx(&mut tx, header).await?),
        None => None,
    };
    tx.commit().await.map_err(database_error)?;
    Ok(Json(DraftResponse { count }))
}

pub(crate) async fn list_counts(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<HistoryQuery>,
) -> Result<Json<Vec<CountSummary>>, ApiError> {
    let m = membership(&state, &headers).await?;
    let limit = query.limit.clamp(1, 50);
    let rows = sqlx::query_as::<_, CountSummary>(
        "SELECT s.id,s.status,s.scope,s.revision,s.created_at,s.updated_at,s.completed_at,
                (SELECT COUNT(*) FROM inventory_count_entries e WHERE e.session_id=s.id)::bigint entry_count,
                (SELECT COUNT(*) FROM inventory_count_entries e WHERE e.session_id=s.id AND e.quantity IS NOT NULL)::bigint counted_count,
                (SELECT COUNT(*) FROM inventory_count_entries e WHERE e.session_id=s.id AND e.skipped)::bigint skipped_count,
                (SELECT string_agg(a.name, ', ' ORDER BY a.sort_order, a.name)
                   FROM inventory_count_session_areas sa
                   JOIN storage_areas a ON a.id=sa.storage_area_id
                  WHERE sa.session_id=s.id) area_names
         FROM inventory_count_sessions s
         WHERE s.restaurant_id=$1 AND s.status='completed'
         ORDER BY s.completed_at DESC,s.id DESC
         LIMIT $2",
    )
    .bind(m.restaurant_id)
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(rows))
}

pub(crate) async fn get_count(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Count>, ApiError> {
    let m = membership(&state, &headers).await?;
    load_count(&state, id, m.restaurant_id).await.map(Json)
}

pub(crate) async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Option<Json<StartInput>>,
) -> Result<(StatusCode, Json<Count>), ApiError> {
    let m = membership(&state, &headers).await?;
    let area_ids = body
        .map(|Json(input)| input.storage_area_ids.unwrap_or_default())
        .unwrap_or_default();
    let mut unique_areas = Vec::new();
    let mut seen = HashSet::new();
    for id in area_ids {
        if seen.insert(id) {
            unique_areas.push(id);
        }
    }
    let scope = if unique_areas.is_empty() {
        "all"
    } else {
        "areas"
    };

    let mut tx = state.pool.begin().await.map_err(database_error)?;
    if scope == "areas" {
        let found = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM storage_areas WHERE restaurant_id=$1 AND active AND id = ANY($2)",
        )
        .bind(m.restaurant_id)
        .bind(&unique_areas)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        if found.len() != unique_areas.len() {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose active storage areas from this restaurant.",
            ));
        }
    }

    #[derive(sqlx::FromRow)]
    struct ItemRow {
        id: Uuid,
        name: String,
        category: Option<String>,
        count_unit: String,
        storage_area_name: Option<String>,
        storage_area_sort: i32,
        shelf_order: i32,
        previous_quantity: Option<BigDecimal>,
    }

    let items = if scope == "areas" {
        sqlx::query_as::<_, ItemRow>(
            "WITH last AS (
               SELECT DISTINCT ON (e.inventory_item_id) e.inventory_item_id, e.quantity
               FROM inventory_count_entries e
               JOIN inventory_count_sessions s ON s.id=e.session_id
               WHERE s.restaurant_id=$1 AND s.status='completed' AND e.quantity IS NOT NULL
               ORDER BY e.inventory_item_id, s.completed_at DESC, s.id DESC
             )
             SELECT i.id,i.name,i.category,i.count_unit,a.name storage_area_name,
                    a.sort_order storage_area_sort,i.shelf_order,l.quantity previous_quantity
             FROM inventory_items i
             JOIN storage_areas a ON a.id=i.storage_area_id
             LEFT JOIN last l ON l.inventory_item_id=i.id
             WHERE i.restaurant_id=$1 AND i.active AND i.storage_area_id = ANY($2)
             ORDER BY a.sort_order,i.shelf_order,LOWER(i.name),i.id",
        )
        .bind(m.restaurant_id)
        .bind(&unique_areas)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?
    } else {
        sqlx::query_as::<_, ItemRow>(
            "WITH last AS (
               SELECT DISTINCT ON (e.inventory_item_id) e.inventory_item_id, e.quantity
               FROM inventory_count_entries e
               JOIN inventory_count_sessions s ON s.id=e.session_id
               WHERE s.restaurant_id=$1 AND s.status='completed' AND e.quantity IS NOT NULL
               ORDER BY e.inventory_item_id, s.completed_at DESC, s.id DESC
             )
             SELECT i.id,i.name,i.category,i.count_unit,a.name storage_area_name,
                    COALESCE(a.sort_order, 1000000) storage_area_sort,i.shelf_order,
                    l.quantity previous_quantity
             FROM inventory_items i
             LEFT JOIN storage_areas a ON a.id=i.storage_area_id
             LEFT JOIN last l ON l.inventory_item_id=i.id
             WHERE i.restaurant_id=$1 AND i.active
             ORDER BY COALESCE(a.sort_order, 1000000), i.shelf_order,LOWER(i.name),i.id",
        )
        .bind(m.restaurant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?
    };

    if items.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            if scope == "areas" {
                "No active items are assigned to the selected storage areas."
            } else {
                "Add an active inventory item before starting a count."
            },
        ));
    }

    let id = Uuid::now_v7();
    if let Err(e) = sqlx::query(
        "INSERT INTO inventory_count_sessions(id,restaurant_id,created_by,scope) VALUES($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(m.user_id)
    .bind(scope)
    .execute(&mut *tx)
    .await
    {
        return Err(if unique(&e) {
            ApiError(
                StatusCode::CONFLICT,
                "A draft inventory count already exists.",
            )
        } else {
            database_error(e)
        });
    }

    if scope == "areas" {
        let mut q = QueryBuilder::<Postgres>::new(
            "INSERT INTO inventory_count_session_areas(session_id,storage_area_id) ",
        );
        q.push_values(&unique_areas, |mut b, area| {
            b.push_bind(id).push_bind(*area);
        });
        q.build().execute(&mut *tx).await.map_err(database_error)?;
    }

    let mut q = QueryBuilder::<Postgres>::new(
        "INSERT INTO inventory_count_entries(id,session_id,inventory_item_id,name,category,count_unit,storage_area_name,storage_area_sort,shelf_order,previous_quantity) ",
    );
    q.push_values(items, |mut b, item| {
        b.push_bind(Uuid::now_v7())
            .push_bind(id)
            .push_bind(item.id)
            .push_bind(item.name)
            .push_bind(item.category)
            .push_bind(item.count_unit)
            .push_bind(item.storage_area_name)
            .push_bind(item.storage_area_sort)
            .push_bind(item.shelf_order)
            .push_bind(item.previous_quantity);
    });
    q.build().execute(&mut *tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(load_count(&state, id, m.restaurant_id).await?),
    ))
}

pub(crate) async fn save(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<SaveInput>,
) -> Result<Json<Count>, ApiError> {
    let m = membership(&state, &headers).await?;
    let revision = input.revision;
    let values = validate_save(input)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let session = sqlx::query_as::<_, (String, i64)>(
        "SELECT status,revision FROM inventory_count_sessions WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Inventory count not found.",
    ))?;
    if session.0 != "draft" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a draft inventory count can be saved.",
        ));
    }
    if session.1 != revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This draft changed on another device. Reload it before saving.",
        ));
    }
    let expected =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM inventory_count_entries WHERE session_id=$1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(database_error)?;
    ensure_full_payload(&expected, &values)?;
    for (entry, quantity, skipped) in values {
        sqlx::query(
            "UPDATE inventory_count_entries SET quantity=$2,skipped=$3,updated_at=NOW()
             WHERE id=$1 AND session_id=$4",
        )
        .bind(entry)
        .bind(quantity)
        .bind(skipped)
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    }
    sqlx::query(
        "UPDATE inventory_count_sessions SET revision=revision+1,updated_at=clock_timestamp() WHERE id=$1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    let header = sqlx::query_as::<_, CountHeader>(
        "SELECT id,status,scope,revision,created_at,updated_at,completed_at
         FROM inventory_count_sessions WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    let count = load_count_tx(&mut tx, header).await?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(count))
}

pub(crate) async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<CompleteInput>,
) -> Result<Json<Count>, ApiError> {
    let m = membership(&state, &headers).await?;
    let confirm = input.confirm_skipped || input.confirm_missing;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(m.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let session = sqlx::query_as::<_, (String, i64)>(
        "SELECT status,revision FROM inventory_count_sessions WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Inventory count not found."))?;
    if session.0 != "draft" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a draft inventory count can be completed.",
        ));
    }
    if session.1 != input.revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This draft changed after you reviewed it. Reload and review it again.",
        ));
    }
    let open = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM inventory_count_entries
         WHERE session_id=$1 AND quantity IS NULL AND NOT skipped",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if open > 0 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Finish or skip every item before completing this count.",
        ));
    }
    let skipped = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM inventory_count_entries WHERE session_id=$1 AND skipped",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if skipped > 0 && !confirm {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Some items were skipped. Confirm skipped items to complete the count.",
        ));
    }
    let changed = sqlx::query(
        "UPDATE inventory_count_sessions SET status='completed',revision=revision+1,
         completed_by=$3,completed_at=clock_timestamp(),updated_at=clock_timestamp()
         WHERE id=$1 AND restaurant_id=$2 AND status='draft'",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(m.user_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a draft inventory count can be completed.",
        ));
    }
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load_count(&state, id, m.restaurant_id).await?))
}

async fn load_count(state: &AppState, id: Uuid, restaurant: Uuid) -> Result<Count, ApiError> {
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let h = sqlx::query_as::<_, CountHeader>(
        "SELECT id,status,scope,revision,created_at,updated_at,completed_at
         FROM inventory_count_sessions WHERE id=$1 AND restaurant_id=$2",
    )
    .bind(id)
    .bind(restaurant)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Inventory count not found.",
    ))?;
    let count = load_count_tx(&mut tx, h).await?;
    tx.commit().await.map_err(database_error)?;
    Ok(count)
}

async fn load_count_tx(
    tx: &mut Transaction<'_, Postgres>,
    h: CountHeader,
) -> Result<Count, ApiError> {
    let storage_area_ids = sqlx::query_scalar::<_, Uuid>(
        "SELECT storage_area_id FROM inventory_count_session_areas WHERE session_id=$1 ORDER BY storage_area_id",
    )
    .bind(h.id)
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?;
    let entries = load_entries(&mut **tx, h.id).await?;
    Ok(Count {
        id: h.id,
        status: h.status,
        scope: h.scope,
        storage_area_ids,
        revision: h.revision,
        created_at: h.created_at,
        updated_at: h.updated_at,
        completed_at: h.completed_at,
        entries,
    })
}

async fn load_entries<'e, E>(executor: E, id: Uuid) -> Result<Vec<CountEntry>, ApiError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    sqlx::query_as::<_, CountEntry>(
        "SELECT id,inventory_item_id,name,category,count_unit,storage_area_name,storage_area_sort,shelf_order,
                previous_quantity::text previous_quantity,quantity::text quantity,skipped
         FROM inventory_count_entries
         WHERE session_id=$1
         ORDER BY storage_area_sort,shelf_order,LOWER(name),id",
    )
    .bind(id)
    .fetch_all(executor)
    .await
    .map_err(database_error)
}

async fn validate_area(
    tx: &mut Transaction<'_, Postgres>,
    restaurant: Uuid,
    area_id: Option<Uuid>,
) -> Result<Option<String>, ApiError> {
    let Some(area_id) = area_id else {
        return Ok(None);
    };
    let name = sqlx::query_scalar::<_, String>(
        "SELECT name FROM storage_areas WHERE id=$1 AND restaurant_id=$2 AND active",
    )
    .bind(area_id)
    .bind(restaurant)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Choose an active storage area from this restaurant.",
    ))?;
    Ok(Some(name))
}

async fn membership(state: &AppState, headers: &HeaderMap) -> Result<Membership, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as(
        "SELECT m.restaurant_id,u.id user_id,m.role FROM users u JOIN restaurant_memberships m ON m.user_id=u.id WHERE u.auth_subject=$1",
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

fn require_manager(m: &Membership) -> Result<(), ApiError> {
    if matches!(m.role.as_str(), "owner" | "manager") {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Owner or manager access is required.",
        ))
    }
}

fn unique(e: &sqlx::Error) -> bool {
    e.as_database_error()
        .and_then(|e| e.code())
        .is_some_and(|c| c == "23505")
}

pub(crate) fn item_write_error(e: sqlx::Error) -> ApiError {
    if unique(&e) {
        ApiError(
            StatusCode::CONFLICT,
            "That inventory item is already in Parline.",
        )
    } else {
        database_error(e)
    }
}

fn area_write_error(e: sqlx::Error) -> ApiError {
    if unique(&e) {
        ApiError(
            StatusCode::CONFLICT,
            "That storage area name is already in use.",
        )
    } else {
        database_error(e)
    }
}

fn empty_item(id: Uuid, v: ValidItem, area_name: Option<String>) -> InventoryItem {
    InventoryItem {
        id,
        name: v.name,
        category: v.category,
        count_unit: v.count_unit,
        par_level: v.par_level.map(|x| x.to_string()),
        active: v.active,
        storage_area_id: v.storage_area_id,
        storage_area_name: area_name,
        shelf_order: v.shelf_order,
        latest_quantity: None,
        previous_quantity: None,
        change: None,
        last_counted_at: None,
        low_stock: false,
    }
}

fn normalize_area_name(name: &str) -> Result<String, ApiError> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 40 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Storage area name must be between 1 and 40 characters.",
        ));
    }
    Ok(name)
}

impl ItemInput {
    pub(crate) fn validated(mut self) -> Result<ValidItem, ApiError> {
        self.name = self.name.trim().to_owned();
        self.count_unit = self.count_unit.trim().to_owned();
        self.category = self.category.and_then(|x| {
            let x = x.trim();
            (!x.is_empty()).then(|| x.to_owned())
        });
        if self.name.is_empty() || self.name.chars().count() > 50 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Inventory item name must be between 1 and 50 characters.",
            ));
        }
        if self
            .category
            .as_ref()
            .is_some_and(|x| x.chars().count() > 20)
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Inventory item category must be no more than 20 characters.",
            ));
        }
        if self.count_unit.is_empty() || self.count_unit.chars().count() > 20 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Count unit must be between 1 and 20 characters.",
            ));
        }
        if self.shelf_order < 0 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Shelf order must be zero or greater.",
            ));
        }
        let par = self
            .par_level
            .as_deref()
            .map(parse_quantity)
            .transpose()
            .map_err(|_| {
                ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Par level must be a nonnegative decimal with at most 6 decimal places.",
                )
            })?;
        Ok(ValidItem {
            name: self.name,
            category: self.category,
            count_unit: self.count_unit,
            par_level: par,
            storage_area_id: self.storage_area_id,
            shelf_order: self.shelf_order,
            active: self.active,
        })
    }
}

fn parse_quantity(s: &str) -> Result<BigDecimal, ()> {
    let n = strict_decimal(s, 6).map_err(|_| ())?;
    if n < 0 { Err(()) } else { Ok(n) }
}

fn validate_save(input: SaveInput) -> Result<Vec<(Uuid, Option<BigDecimal>, bool)>, ApiError> {
    let mut seen = HashSet::new();
    input
        .entries
        .into_iter()
        .map(|e| {
            if !seen.insert(e.id) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Each count entry may appear only once.",
                ));
            }
            let q = e
                .quantity
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(parse_quantity)
                .transpose()
                .map_err(|_| {
                    ApiError(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Quantity must be null or a nonnegative decimal with at most 6 decimal places.",
                    )
                })?;
            if e.skipped && q.is_some() {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "A skipped item cannot also have a quantity.",
                ));
            }
            Ok((e.id, q, e.skipped))
        })
        .collect()
}

fn ensure_full_payload(
    expected: &[Uuid],
    actual: &[(Uuid, Option<BigDecimal>, bool)],
) -> Result<(), ApiError> {
    let got: HashSet<_> = actual.iter().map(|x| x.0).collect();
    if got.len() != expected.len() || expected.iter().any(|id| !got.contains(id)) {
        Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Submit every entry in this inventory count, with no extra entries.",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Large swing vs previous count: at least 1 unit and ≥25% of previous.
    fn is_big_change(previous: Option<&str>, quantity: Option<&str>) -> bool {
        let (Some(prev), Some(qty)) = (
            previous.and_then(|s| s.parse::<f64>().ok()),
            quantity.and_then(|s| s.parse::<f64>().ok()),
        ) else {
            return false;
        };
        let delta = (qty - prev).abs();
        delta >= 1.0 && delta >= prev.abs() * 0.25
    }

    fn item(par: Option<&str>) -> ItemInput {
        ItemInput {
            name: "  Flour ".into(),
            category: Some("  Dry  ".into()),
            count_unit: " bag ".into(),
            par_level: par.map(Into::into),
            storage_area_id: None,
            shelf_order: 0,
            active: true,
        }
    }
    #[test]
    fn normalizes_item_and_par() {
        let x = item(Some("0.123456")).validated().unwrap();
        assert_eq!(x.name, "Flour");
        assert_eq!(x.category.as_deref(), Some("Dry"));
        assert_eq!(x.count_unit, "bag");
        assert!(item(Some("-1")).validated().is_err());
        assert!(item(Some("1.1234567")).validated().is_err());
    }
    #[test]
    fn validates_quantities() {
        assert!(parse_quantity("0").is_ok());
        assert!(parse_quantity("-0.1").is_err());
        assert!(parse_quantity("1e2").is_err());
    }
    #[test]
    fn validates_duplicate_and_full_payload() {
        let id = Uuid::now_v7();
        let duplicate = SaveInput {
            revision: 0,
            entries: vec![
                SaveEntry {
                    id,
                    quantity: None,
                    skipped: false,
                },
                SaveEntry {
                    id,
                    quantity: Some("1".into()),
                    skipped: false,
                },
            ],
        };
        assert!(validate_save(duplicate).is_err());
        let values = vec![(id, None, false)];
        assert!(ensure_full_payload(&[id], &values).is_ok());
        assert!(ensure_full_payload(&[Uuid::now_v7()], &values).is_err());
    }
    #[test]
    fn rejects_skipped_with_quantity() {
        let id = Uuid::now_v7();
        let input = SaveInput {
            revision: 0,
            entries: vec![SaveEntry {
                id,
                quantity: Some("1".into()),
                skipped: true,
            }],
        };
        assert!(validate_save(input).is_err());
    }
    #[test]
    fn flags_big_changes() {
        assert!(is_big_change(Some("10"), Some("4")));
        assert!(!is_big_change(Some("10"), Some("9")));
        assert!(!is_big_change(None, Some("4")));
    }
}
