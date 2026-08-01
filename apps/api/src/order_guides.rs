use crate::{ApiError, AppState, authenticated_subject, database_error};
use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::{BigDecimal, RoundingMode};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct Member {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}
#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Line {
    id: Uuid,
    inventory_item_id: Uuid,
    inventory_item_name: String,
    count_unit: String,
    counted_quantity: String,
    par_level: String,
    shortage: String,
    supplier_mapping_id: Option<Uuid>,
    supplier_id: Option<Uuid>,
    supplier_name: Option<String>,
    product_description: Option<String>,
    supplier_sku: Option<String>,
    order_unit: String,
    conversion: String,
    suggested_order_quantity: String,
    order_quantity: String,
    received_quantity: Option<String>,
    receipt_status: Option<String>,
    discrepancy_kind: Option<String>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Guide {
    id: Uuid,
    source_count_id: Uuid,
    status: String,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    ordered_at: Option<DateTime<Utc>>,
    received_at: Option<DateTime<Utc>>,
    cancelled_at: Option<DateTime<Utc>>,
    linked_invoice_id: Option<Uuid>,
    linked_invoice_supplier_name: Option<String>,
    linked_invoice_date: Option<chrono::NaiveDate>,
    lines: Vec<Line>,
}
#[derive(sqlx::FromRow)]
struct Header {
    id: Uuid,
    source_count_id: Uuid,
    status: String,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    ordered_at: Option<DateTime<Utc>>,
    received_at: Option<DateTime<Utc>>,
    cancelled_at: Option<DateTime<Utc>>,
    linked_invoice_id: Option<Uuid>,
    linked_invoice_supplier_name: Option<String>,
    linked_invoice_date: Option<chrono::NaiveDate>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Create {
    count_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Update {
    revision: i64,
    lines: Vec<Edit>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Edit {
    id: Uuid,
    #[serde(default)]
    supplier_id: Option<Uuid>,
    supplier_name: Option<String>,
    order_unit: String,
    conversion: String,
    order_quantity: String,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Receive {
    lines: Vec<Received>,
    #[serde(default)]
    linked_invoice_id: Option<Uuid>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RevisionInput {
    revision: i64,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct Received {
    id: Uuid,
    received_quantity: String,
}

async fn member(s: &AppState, h: &HeaderMap) -> Result<Member, ApiError> {
    let sub = authenticated_subject(s, h).await?;
    sqlx::query_as("SELECT m.restaurant_id,u.id user_id,m.role FROM users u JOIN restaurant_memberships m ON m.user_id=u.id WHERE u.auth_subject=$1").bind(sub).fetch_optional(&s.pool).await.map_err(database_error)?.ok_or(ApiError(StatusCode::FORBIDDEN,"A restaurant membership is required."))
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
fn positive(v: &str) -> Result<BigDecimal, ApiError> {
    let x = crate::invoices::strict_decimal_with_precision(v, 30, 12).map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Quantities and conversion must be positive decimals with at most 12 decimal places.",
        )
    })?;
    if x <= 0 {
        Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Quantities and conversion must be positive decimals with at most 12 decimal places.",
        ))
    } else {
        Ok(x)
    }
}
fn represented(value: BigDecimal) -> Result<BigDecimal, ApiError> {
    let rounded = value.with_scale_round(12, RoundingMode::HalfEven);
    // NUMERIC(30,12): at most 18 integer digits after deterministic representation rounding.
    let represented = crate::invoices::strict_decimal_with_precision(&rounded.to_string(), 30, 12)
        .map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Calculated order quantity is out of range.",
            )
        })?;
    if represented <= 0 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Calculated order quantity is out of range.",
        ));
    }
    Ok(represented)
}
fn shortage(par: &BigDecimal, count: &BigDecimal) -> BigDecimal {
    let x = par - count;
    if x > 0 { x } else { BigDecimal::from(0) }
}
fn discrepancy_kind(ordered: &BigDecimal, received: &BigDecimal) -> &'static str {
    if *received == 0 {
        "missing"
    } else if received < ordered {
        "short"
    } else if received > ordered {
        "over"
    } else {
        "none"
    }
}
type MappingRow = (
    Uuid,
    Option<Uuid>,
    String,
    String,
    Option<String>,
    String,
    BigDecimal,
);

pub(crate) async fn create(
    State(s): State<AppState>,
    h: HeaderMap,
    Json(i): Json<Create>,
) -> Result<(StatusCode, Json<Guide>), ApiError> {
    let m = member(&s, &h).await?;
    manager(&m)?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(m.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let latest=sqlx::query_scalar::<_,Uuid>("SELECT id FROM inventory_count_sessions WHERE restaurant_id=$1 AND status='completed' ORDER BY completed_at DESC,id DESC LIMIT 1 FOR SHARE").bind(m.restaurant_id).fetch_optional(&mut *tx).await.map_err(database_error)?.ok_or(ApiError(StatusCode::UNPROCESSABLE_ENTITY,"Complete an inventory count before creating an order guide."))?;
    if i.count_id.is_some_and(|count_id| latest != count_id) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Order guides can only use the latest completed count.",
        ));
    }
    if let Some(id) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM order_guides WHERE restaurant_id=$1 AND source_count_id=$2",
    )
    .bind(m.restaurant_id)
    .bind(latest)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    {
        drop(tx);
        return Ok((StatusCode::OK, Json(load(&s, id, m.restaurant_id).await?)));
    }
    let candidates=sqlx::query_as::<_,(Uuid,String,String,Option<BigDecimal>,BigDecimal,Option<Uuid>)>("SELECT i.id,COALESCE(e.name,i.name),COALESCE(e.count_unit,i.count_unit),e.quantity,i.par_level,i.preferred_supplier_id FROM inventory_items i LEFT JOIN inventory_count_entries e ON e.inventory_item_id=i.id AND e.session_id=$2 WHERE i.restaurant_id=$1 AND i.active AND i.par_level IS NOT NULL").bind(m.restaurant_id).bind(latest).fetch_all(&mut *tx).await.map_err(database_error)?;
    if candidates.iter().any(|x| x.3.is_none()) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "All active par-configured items must have count quantities.",
        ));
    }
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO order_guides(id,restaurant_id,source_count_id,created_by) VALUES($1,$2,$3,$4)",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .bind(latest)
    .bind(m.user_id)
    .execute(&mut *tx)
    .await
    .map_err(|e| {
        if e.as_database_error()
            .and_then(|x| x.code())
            .is_some_and(|x| x == "23505")
        {
            ApiError(StatusCode::CONFLICT, "An open order guide already exists.")
        } else {
            database_error(e)
        }
    })?;
    let mut inserted = 0usize;
    for (item, name, unit, count, par, preferred) in candidates {
        let count = count.expect("missing count quantity was checked");
        let sh = shortage(&par, &count);
        if sh <= 0 {
            continue;
        }
        let (mid, sid, supplier, desc, sku, ou, conv) =
            resolve_line_supplier(&mut tx, m.restaurant_id, item, &unit, preferred).await?;
        let suggested = represented(&sh / &conv)?;
        sqlx::query("INSERT INTO order_guide_lines(id,restaurant_id,guide_id,inventory_item_id,inventory_item_name,count_unit,counted_quantity,par_level,shortage,supplier_mapping_id,supplier_id,supplier_name,product_description,supplier_sku,order_unit,count_units_per_order_unit,suggested_order_quantity,order_quantity) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$17)").bind(Uuid::now_v7()).bind(m.restaurant_id).bind(id).bind(item).bind(name).bind(unit).bind(count).bind(par).bind(sh).bind(mid).bind(sid).bind(supplier).bind(desc).bind(sku).bind(ou).bind(conv).bind(suggested).execute(&mut *tx).await.map_err(database_error)?;
        inserted += 1;
    }
    if inserted == 0 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "No inventory items are currently below par.",
        ));
    }
    tx.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(load(&s, id, m.restaurant_id).await?),
    ))
}
pub(crate) async fn open(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Option<Guide>>, ApiError> {
    let m = member(&s, &h).await?;
    let id = sqlx::query_scalar(
        "SELECT id FROM order_guides WHERE restaurant_id=$1 AND status IN ('draft','ordered')",
    )
    .bind(m.restaurant_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(match id {
        Some(x) => Some(load(&s, x, m.restaurant_id).await?),
        None => None,
    }))
}
pub(crate) async fn get(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Guide>, ApiError> {
    let m = member(&s, &h).await?;
    Ok(Json(load(&s, id, m.restaurant_id).await?))
}
pub(crate) async fn update(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(i): Json<Update>,
) -> Result<Json<Guide>, ApiError> {
    let m = member(&s, &h).await?;
    manager(&m)?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let rev=sqlx::query_scalar::<_,i64>("SELECT revision FROM order_guides WHERE id=$1 AND restaurant_id=$2 AND status='draft' FOR UPDATE").bind(id).bind(m.restaurant_id).fetch_optional(&mut *tx).await.map_err(database_error)?.ok_or(ApiError(StatusCode::CONFLICT,"Only a draft order guide can be edited."))?;
    if rev != i.revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This order guide changed. Reload it before saving.",
        ));
    }
    let expected =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM order_guide_lines WHERE guide_id=$1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(database_error)?;
    let submitted: HashSet<_> = i.lines.iter().map(|line| line.id).collect();
    if expected.len() != i.lines.len() || submitted != expected.into_iter().collect() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Submit every order guide line exactly once.",
        ));
    }
    for x in i.lines {
        let c = positive(&x.conversion)?;
        let q = positive(&x.order_quantity)?;
        let u = x.order_unit.trim();
        if u.is_empty() || u.chars().count() > 40 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Order unit must be between 1 and 40 characters.",
            ));
        }
        let item_id = sqlx::query_scalar::<_, Uuid>(
            "SELECT inventory_item_id FROM order_guide_lines WHERE id=$1 AND guide_id=$2",
        )
        .bind(x.id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        let (sid, sn, mid) =
            resolve_edit_supplier(&mut tx, m.restaurant_id, m.user_id, item_id, x.supplier_id, x.supplier_name)
                .await?;
        let shortage = sqlx::query_scalar::<_, BigDecimal>(
            "SELECT shortage FROM order_guide_lines WHERE id=$1 AND guide_id=$2",
        )
        .bind(x.id)
        .bind(id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        let suggested = represented(shortage / &c)?;
        sqlx::query("UPDATE order_guide_lines SET supplier_id=$3,supplier_name=$4,supplier_mapping_id=$5,order_unit=$6,count_units_per_order_unit=$7,suggested_order_quantity=$8,order_quantity=$9 WHERE id=$1 AND guide_id=$2").bind(x.id).bind(id).bind(sid).bind(sn).bind(mid).bind(u).bind(c).bind(suggested).bind(q).execute(&mut *tx).await.map_err(database_error)?;
    }
    sqlx::query(
        "UPDATE order_guides SET revision=revision+1,updated_at=clock_timestamp() WHERE id=$1",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(&s, id, m.restaurant_id).await?))
}
pub(crate) async fn ordered(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RevisionInput>,
) -> Result<Json<Guide>, ApiError> {
    transition(&s, &h, id, input.revision).await
}
pub(crate) async fn cancel(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RevisionInput>,
) -> Result<Json<Guide>, ApiError> {
    let m = member(&s, &h).await?;
    manager(&m)?;
    let n=sqlx::query("UPDATE order_guides g SET status='cancelled',cancelled_by=$3,cancelled_at=NOW(),revision=revision+1,updated_at=NOW() WHERE id=$1 AND restaurant_id=$2 AND revision=$4 AND status IN ('draft','ordered') AND NOT EXISTS(SELECT 1 FROM order_guide_lines WHERE guide_id=g.id AND received_quantity IS NOT NULL)").bind(id).bind(m.restaurant_id).bind(m.user_id).bind(input.revision).execute(&s.pool).await.map_err(database_error)?.rows_affected();
    if n == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This order guide cannot be cancelled.",
        ));
    }
    Ok(Json(load(&s, id, m.restaurant_id).await?))
}
async fn transition(
    s: &AppState,
    h: &HeaderMap,
    id: Uuid,
    expected_revision: i64,
) -> Result<Json<Guide>, ApiError> {
    let m = member(s, h).await?;
    manager(&m)?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(m.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    let (status, revision, source) = sqlx::query_as::<_, (String, i64, Uuid)>("SELECT status,revision,source_count_id FROM order_guides WHERE id=$1 AND restaurant_id=$2 FOR UPDATE").bind(id).bind(m.restaurant_id).fetch_optional(&mut *tx).await.map_err(database_error)?.ok_or(ApiError(StatusCode::NOT_FOUND, "Order guide not found."))?;
    if status == "ordered" && revision == expected_revision + 1 {
        tx.commit().await.map_err(database_error)?;
        return Ok(Json(load(s, id, m.restaurant_id).await?));
    }
    if status != "draft" || revision != expected_revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This draft changed or has already transitioned. Reload it before ordering.",
        ));
    }
    let latest=sqlx::query_scalar::<_,Uuid>("SELECT id FROM inventory_count_sessions WHERE restaurant_id=$1 AND status='completed' ORDER BY completed_at DESC,id DESC LIMIT 1").bind(m.restaurant_id).fetch_optional(&mut *tx).await.map_err(database_error)?;
    if latest != Some(source) {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "A newer inventory count exists. Create a new order guide.",
        ));
    }
    sqlx::query("UPDATE order_guides SET status='ordered',ordered_by=$3,ordered_at=NOW(),revision=revision+1,updated_at=NOW() WHERE id=$1 AND restaurant_id=$2").bind(id).bind(m.restaurant_id).bind(m.user_id).execute(&mut *tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(s, id, m.restaurant_id).await?))
}
pub(crate) async fn receive(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(i): Json<Receive>,
) -> Result<Json<Guide>, ApiError> {
    let m = member(&s, &h).await?;
    if i.lines.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Receive at least one order guide line.",
        ));
    }
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM order_guides WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Order guide not found."))?;
    if status != "ordered" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an ordered guide can be received.",
        ));
    }
    let linked_invoice_id = if let Some(invoice_id) = i.linked_invoice_id {
        let ok = sqlx::query_scalar::<_, bool>(
            "SELECT EXISTS(SELECT 1 FROM invoices WHERE id=$1 AND restaurant_id=$2 AND status='ready')",
        )
        .bind(invoice_id)
        .bind(m.restaurant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        if !ok {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Link an approved invoice from this restaurant.",
            ));
        }
        Some(invoice_id)
    } else {
        None
    };
    for x in i.lines {
        let q = crate::invoices::strict_decimal_with_precision(&x.received_quantity, 30, 12)
            .map_err(|_| {
                ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Received quantity must be a nonnegative decimal.",
                )
            })?;
        if q < 0 {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Received quantity must be a nonnegative decimal.",
            ));
        }
        let ordered = sqlx::query_scalar::<_, BigDecimal>(
            "SELECT order_quantity FROM order_guide_lines
             WHERE id=$1 AND guide_id=$2 AND received_quantity IS NULL",
        )
        .bind(x.id)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or(ApiError(
            StatusCode::CONFLICT,
            "Each order guide line can only be received once.",
        ))?;
        let kind = discrepancy_kind(&ordered, &q);
        let rs = if q == 0 { "missing" } else { "received" };
        let n = sqlx::query(
            "UPDATE order_guide_lines SET received_quantity=$3,receipt_status=$4,discrepancy_kind=$5,
             received_by=$6,received_at=NOW()
             WHERE id=$1 AND guide_id=$2 AND received_quantity IS NULL",
        )
        .bind(x.id)
        .bind(id)
        .bind(q)
        .bind(rs)
        .bind(kind)
        .bind(m.user_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?
        .rows_affected();
        if n == 0 {
            return Err(ApiError(
                StatusCode::CONFLICT,
                "Each order guide line can only be received once.",
            ));
        }
    }
    sqlx::query(
        "UPDATE order_guides g SET
           linked_invoice_id=COALESCE(g.linked_invoice_id,$3),
           status=CASE WHEN NOT EXISTS(SELECT 1 FROM order_guide_lines WHERE guide_id=g.id AND order_quantity>0 AND received_quantity IS NULL) THEN 'received' ELSE status END,
           received_by=CASE WHEN NOT EXISTS(SELECT 1 FROM order_guide_lines WHERE guide_id=g.id AND order_quantity>0 AND received_quantity IS NULL) THEN $2 ELSE received_by END,
           received_at=CASE WHEN NOT EXISTS(SELECT 1 FROM order_guide_lines WHERE guide_id=g.id AND order_quantity>0 AND received_quantity IS NULL) THEN NOW() ELSE received_at END,
           revision=revision+1,updated_at=NOW()
         WHERE id=$1",
    )
    .bind(id)
    .bind(m.user_id)
    .bind(linked_invoice_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(&s, id, m.restaurant_id).await?))
}
async fn resolve_line_supplier(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    restaurant_id: Uuid,
    item: Uuid,
    count_unit: &str,
    preferred: Option<Uuid>,
) -> Result<
    (
        Option<Uuid>,
        Option<Uuid>,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
        BigDecimal,
    ),
    ApiError,
> {
    if let Some(pref) = preferred {
        if let Some(map) = sqlx::query_as::<_, MappingRow>(
            "SELECT id,supplier_id,supplier_name,product_description,supplier_sku,purchase_unit,
                    count_units_per_purchase_unit
             FROM supplier_product_mappings
             WHERE restaurant_id=$1 AND inventory_item_id=$2 AND supplier_id=$3
             ORDER BY updated_at DESC,id DESC LIMIT 1",
        )
        .bind(restaurant_id)
        .bind(item)
        .bind(pref)
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        {
            return Ok((
                Some(map.0),
                map.1.or(Some(pref)),
                Some(map.2),
                Some(map.3),
                map.4,
                map.5,
                map.6,
            ));
        }
        if let Some(name) = sqlx::query_scalar::<_, String>(
            "SELECT name FROM suppliers WHERE id=$1 AND restaurant_id=$2 AND archived_at IS NULL",
        )
        .bind(pref)
        .bind(restaurant_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(database_error)?
        {
            return Ok((
                None,
                Some(pref),
                Some(name),
                None,
                None,
                count_unit.to_owned(),
                BigDecimal::from(1),
            ));
        }
    }
    let maps = sqlx::query_as::<_, MappingRow>(
        "SELECT id,supplier_id,supplier_name,product_description,supplier_sku,purchase_unit,
                count_units_per_purchase_unit
         FROM supplier_product_mappings
         WHERE restaurant_id=$1 AND inventory_item_id=$2
         ORDER BY updated_at DESC,id DESC LIMIT 2",
    )
    .bind(restaurant_id)
    .bind(item)
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?;
    if maps.len() == 1 {
        let map = &maps[0];
        return Ok((
            Some(map.0),
            map.1,
            Some(map.2.clone()),
            Some(map.3.clone()),
            map.4.clone(),
            map.5.clone(),
            map.6.clone(),
        ));
    }
    Ok((
        None,
        None,
        None,
        None,
        None,
        count_unit.to_owned(),
        BigDecimal::from(1),
    ))
}
async fn resolve_edit_supplier(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    restaurant_id: Uuid,
    user_id: Uuid,
    item_id: Uuid,
    supplier_id: Option<Uuid>,
    supplier_name: Option<String>,
) -> Result<(Option<Uuid>, Option<String>, Option<Uuid>), ApiError> {
    let (sid, sn) = if let Some(id) = supplier_id {
        let name = crate::suppliers::require_active_supplier(tx, restaurant_id, id).await?;
        (Some(id), Some(name))
    } else if let Some(raw) = supplier_name {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            (None, None)
        } else {
            let (id, name) =
                crate::suppliers::ensure_supplier(tx, restaurant_id, user_id, trimmed).await?;
            (Some(id), Some(name))
        }
    } else {
        (None, None)
    };
    let mid = match sid {
        Some(id) => {
            sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM supplier_product_mappings
                 WHERE restaurant_id=$1 AND inventory_item_id=$2 AND supplier_id=$3
                 ORDER BY updated_at DESC,id DESC LIMIT 1",
            )
            .bind(restaurant_id)
            .bind(item_id)
            .bind(id)
            .fetch_optional(&mut **tx)
            .await
            .map_err(database_error)?
        }
        None => None,
    };
    Ok((sid, sn, mid))
}
async fn load(s: &AppState, id: Uuid, r: Uuid) -> Result<Guide, ApiError> {
    let h = sqlx::query_as::<_, Header>(
        "SELECT g.id,g.source_count_id,g.status,g.revision,g.created_at,g.updated_at,
                g.ordered_at,g.received_at,g.cancelled_at,g.linked_invoice_id,
                i.supplier_name linked_invoice_supplier_name,i.invoice_date linked_invoice_date
         FROM order_guides g
         LEFT JOIN invoices i ON i.id=g.linked_invoice_id AND i.restaurant_id=g.restaurant_id
         WHERE g.id=$1 AND g.restaurant_id=$2",
    )
    .bind(id)
    .bind(r)
    .fetch_optional(&s.pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Order guide not found."))?;
    let lines = sqlx::query_as::<_, Line>(
        "SELECT id,inventory_item_id,inventory_item_name,count_unit,counted_quantity::text,
                par_level::text,shortage::text,supplier_mapping_id,supplier_id,supplier_name,
                product_description,supplier_sku,order_unit,
                count_units_per_order_unit::text conversion,suggested_order_quantity::text,
                order_quantity::text,received_quantity::text,receipt_status,discrepancy_kind
         FROM order_guide_lines WHERE guide_id=$1 ORDER BY inventory_item_name,id",
    )
    .bind(id)
    .fetch_all(&s.pool)
    .await
    .map_err(database_error)?;
    Ok(Guide {
        id: h.id,
        source_count_id: h.source_count_id,
        status: h.status,
        revision: h.revision,
        created_at: h.created_at,
        updated_at: h.updated_at,
        ordered_at: h.ordered_at,
        received_at: h.received_at,
        cancelled_at: h.cancelled_at,
        linked_invoice_id: h.linked_invoice_id,
        linked_invoice_supplier_name: h.linked_invoice_supplier_name,
        linked_invoice_date: h.linked_invoice_date,
        lines,
    })
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_math() {
        let p: BigDecimal = "10.25".parse().unwrap();
        let c: BigDecimal = "3.1".parse().unwrap();
        let k: BigDecimal = "2.5".parse().unwrap();
        assert_eq!(shortage(&p, &c).to_string(), "7.15");
        assert_eq!((shortage(&p, &c) / k).to_string(), "2.86");
    }
    #[test]
    fn discrepancy_kinds() {
        let ordered: BigDecimal = "3".parse().unwrap();
        assert_eq!(discrepancy_kind(&ordered, &"0".parse().unwrap()), "missing");
        assert_eq!(discrepancy_kind(&ordered, &"2".parse().unwrap()), "short");
        assert_eq!(discrepancy_kind(&ordered, &"3".parse().unwrap()), "none");
        assert_eq!(discrepancy_kind(&ordered, &"4".parse().unwrap()), "over");
    }
    #[test]
    fn representation_rounds_and_checks_range() {
        assert_eq!(
            represented(BigDecimal::from(1) / BigDecimal::from(3))
                .unwrap()
                .to_string(),
            "0.333333333333"
        );
        assert!(positive("999999999999999999.999999999999").is_ok());
        assert!(positive("1000000000000000000").is_err());
        assert!(represented("999999999999999999.9999999999994".parse().unwrap()).is_ok());
        assert!(represented("999999999999999999.9999999999996".parse().unwrap()).is_err());
        assert!(represented("0.0000000000004".parse().unwrap()).is_err());
    }
}
