use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error};

#[derive(sqlx::FromRow)]
struct Member {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Supplier {
    id: Uuid,
    name: String,
    archived_at: Option<DateTime<Utc>>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SupplierInput {
    name: String,
}

#[derive(Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ListQuery {
    #[serde(default)]
    include_archived: bool,
}

async fn member(s: &AppState, h: &HeaderMap) -> Result<Member, ApiError> {
    let sub = authenticated_subject(s, h).await?;
    sqlx::query_as(
        "SELECT m.restaurant_id,u.id user_id,m.role FROM users u
         JOIN restaurant_memberships m ON m.user_id=u.id WHERE u.auth_subject=$1",
    )
    .bind(sub)
    .fetch_optional(&s.pool)
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

pub(crate) fn normalize_name(name: &str) -> Result<(String, String), ApiError> {
    let name = name.trim().to_owned();
    if name.is_empty() || name.chars().count() > 120 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Supplier name must be between 1 and 120 characters.",
        ));
    }
    let key = name.to_lowercase();
    Ok((name, key))
}

/// Find or create an active supplier by normalized name. Reactivates archived matches.
pub(crate) async fn ensure_supplier(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    user_id: Uuid,
    name: &str,
) -> Result<(Uuid, String), ApiError> {
    let (name, key) = normalize_name(name)?;
    if let Some(row) = sqlx::query_as::<_, (Uuid, String)>(
        "SELECT id,name FROM suppliers WHERE restaurant_id=$1 AND name_key=$2 FOR UPDATE",
    )
    .bind(restaurant_id)
    .bind(&key)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    {
        sqlx::query(
            "UPDATE suppliers SET name=$3,archived_at=NULL,updated_at=NOW()
             WHERE id=$1 AND restaurant_id=$2 AND (archived_at IS NOT NULL OR name IS DISTINCT FROM $3)",
        )
        .bind(row.0)
        .bind(restaurant_id)
        .bind(&name)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
        return Ok((row.0, name));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO suppliers(id,restaurant_id,name,name_key,created_by) VALUES($1,$2,$3,$4,$5)",
    )
    .bind(id)
    .bind(restaurant_id)
    .bind(&name)
    .bind(&key)
    .bind(user_id)
    .execute(&mut **tx)
    .await
    .map_err(|e| {
        if e.as_database_error()
            .and_then(|x| x.code())
            .is_some_and(|x| x == "23505")
        {
            ApiError(
                StatusCode::CONFLICT,
                "A supplier with this name already exists.",
            )
        } else {
            database_error(e)
        }
    })?;
    Ok((id, name))
}

pub(crate) async fn require_active_supplier(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    supplier_id: Uuid,
) -> Result<String, ApiError> {
    sqlx::query_scalar::<_, String>(
        "SELECT name FROM suppliers WHERE id=$1 AND restaurant_id=$2 AND archived_at IS NULL",
    )
    .bind(supplier_id)
    .bind(restaurant_id)
    .fetch_optional(&mut **tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Choose an active supplier from this restaurant.",
    ))
}

pub(crate) async fn list(
    State(s): State<AppState>,
    h: HeaderMap,
    Query(q): Query<ListQuery>,
) -> Result<Json<Vec<Supplier>>, ApiError> {
    let m = member(&s, &h).await?;
    let rows = if q.include_archived {
        sqlx::query_as::<_, Supplier>(
            "SELECT id,name,archived_at,created_at,updated_at FROM suppliers
             WHERE restaurant_id=$1 ORDER BY archived_at NULLS FIRST,LOWER(name),id",
        )
        .bind(m.restaurant_id)
        .fetch_all(&s.pool)
        .await
    } else {
        sqlx::query_as::<_, Supplier>(
            "SELECT id,name,archived_at,created_at,updated_at FROM suppliers
             WHERE restaurant_id=$1 AND archived_at IS NULL ORDER BY LOWER(name),id",
        )
        .bind(m.restaurant_id)
        .fetch_all(&s.pool)
        .await
    }
    .map_err(database_error)?;
    Ok(Json(rows))
}

pub(crate) async fn create(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(input): Json<SupplierInput>,
) -> Result<(StatusCode, Json<Supplier>), ApiError> {
    let m = member(&s, &h).await?;
    manager(&m)?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let (id, _) = ensure_supplier(&mut tx, m.restaurant_id, m.user_id, &input.name).await?;
    let row = sqlx::query_as::<_, Supplier>(
        "SELECT id,name,archived_at,created_at,updated_at FROM suppliers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(row)))
}

pub(crate) async fn update(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<SupplierInput>,
) -> Result<Json<Supplier>, ApiError> {
    let m = member(&s, &h).await?;
    manager(&m)?;
    let (name, key) = normalize_name(&input.name)?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let n = sqlx::query(
        "UPDATE suppliers SET name=$3,name_key=$4,updated_at=NOW()
         WHERE id=$1 AND restaurant_id=$2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(&name)
    .bind(&key)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if e.as_database_error()
            .and_then(|x| x.code())
            .is_some_and(|x| x == "23505")
        {
            ApiError(
                StatusCode::CONFLICT,
                "A supplier with this name already exists.",
            )
        } else {
            database_error(e)
        }
    })?
    .rows_affected();
    if n == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Supplier not found or already archived.",
        ));
    }
    // Keep free-text mirrors in sync for active mappings.
    sqlx::query(
        "UPDATE supplier_product_mappings SET supplier_name=$3,supplier_key=$4,updated_at=NOW()
         WHERE restaurant_id=$1 AND supplier_id=$2",
    )
    .bind(m.restaurant_id)
    .bind(id)
    .bind(&name)
    .bind(&key)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    let row = sqlx::query_as::<_, Supplier>(
        "SELECT id,name,archived_at,created_at,updated_at FROM suppliers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(row))
}

pub(crate) async fn archive(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Supplier>, ApiError> {
    let m = member(&s, &h).await?;
    manager(&m)?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let n = sqlx::query(
        "UPDATE suppliers SET archived_at=NOW(),updated_at=NOW()
         WHERE id=$1 AND restaurant_id=$2 AND archived_at IS NULL",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?
    .rows_affected();
    if n == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Supplier not found or already archived.",
        ));
    }
    sqlx::query(
        "UPDATE inventory_items SET preferred_supplier_id=NULL,updated_at=NOW()
         WHERE restaurant_id=$1 AND preferred_supplier_id=$2",
    )
    .bind(m.restaurant_id)
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    let row = sqlx::query_as::<_, Supplier>(
        "SELECT id,name,archived_at,created_at,updated_at FROM suppliers WHERE id=$1",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(row))
}
