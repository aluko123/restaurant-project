use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, QueryBuilder};
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
pub(crate) struct InventoryItem {
    id: Uuid,
    name: String,
    category: Option<String>,
    count_unit: String,
    par_level: Option<String>,
    active: bool,
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
    quantity: Option<String>,
}
#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct CountHeader {
    id: Uuid,
    status: String,
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
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct CompleteInput {
    confirm_missing: bool,
    revision: i64,
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
          l.latest::text latest_quantity,l.previous::text previous_quantity,
          CASE WHEN l.latest IS NOT NULL AND l.previous IS NOT NULL THEN (l.latest-l.previous)::text END change,
          l.counted last_counted_at,(l.latest IS NOT NULL AND i.par_level IS NOT NULL AND l.latest<i.par_level) low_stock
        FROM inventory_items i LEFT JOIN latest l ON l.inventory_item_id=i.id WHERE i.restaurant_id=$1
        ORDER BY i.active DESC,i.category NULLS LAST,LOWER(i.name),i.id")
        .bind(m.restaurant_id).fetch_all(&state.pool).await.map_err(database_error)?;
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
    let id = Uuid::now_v7();
    sqlx::query("INSERT INTO inventory_items(id,restaurant_id,name,category,count_unit,par_level,active) VALUES($1,$2,$3,$4,$5,$6,$7)")
        .bind(id).bind(m.restaurant_id).bind(&v.name).bind(&v.category).bind(&v.count_unit).bind(&v.par_level).bind(v.active)
        .execute(&state.pool).await.map_err(item_write_error)?;
    Ok((StatusCode::CREATED, Json(empty_item(id, v))))
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
    sqlx::query("UPDATE inventory_items SET name=$3,category=$4,count_unit=$5,par_level=$6,active=$7,updated_at=NOW()
      WHERE id=$1 AND restaurant_id=$2")
        .bind(id).bind(m.restaurant_id).bind(&v.name).bind(&v.category).bind(&v.count_unit).bind(&v.par_level).bind(v.active)
        .execute(&mut *tx).await.map_err(item_write_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(empty_item(id, v)))
}

pub(crate) async fn draft(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<DraftResponse>, ApiError> {
    let m = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let header = sqlx::query_as::<_, CountHeader>(
        "SELECT id,status,revision,created_at,updated_at,completed_at
         FROM inventory_count_sessions
         WHERE restaurant_id=$1 AND status='draft' FOR SHARE",
    )
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?;
    let count = match header {
        Some(header) => {
            let entries = load_entries(&mut *tx, header.id).await?;
            Some(count_from(header, entries))
        }
        None => None,
    };
    tx.commit().await.map_err(database_error)?;
    Ok(Json(DraftResponse { count }))
}

pub(crate) async fn start(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<(StatusCode, Json<Count>), ApiError> {
    let m = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let items=sqlx::query_as::<_,(Uuid,String,Option<String>,String)>("SELECT id,name,category,count_unit FROM inventory_items WHERE restaurant_id=$1 AND active ORDER BY category NULLS LAST,name,id FOR SHARE")
        .bind(m.restaurant_id).fetch_all(&mut *tx).await.map_err(database_error)?;
    if items.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Add an active inventory item before starting a count.",
        ));
    }
    let id = Uuid::now_v7();
    if let Err(e) = sqlx::query(
        "INSERT INTO inventory_count_sessions(id,restaurant_id,created_by) VALUES($1,$2,$3)",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(m.user_id)
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
    let mut q = QueryBuilder::<Postgres>::new(
        "INSERT INTO inventory_count_entries(id,session_id,inventory_item_id,name,category,count_unit) ",
    );
    q.push_values(items, |mut b, (item, name, category, unit)| {
        b.push_bind(Uuid::now_v7())
            .push_bind(id)
            .push_bind(item)
            .push_bind(name)
            .push_bind(category)
            .push_bind(unit);
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
    for (entry, quantity) in values {
        sqlx::query("UPDATE inventory_count_entries SET quantity=$2,updated_at=NOW() WHERE id=$1 AND session_id=$3").bind(entry).bind(quantity).bind(id).execute(&mut *tx).await.map_err(database_error)?;
    }
    sqlx::query("UPDATE inventory_count_sessions SET revision=revision+1,updated_at=clock_timestamp() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load_count(&state, id, m.restaurant_id).await?))
}

pub(crate) async fn complete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<CompleteInput>,
) -> Result<Json<Count>, ApiError> {
    let m = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
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
    let nulls = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM inventory_count_entries WHERE session_id=$1 AND quantity IS NULL",
    )
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if nulls > 0 && !input.confirm_missing {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Some quantities are missing. Confirm missing quantities to complete the count.",
        ));
    }
    let changed=sqlx::query("UPDATE inventory_count_sessions SET status='completed',revision=revision+1,completed_by=$3,completed_at=clock_timestamp(),updated_at=clock_timestamp() WHERE id=$1 AND restaurant_id=$2 AND status='draft'").bind(id).bind(m.restaurant_id).bind(m.user_id).execute(&mut *tx).await.map_err(database_error)?.rows_affected();
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
    let h=sqlx::query_as::<_,CountHeader>("SELECT id,status,revision,created_at,updated_at,completed_at FROM inventory_count_sessions WHERE id=$1 AND restaurant_id=$2").bind(id).bind(restaurant).fetch_optional(&state.pool).await.map_err(database_error)?.ok_or(ApiError(StatusCode::NOT_FOUND,"Inventory count not found."))?;
    let mut connection = state.pool.acquire().await.map_err(database_error)?;
    let entries = load_entries(&mut *connection, id).await?;
    Ok(count_from(h, entries))
}
async fn load_entries<'e, E>(executor: E, id: Uuid) -> Result<Vec<CountEntry>, ApiError>
where
    E: sqlx::Executor<'e, Database = Postgres>,
{
    sqlx::query_as::<_,CountEntry>("SELECT id,inventory_item_id,name,category,count_unit,quantity::text quantity FROM inventory_count_entries WHERE session_id=$1 ORDER BY category NULLS LAST,name,id").bind(id).fetch_all(executor).await.map_err(database_error)
}
fn count_from(h: CountHeader, entries: Vec<CountEntry>) -> Count {
    Count {
        id: h.id,
        status: h.status,
        revision: h.revision,
        created_at: h.created_at,
        updated_at: h.updated_at,
        completed_at: h.completed_at,
        entries,
    }
}
async fn membership(state: &AppState, headers: &HeaderMap) -> Result<Membership, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as("SELECT m.restaurant_id,u.id user_id,m.role FROM users u JOIN restaurant_memberships m ON m.user_id=u.id WHERE u.auth_subject=$1")
        .bind(subject).fetch_optional(&state.pool).await.map_err(database_error)?.ok_or(ApiError(StatusCode::FORBIDDEN,"A restaurant membership is required."))
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
fn empty_item(id: Uuid, v: ValidItem) -> InventoryItem {
    InventoryItem {
        id,
        name: v.name,
        category: v.category,
        count_unit: v.count_unit,
        par_level: v.par_level.map(|x| x.to_string()),
        active: v.active,
        latest_quantity: None,
        previous_quantity: None,
        change: None,
        last_counted_at: None,
        low_stock: false,
    }
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
            active: self.active,
        })
    }
}
fn parse_quantity(s: &str) -> Result<BigDecimal, ()> {
    let n = strict_decimal(s, 6).map_err(|_| ())?;
    if n < 0 { Err(()) } else { Ok(n) }
}
fn validate_save(input: SaveInput) -> Result<Vec<(Uuid, Option<BigDecimal>)>, ApiError> {
    let mut seen = HashSet::new();
    input.entries.into_iter().map(|e|{if !seen.insert(e.id){return Err(ApiError(StatusCode::UNPROCESSABLE_ENTITY,"Each count entry may appear only once."));} let q=e.quantity.as_deref().map(parse_quantity).transpose().map_err(|_|ApiError(StatusCode::UNPROCESSABLE_ENTITY,"Quantity must be null or a nonnegative decimal with at most 6 decimal places."))?;Ok((e.id,q))}).collect()
}
fn ensure_full_payload(
    expected: &[Uuid],
    actual: &[(Uuid, Option<BigDecimal>)],
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
    fn item(par: Option<&str>) -> ItemInput {
        ItemInput {
            name: "  Flour ".into(),
            category: Some("  Dry  ".into()),
            count_unit: " bag ".into(),
            par_level: par.map(Into::into),
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
                SaveEntry { id, quantity: None },
                SaveEntry {
                    id,
                    quantity: Some("1".into()),
                },
            ],
        };
        assert!(validate_save(duplicate).is_err());
        let values = vec![(id, None)];
        assert!(ensure_full_payload(&[id], &values).is_ok());
        assert!(ensure_full_payload(&[Uuid::now_v7()], &values).is_err());
    }
}
