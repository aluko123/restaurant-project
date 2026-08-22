use axum::{
    Json,
    extract::{Multipart, Path, Query, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use bytes::Bytes;
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use num_bigint::Sign;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{
    ApiError, AppState, authenticated_subject,
    uploads::{UploadedFile, multipart_error},
};

use sha2::{Digest, Sha256};

const MAX_SUPPLIER_CHARS: usize = 120;
/// Placeholder until extraction (or review) supplies the real supplier name.
pub(crate) const READING_SUPPLIER: &str = "Reading invoice…";

#[derive(sqlx::FromRow)]
pub(crate) struct Membership {
    pub(crate) restaurant_id: Uuid,
    pub(crate) user_id: Uuid,
    pub(crate) timezone: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Invoice {
    id: Uuid,
    supplier_name: String,
    invoice_date: NaiveDate,
    original_filename: String,
    content_type: String,
    size_bytes: i64,
    status: String,
    delayed: bool,
    price_change_count: i64,
    purchase_receipt_recorded: bool,
    duplicate: bool,
    created_at: chrono::DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct ReviewHeader {
    invoice_id: Uuid,
    supplier_name: String,
    invoice_number: Option<String>,
    invoice_date: Option<NaiveDate>,
    currency: String,
    subtotal: Option<String>,
    tax: Option<String>,
    fees: Option<String>,
    discount: Option<String>,
    total: Option<String>,
    has_warnings: bool,
}

struct Upload {
    supplier_name: String,
    invoice_date: NaiveDate,
    original_filename: String,
    content_type: &'static str,
    extension: &'static str,
    bytes: Bytes,
}

#[derive(Serialize)]
pub(crate) struct FileUrl {
    url: String,
}

const DUPLICATE_INVOICE_MESSAGE: &str = "This invoice file was already uploaded.";

pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<(StatusCode, Json<Invoice>), ApiError> {
    let membership = membership(&state, &headers).await?;
    let today = restaurant_local_today(&membership.timezone, Utc::now());
    let upload = parse_upload(multipart, today).await?;
    let content_hash = hex::encode(Sha256::digest(&upload.bytes));

    if let Some(existing) =
        find_by_content_hash(&state, membership.restaurant_id, &content_hash).await?
    {
        return Ok((StatusCode::OK, Json(existing)));
    }

    let id = Uuid::now_v7();
    let key = object_key(membership.restaurant_id, id, upload.extension);
    let size_bytes = upload.bytes.len() as i64;
    state
        .storage
        .put(&key, upload.content_type, upload.bytes)
        .await
        .map_err(|error| {
            tracing::error!(%error, "invoice upload to R2 failed");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "We couldn't store this invoice. Please try again.",
            )
        })?;

    let result = async {
        let mut tx = state.pool.begin().await.map_err(crate::database_error)?;
        let invoice = sqlx::query_as::<_, Invoice>(
            "INSERT INTO invoices
         (id, restaurant_id, uploaded_by, supplier_name, invoice_date, original_filename,
          content_type, size_bytes, object_key, content_hash, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, 'processing')
         RETURNING id, supplier_name, invoice_date, original_filename, content_type,
                   size_bytes, status, FALSE AS delayed, 0::bigint AS price_change_count,
                   FALSE AS purchase_receipt_recorded, FALSE AS duplicate,
                   created_at",
        )
        .bind(id)
        .bind(membership.restaurant_id)
        .bind(membership.user_id)
        .bind(&upload.supplier_name)
        .bind(upload.invoice_date)
        .bind(upload.original_filename)
        .bind(upload.content_type)
        .bind(size_bytes)
        .bind(&key)
        .bind(&content_hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(|error| {
            if error
                .as_database_error()
                .and_then(|code| code.code())
                .is_some_and(|code| code == "23505")
            {
                ApiError(StatusCode::CONFLICT, DUPLICATE_INVOICE_MESSAGE)
            } else {
                crate::database_error(error)
            }
        })?;
        sqlx::query("INSERT INTO invoice_extraction_jobs (invoice_id) VALUES ($1)")
            .bind(id)
            .execute(&mut *tx)
            .await
            .map_err(crate::database_error)?;
        // Canonical suppliers are created only after human review approve.
        tx.commit().await.map_err(crate::database_error)?;
        Ok::<_, ApiError>(invoice)
    }
    .await;

    match result {
        Ok(invoice) => Ok((StatusCode::CREATED, Json(invoice))),
        Err(ApiError(StatusCode::CONFLICT, message)) if message == DUPLICATE_INVOICE_MESSAGE => {
            // A concurrent upload of the same file won the race; return its record.
            if let Err(delete_error) = state.storage.delete(&key).await {
                tracing::error!(%delete_error, object_key = %key, "invoice R2 cleanup failed");
            }
            match find_by_content_hash(&state, membership.restaurant_id, &content_hash).await? {
                Some(existing) => Ok((StatusCode::OK, Json(existing))),
                None => Err(ApiError(StatusCode::CONFLICT, DUPLICATE_INVOICE_MESSAGE)),
            }
        }
        Err(error) => {
            tracing::error!("invoice metadata insert failed");
            if let Err(delete_error) = state.storage.delete(&key).await {
                tracing::error!(%delete_error, object_key = %key, "invoice R2 cleanup failed");
            }
            Err(match error {
                ApiError(StatusCode::INTERNAL_SERVER_ERROR, _) => ApiError(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "We couldn't save this invoice. Please try again.",
                ),
                other => other,
            })
        }
    }
}

async fn find_by_content_hash(
    state: &AppState,
    restaurant_id: Uuid,
    content_hash: &str,
) -> Result<Option<Invoice>, ApiError> {
    sqlx::query_as::<_, Invoice>(
        "SELECT invoice.id,invoice.supplier_name,invoice.invoice_date,
                invoice.original_filename,invoice.content_type,invoice.size_bytes,invoice.status,
                invoice.status = 'processing'
                    AND invoice.updated_at < NOW() - INTERVAL '5 minutes' AS delayed,
                (SELECT COUNT(*) FROM invoice_price_findings finding
                 WHERE finding.restaurant_id=invoice.restaurant_id
                   AND finding.invoice_id=invoice.id AND finding.status='open') AS price_change_count,
                EXISTS(
                    SELECT 1 FROM purchase_receipts receipt
                    WHERE receipt.invoice_id=invoice.id
                      AND receipt.restaurant_id=invoice.restaurant_id
                ) AS purchase_receipt_recorded,
                TRUE AS duplicate,
                invoice.created_at
         FROM invoices invoice
         WHERE invoice.restaurant_id=$1 AND invoice.content_hash=$2",
    )
    .bind(restaurant_id)
    .bind(content_hash)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, "invoice dedupe lookup failed");
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "We couldn't check this invoice. Please try again.",
        )
    })
}

#[derive(Deserialize)]
pub(crate) struct ListQuery {
    /// Keyset cursor: only rows strictly older than this pair are returned.
    #[serde(default)]
    before_created_at: Option<chrono::DateTime<Utc>>,
    #[serde(default)]
    before_id: Option<Uuid>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(query): Query<ListQuery>,
) -> Result<Json<Vec<Invoice>>, ApiError> {
    let membership = membership(&state, &headers).await?;
    let invoices = sqlx::query_as::<_, Invoice>(
        "SELECT invoice.id,invoice.supplier_name,invoice.invoice_date,
                invoice.original_filename,invoice.content_type,invoice.size_bytes,invoice.status,
                invoice.status = 'processing'
                    AND invoice.updated_at < NOW() - INTERVAL '5 minutes' AS delayed,
                (SELECT COUNT(*) FROM invoice_price_findings finding
                 WHERE finding.restaurant_id=invoice.restaurant_id
                   AND finding.invoice_id=invoice.id AND finding.status='open') AS price_change_count,
                EXISTS(
                    SELECT 1 FROM purchase_receipts receipt
                    WHERE receipt.invoice_id=invoice.id
                      AND receipt.restaurant_id=invoice.restaurant_id
                ) AS purchase_receipt_recorded,
                FALSE AS duplicate,
                invoice.created_at
          FROM invoices invoice
          WHERE invoice.restaurant_id=$1
            AND ($2::timestamptz IS NULL
                 OR invoice.created_at < $2::timestamptz
                 OR (invoice.created_at = $2::timestamptz AND invoice.id < $3))
          ORDER BY invoice.created_at DESC,invoice.id DESC LIMIT 100",
    )
    .bind(membership.restaurant_id)
    .bind(query.before_created_at)
    .bind(query.before_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, "invoice list query failed");
        ApiError(
            StatusCode::INTERNAL_SERVER_ERROR,
            "We couldn't load invoices. Please try again.",
        )
    })?;
    Ok(Json(invoices))
}

pub(crate) async fn file_url(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<FileUrl>, ApiError> {
    let membership = membership(&state, &headers).await?;
    let key = sqlx::query_scalar::<_, String>(
        "SELECT object_key FROM invoices WHERE id = $1 AND restaurant_id = $2",
    )
    .bind(id)
    .bind(membership.restaurant_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, "invoice file lookup failed");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Please try again.")
    })?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Invoice not found."))?;
    let url = state.storage.signed_get_url(&key).await.map_err(|error| {
        tracing::error!(%error, "invoice URL signing failed");
        ApiError(
            StatusCode::BAD_GATEWAY,
            "We couldn't open this invoice. Please try again.",
        )
    })?;
    Ok(Json(FileUrl { url }))
}

pub(crate) async fn membership(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Membership, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as::<_, Membership>(
        "SELECT m.restaurant_id, u.id AS user_id, r.timezone
         FROM users u
         JOIN restaurant_memberships m ON m.user_id = u.id
         JOIN restaurants r ON r.id = m.restaurant_id
         WHERE u.auth_subject = $1 AND m.role IN ('owner','manager')",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        tracing::error!(%error, "invoice membership lookup failed");
        ApiError(StatusCode::INTERNAL_SERVER_ERROR, "Please try again.")
    })?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "Owner or manager access is required for invoices.",
    ))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Review {
    invoice_id: Uuid,
    supplier_name: String,
    invoice_number: Option<String>,
    invoice_date: Option<NaiveDate>,
    currency: String,
    subtotal: Option<String>,
    tax: Option<String>,
    fees: Option<String>,
    discount: Option<String>,
    total: Option<String>,
    has_warnings: bool,
    line_items: Vec<ReviewLine>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct ReviewLine {
    id: Uuid,
    sku: Option<String>,
    description: String,
    quantity: Option<String>,
    unit: Option<String>,
    unit_price: Option<String>,
    line_total: Option<String>,
    has_warnings: bool,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PriceChange {
    id: Uuid,
    description: String,
    unit: Option<String>,
    currency: String,
    previous_unit_price: String,
    current_unit_price: String,
    percentage_change: String,
    previous_invoice_date: NaiveDate,
    status: String,
}

#[derive(sqlx::FromRow)]
struct PriceChangeRow {
    id: Uuid,
    invoice_id: Uuid,
    supplier_name: String,
    invoice_date: NaiveDate,
    created_at: chrono::DateTime<Utc>,
    description: String,
    unit: Option<String>,
    currency: String,
    previous_unit_price: String,
    current_unit_price: String,
    percentage_change: String,
    previous_invoice_date: NaiveDate,
    comparison_key: String,
    comparison_unit: String,
    increased: bool,
    at_least_ten_percent: bool,
    status: String,
}

pub(crate) struct TodayPriceChange {
    pub(crate) invoice_id: Uuid,
    pub(crate) supplier_name: String,
    pub(crate) invoice_date: NaiveDate,
    pub(crate) created_at: chrono::DateTime<Utc>,
    pub(crate) description: String,
    pub(crate) unit: Option<String>,
    pub(crate) currency: String,
    pub(crate) previous_unit_price: String,
    pub(crate) current_unit_price: String,
    pub(crate) percentage_change: String,
    pub(crate) previous_invoice_date: NaiveDate,
    pub(crate) comparison_key: String,
    pub(crate) comparison_unit: String,
    pub(crate) increased: bool,
    pub(crate) at_least_ten_percent: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Approval {
    price_changes: Vec<PriceChange>,
}

const PRICE_CHANGES_SQL: &str = "WITH comparable_lines AS (
        SELECT line.id,line.invoice_id,line.position,line.description,line.unit,line.unit_price,
               line.comparison_key,line.comparison_unit,invoice.restaurant_id,
               invoice.supplier_name,invoice.invoice_date,invoice.created_at,extraction.currency,
               COUNT(*) OVER (
                   PARTITION BY line.invoice_id,line.comparison_key,line.comparison_unit
               ) AS matching_lines
        FROM invoices invoice
        JOIN invoice_extractions extraction ON extraction.invoice_id=invoice.id
        JOIN invoice_line_items line ON line.invoice_id=invoice.id AND line.unit_price>0
        WHERE invoice.restaurant_id=$2 AND invoice.status='ready'
          AND line.comparison_key IS NOT NULL AND line.comparison_unit IS NOT NULL
     ), history AS (
        SELECT id,invoice_id,position,description,unit,unit_price,currency,supplier_name,
               invoice_date,created_at,comparison_key,comparison_unit,
               LAG(unit_price) OVER item_history AS previous_unit_price,
               LAG(invoice_date) OVER item_history AS previous_invoice_date
        FROM comparable_lines
        WHERE matching_lines=1
        WINDOW item_history AS (
            PARTITION BY restaurant_id,LOWER(BTRIM(supplier_name)),currency,
                         comparison_key,comparison_unit
            ORDER BY invoice_date,created_at,invoice_id
        )
     ), changes AS (
        SELECT id,invoice_id,position,supplier_name,invoice_date,created_at,description,unit,
               currency,previous_unit_price,unit_price,comparison_key,comparison_unit,
               ROUND(((unit_price-previous_unit_price)/previous_unit_price)*100,2)
                   AS percentage_change,
               previous_invoice_date,unit_price>previous_unit_price AS increased,
               unit_price*100>=previous_unit_price*110 AS at_least_ten_percent
        FROM history
        WHERE previous_unit_price IS NOT NULL
          AND ABS(unit_price-previous_unit_price)*100>=previous_unit_price*5
     )
     SELECT id,invoice_id,supplier_name,invoice_date,created_at,description,unit,currency,
            previous_unit_price::text AS previous_unit_price,
            unit_price::text AS current_unit_price,percentage_change::text AS percentage_change,
            previous_invoice_date,comparison_key,comparison_unit,increased,at_least_ten_percent,
            ''::text AS status
     FROM changes
     WHERE $1::uuid IS NULL OR invoice_id=$1
     ORDER BY ABS(percentage_change) DESC,position,invoice_id";

const FINDINGS_SQL: &str = "SELECT id,invoice_id,supplier_name,invoice_date,
        invoice_created_at AS created_at,description,unit,currency,
        previous_unit_price::text AS previous_unit_price,
        current_unit_price::text AS current_unit_price,
        percentage_change::text AS percentage_change,previous_invoice_date,comparison_key,
        comparison_unit,increased,at_least_ten_percent,status
    FROM invoice_price_findings
    WHERE restaurant_id=$1 AND ($2::uuid IS NULL OR invoice_id=$2)
      AND status<>'baseline'
    ORDER BY ABS(percentage_change) DESC,created_at,id";

const OPEN_FINDINGS_SQL: &str = "SELECT id,invoice_id,supplier_name,invoice_date,
        invoice_created_at AS created_at,description,unit,currency,
        previous_unit_price::text AS previous_unit_price,
        current_unit_price::text AS current_unit_price,
        percentage_change::text AS percentage_change,previous_invoice_date,comparison_key,
        comparison_unit,increased,at_least_ten_percent,status
    FROM invoice_price_findings
    WHERE restaurant_id=$1 AND status='open'
    ORDER BY ABS(percentage_change) DESC,created_at,id";

struct ReviewedLine {
    line_id: Uuid,
    position: i32,
    sku: Option<String>,
    description: String,
    quantity: Option<BigDecimal>,
    unit: Option<String>,
    unit_price: Option<BigDecimal>,
    line_total: Option<BigDecimal>,
    comparison_key: Option<String>,
    comparison_unit: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct ReviewInput {
    supplier_name: String,
    invoice_number: Option<String>,
    invoice_date: Option<String>,
    currency: String,
    subtotal: Option<String>,
    tax: Option<String>,
    fees: Option<String>,
    discount: Option<String>,
    total: Option<String>,
    line_items: Vec<ReviewLineInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ReviewLineInput {
    sku: Option<String>,
    description: String,
    quantity: Option<String>,
    unit: Option<String>,
    unit_price: Option<String>,
    line_total: Option<String>,
}

pub(crate) async fn get_review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Review>, ApiError> {
    let member = membership(&state, &headers).await?;
    let header = sqlx::query_as::<_, ReviewHeader>("SELECT e.invoice_id,e.supplier_name,e.invoice_number,e.invoice_date,e.currency,
        e.subtotal::text subtotal,e.tax::text tax,e.fees::text fees,e.discount::text discount,e.total::text total,e.has_warnings
        FROM invoice_extractions e JOIN invoices i ON i.id=e.invoice_id WHERE e.invoice_id=$1 AND i.restaurant_id=$2 AND i.status IN ('needs_review','ready')")
        .bind(id).bind(member.restaurant_id).fetch_optional(&state.pool).await.map_err(crate::database_error)?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Invoice review is not available."))?;
    let line_items = sqlx::query_as::<_, ReviewLine>("SELECT id,sku,description,quantity::text quantity,unit,unit_price::text unit_price,line_total::text line_total,has_warnings FROM invoice_line_items WHERE invoice_id=$1 ORDER BY position")
        .bind(id).fetch_all(&state.pool).await.map_err(crate::database_error)?;
    let review = Review {
        invoice_id: header.invoice_id,
        supplier_name: header.supplier_name,
        invoice_number: header.invoice_number,
        invoice_date: header.invoice_date,
        currency: header.currency,
        subtotal: header.subtotal,
        tax: header.tax,
        fees: header.fees,
        discount: header.discount,
        total: header.total,
        has_warnings: header.has_warnings,
        line_items,
    };
    Ok(Json(review))
}

pub(crate) async fn put_review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReviewInput>,
) -> Result<Json<Approval>, ApiError> {
    let member = membership(&state, &headers).await?;
    let today = restaurant_local_today(&member.timezone, Utc::now());
    let input = validate_review(input, today)?;
    let invoice_date = input
        .invoice_date
        .as_deref()
        .map(|value| validate_date(value, today))
        .transpose()?;
    let subtotal = parse_decimal(&input.subtotal, 4)?;
    let tax = parse_decimal(&input.tax, 4)?;
    let fees = parse_decimal(&input.fees, 4)?;
    let discount = parse_decimal(&input.discount, 4)?;
    let total = parse_decimal(&input.total, 4)?;
    let lines = reviewed_lines(&input.line_items)?;

    let mut tx = state.pool.begin().await.map_err(crate::database_error)?;
    let changed = sqlx::query("UPDATE invoices SET supplier_name=$3,invoice_date=COALESCE($4,invoice_date),status='ready',updated_at=NOW() WHERE id=$1 AND restaurant_id=$2 AND status='needs_review'")
        .bind(id).bind(member.restaurant_id).bind(&input.supplier_name).bind(invoice_date)
        .execute(&mut *tx).await.map_err(crate::database_error)?.rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This invoice is not waiting for review.",
        ));
    }
    sqlx::query("UPDATE invoice_extractions SET supplier_name=$2,invoice_number=$3,invoice_date=$4,currency=$5,subtotal=$6,tax=$7,fees=$8,discount=$9,total=$10,has_warnings=FALSE,reviewed_by=$11,reviewed_at=NOW(),updated_at=NOW() WHERE invoice_id=$1")
        .bind(id).bind(&input.supplier_name).bind(&input.invoice_number).bind(invoice_date).bind(&input.currency)
        .bind(subtotal).bind(tax).bind(fees).bind(discount).bind(total).bind(member.user_id)
        .execute(&mut *tx).await.map_err(crate::database_error)?;
    // Final corrected supplier name from review becomes the canonical supplier.
    crate::suppliers::ensure_supplier(
        &mut tx,
        member.restaurant_id,
        member.user_id,
        &input.supplier_name,
    )
    .await?;
    sqlx::query("DELETE FROM invoice_line_items WHERE invoice_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(crate::database_error)?;
    if !lines.is_empty() {
        let mut query = QueryBuilder::<Postgres>::new(
            "INSERT INTO invoice_line_items
             (id,invoice_id,position,sku,description,quantity,unit,unit_price,line_total,
              comparison_key,comparison_unit) ",
        );
        query.push_values(&lines, |mut row, line| {
            row.push_bind(line.line_id)
                .push_bind(id)
                .push_bind(line.position)
                .push_bind(&line.sku)
                .push_bind(&line.description)
                .push_bind(&line.quantity)
                .push_bind(&line.unit)
                .push_bind(&line.unit_price)
                .push_bind(&line.line_total)
                .push_bind(&line.comparison_key)
                .push_bind(&line.comparison_unit);
        });
        query
            .build()
            .execute(&mut *tx)
            .await
            .map_err(crate::database_error)?;
    }
    let mut computed_changes = sqlx::query_as::<_, PriceChangeRow>(PRICE_CHANGES_SQL)
        .bind(Some(id))
        .bind(member.restaurant_id)
        .fetch_all(&mut *tx)
        .await
        .map_err(crate::database_error)?;
    let setup_complete = sqlx::query_scalar::<_, bool>(
        "SELECT migration_setup_completed_at IS NOT NULL FROM restaurants WHERE id=$1",
    )
    .bind(member.restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(crate::database_error)?;
    let finding_status = if setup_complete { "open" } else { "baseline" };
    for change in &mut computed_changes {
        change.status = finding_status.to_owned();
    }
    for change in &computed_changes {
        sqlx::query(
            "INSERT INTO invoice_price_findings(
                 id,restaurant_id,invoice_id,source_line_id,supplier_name,invoice_date,
                 invoice_created_at,description,unit,currency,previous_unit_price,
                 current_unit_price,percentage_change,previous_invoice_date,comparison_key,
                 comparison_unit,increased,at_least_ten_percent,status)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11::NUMERIC,$12::NUMERIC,
                    $13::NUMERIC,$14,$15,$16,$17,$18,$19)
             ON CONFLICT (restaurant_id,source_line_id) DO NOTHING",
        )
        .bind(Uuid::now_v7())
        .bind(member.restaurant_id)
        .bind(change.invoice_id)
        .bind(change.id)
        .bind(&change.supplier_name)
        .bind(change.invoice_date)
        .bind(change.created_at)
        .bind(&change.description)
        .bind(&change.unit)
        .bind(&change.currency)
        .bind(&change.previous_unit_price)
        .bind(&change.current_unit_price)
        .bind(&change.percentage_change)
        .bind(change.previous_invoice_date)
        .bind(&change.comparison_key)
        .bind(&change.comparison_unit)
        .bind(change.increased)
        .bind(change.at_least_ten_percent)
        .bind(finding_status)
        .execute(&mut *tx)
        .await
        .map_err(crate::database_error)?;
    }
    let price_changes = computed_changes
        .into_iter()
        .filter(|change| change.status == "open")
        .map(PriceChange::from)
        .collect();
    tx.commit().await.map_err(crate::database_error)?;
    Ok(Json(Approval { price_changes }))
}

pub(crate) async fn price_changes(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Vec<PriceChange>>, ApiError> {
    let member = membership(&state, &headers).await?;
    let changes = sqlx::query_as::<_, PriceChangeRow>(FINDINGS_SQL)
        .bind(member.restaurant_id)
        .bind(Some(id))
        .fetch_all(&state.pool)
        .await
        .map_err(crate::database_error)?
        .into_iter()
        .map(PriceChange::from)
        .collect();
    Ok(Json(changes))
}

pub(crate) async fn restaurant_price_changes(
    pool: &PgPool,
    restaurant_id: Uuid,
) -> Result<Vec<TodayPriceChange>, sqlx::Error> {
    Ok(sqlx::query_as::<_, PriceChangeRow>(OPEN_FINDINGS_SQL)
        .bind(restaurant_id)
        .fetch_all(pool)
        .await?
        .into_iter()
        .map(TodayPriceChange::from)
        .collect())
}

pub(crate) async fn review_price_finding(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path((invoice_id, finding_id)): Path<(Uuid, Uuid)>,
) -> Result<StatusCode, ApiError> {
    let member = membership(&state, &headers).await?;
    let changed = sqlx::query(
        "UPDATE invoice_price_findings
         SET reviewed_by=CASE WHEN status='open' THEN $4 ELSE reviewed_by END,
             reviewed_at=CASE WHEN status='open' THEN NOW() ELSE reviewed_at END,
             status='reviewed'
         WHERE id=$1 AND invoice_id=$2 AND restaurant_id=$3
           AND status IN ('open','reviewed')",
    )
    .bind(finding_id)
    .bind(invoice_id)
    .bind(member.restaurant_id)
    .bind(member.user_id)
    .execute(&state.pool)
    .await
    .map_err(crate::database_error)?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Open price finding not found.",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

impl From<PriceChangeRow> for PriceChange {
    fn from(row: PriceChangeRow) -> Self {
        Self {
            id: row.id,
            description: row.description,
            unit: row.unit,
            currency: row.currency,
            previous_unit_price: row.previous_unit_price,
            current_unit_price: row.current_unit_price,
            percentage_change: row.percentage_change,
            previous_invoice_date: row.previous_invoice_date,
            status: row.status,
        }
    }
}

impl From<PriceChangeRow> for TodayPriceChange {
    fn from(row: PriceChangeRow) -> Self {
        Self {
            invoice_id: row.invoice_id,
            supplier_name: row.supplier_name,
            invoice_date: row.invoice_date,
            created_at: row.created_at,
            description: row.description,
            unit: row.unit,
            currency: row.currency,
            previous_unit_price: row.previous_unit_price,
            current_unit_price: row.current_unit_price,
            percentage_change: row.percentage_change,
            previous_invoice_date: row.previous_invoice_date,
            comparison_key: row.comparison_key,
            comparison_unit: row.comparison_unit,
            increased: row.increased,
            at_least_ten_percent: row.at_least_ten_percent,
        }
    }
}

pub(crate) async fn retry(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let member = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(crate::database_error)?;
    let changed = sqlx::query("UPDATE invoices SET status='processing',updated_at=NOW() WHERE id=$1 AND restaurant_id=$2 AND status='failed'").bind(id).bind(member.restaurant_id).execute(&mut *tx).await.map_err(crate::database_error)?.rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a failed invoice can be retried.",
        ));
    }
    sqlx::query("INSERT INTO invoice_extraction_jobs (invoice_id) VALUES ($1) ON CONFLICT (invoice_id) DO UPDATE SET status='queued',attempts=0,available_at=NOW(),locked_at=NULL,lock_token=NULL,last_error=NULL,updated_at=NOW() WHERE invoice_extraction_jobs.status='failed'")
        .bind(id).execute(&mut *tx).await.map_err(crate::database_error)?;
    tx.commit().await.map_err(crate::database_error)?;
    Ok(StatusCode::ACCEPTED)
}

pub(crate) async fn delete(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let member = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(crate::database_error)?;
    let existing = sqlx::query_as::<_, (String, String)>(
        "SELECT status,object_key FROM invoices WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(member.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| {
        tracing::error!(%error, "invoice delete lookup failed");
        crate::database_error(error)
    })?;
    let (status, object_key) = match existing {
        Some(row) => row,
        None => return Err(ApiError(StatusCode::NOT_FOUND, "Invoice not found.")),
    };
    if !matches!(status.as_str(), "processing" | "failed" | "needs_review") {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an invoice that hasn't been approved yet can be deleted.",
        ));
    }
    let deleted = sqlx::query(
        "DELETE FROM invoices
         WHERE id=$1 AND restaurant_id=$2
           AND NOT EXISTS(
               SELECT 1 FROM purchase_receipts receipt WHERE receipt.invoice_id=invoices.id
           )
           AND NOT EXISTS(
               SELECT 1 FROM order_guides guide WHERE guide.linked_invoice_id=invoices.id
           )",
    )
    .bind(id)
    .bind(member.restaurant_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| {
        tracing::error!(%error, "invoice delete failed");
        crate::database_error(error)
    })?
    .rows_affected();
    tx.commit().await.map_err(crate::database_error)?;
    if deleted == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only an invoice that hasn't been approved yet can be deleted.",
        ));
    }
    if let Err(delete_error) = state.storage.delete(&object_key).await {
        tracing::error!(%delete_error, object_key = %object_key, "invoice R2 cleanup failed");
    }
    Ok(StatusCode::NO_CONTENT)
}

fn validate_review(mut i: ReviewInput, today: NaiveDate) -> Result<ReviewInput, ApiError> {
    i.supplier_name = validate_supplier(&i.supplier_name)?;
    i.currency = i.currency.trim().to_ascii_uppercase();
    if i.line_items.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Add at least one reviewed line item before approving this invoice.",
        ));
    }
    if i.currency.len() != 3
        || !i.currency.bytes().all(|c| c.is_ascii_uppercase())
        || i.line_items.len() > 200
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Check the supplier, currency, and line item count.",
        ));
    }
    for line in &mut i.line_items {
        line.description = line.description.trim().to_owned();
        line.sku = trim_optional(line.sku.take());
        line.unit = trim_optional(line.unit.take());
        if line.description.is_empty()
            || line.description.chars().count() > 500
            || line
                .sku
                .as_ref()
                .is_some_and(|value| value.chars().count() > 120)
            || line
                .unit
                .as_ref()
                .is_some_and(|value| value.chars().count() > 40)
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Check each line's description, SKU, and unit.",
            ));
        }
        parse_nonnegative_decimal(&line.quantity, 6)?;
        parse_nonnegative_decimal(&line.unit_price, 4)?;
        parse_nonnegative_decimal(&line.line_total, 4)?;
    }
    i.invoice_number = trim_optional(i.invoice_number.take());
    if i.invoice_number
        .as_ref()
        .is_some_and(|value| value.chars().count() > 120)
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invoice number must be no more than 120 characters.",
        ));
    }
    for value in [&i.subtotal, &i.tax, &i.fees, &i.discount, &i.total] {
        parse_nonnegative_decimal(value, 4)?;
    }
    if let Some(date) = &i.invoice_date {
        validate_date(date, today)?;
    }
    Ok(i)
}
fn parse_decimal(v: &Option<String>, scale: i64) -> Result<Option<BigDecimal>, ApiError> {
    let Some(v) = v.as_deref() else {
        return Ok(None);
    };
    if v.is_empty() {
        return Ok(None);
    }
    let n = strict_decimal(v, scale as usize).map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Use plain decimal values within the allowed size and decimal places.",
        )
    })?;
    Ok(Some(n))
}

fn parse_nonnegative_decimal(
    v: &Option<String>,
    scale: i64,
) -> Result<Option<BigDecimal>, ApiError> {
    let n = parse_decimal(v, scale)?;
    if n.as_ref()
        .is_some_and(|amount| amount.sign() == Sign::Minus)
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Amounts can't be negative. Remove the minus sign or correct the value.",
        ));
    }
    Ok(n)
}

pub(crate) fn strict_decimal(value: &str, scale: usize) -> Result<BigDecimal, &'static str> {
    strict_decimal_with_precision(value, 18, scale)
}

pub(crate) fn strict_decimal_with_precision(
    value: &str,
    precision: usize,
    scale: usize,
) -> Result<BigDecimal, &'static str> {
    if scale > precision {
        return Err("invalid decimal");
    }
    let unsigned = value
        .strip_prefix('-')
        .or_else(|| value.strip_prefix('+'))
        .unwrap_or(value);
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    if value.is_empty()
        || value.len() > 32
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
        || fraction.len() > scale
        || integer.trim_start_matches('0').len().max(1) > precision - scale
        || unsigned.matches('.').count() > 1
    {
        return Err("invalid decimal");
    }
    value.parse().map_err(|_| "invalid decimal")
}

fn comparison_key(sku: Option<&str>, description: &str) -> Option<String> {
    if let Some(sku) = sku.and_then(normalized_value) {
        Some(format!("sku:{sku}"))
    } else {
        normalized_value(description).map(|description| format!("description:{description}"))
    }
}

fn normalized_value(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect::<String>();
    (!normalized.is_empty()).then_some(normalized)
}

fn reviewed_lines(lines: &[ReviewLineInput]) -> Result<Vec<ReviewedLine>, ApiError> {
    lines
        .iter()
        .enumerate()
        .map(|(position, line)| {
            Ok(ReviewedLine {
                line_id: Uuid::now_v7(),
                position: position as i32,
                sku: line.sku.clone(),
                description: line.description.clone(),
                quantity: parse_decimal(&line.quantity, 6)?,
                unit: line.unit.clone(),
                unit_price: parse_decimal(&line.unit_price, 4)?,
                line_total: parse_decimal(&line.line_total, 4)?,
                comparison_key: comparison_key(line.sku.as_deref(), &line.description),
                comparison_unit: line.unit.as_deref().and_then(normalized_value),
            })
        })
        .collect()
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

async fn parse_upload(mut multipart: Multipart, today: NaiveDate) -> Result<Upload, ApiError> {
    let mut date = None;
    let mut file = None;
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name() {
            // Legacy clients may still send supplierName; ignore it — extraction is source of truth.
            Some("supplierName") => {
                let _ = field.text().await.map_err(multipart_error)?;
            }
            Some("invoiceDate") => date = Some(field.text().await.map_err(multipart_error)?),
            Some("file") => {
                let filename = field.file_name().unwrap_or("").to_owned();
                let bytes = field.bytes().await.map_err(multipart_error)?;
                file = Some((filename, bytes));
            }
            _ => {}
        }
    }
    let invoice_date = match date
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
    {
        Some(value) => validate_date(value, today)?,
        None => today,
    };
    let (original_filename, bytes) = file.ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Choose an invoice file.",
    ))?;
    let file = UploadedFile::validate(original_filename, bytes)?;
    Ok(Upload {
        supplier_name: READING_SUPPLIER.to_owned(),
        invoice_date,
        original_filename: file.original_filename,
        content_type: file.content_type,
        extension: file.extension,
        bytes: file.bytes,
    })
}

fn validate_supplier(value: &str) -> Result<String, ApiError> {
    let value = value.trim();
    if value.is_empty() || value.chars().count() > MAX_SUPPLIER_CHARS {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Supplier name must be between 1 and 120 characters.",
        ));
    }
    if value == READING_SUPPLIER {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Enter the supplier name from the invoice before approving.",
        ));
    }
    Ok(value.to_owned())
}

fn restaurant_local_today(timezone: &str, now: DateTime<Utc>) -> NaiveDate {
    let tz = timezone.parse::<Tz>().unwrap_or_else(|_| {
        tracing::warn!(
            timezone,
            "invalid restaurant timezone; invoice date validation falling back to UTC"
        );
        chrono_tz::UTC
    });
    now.with_timezone(&tz).date_naive()
}

fn validate_date(value: &str, today: NaiveDate) -> Result<NaiveDate, ApiError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invoice date must be a valid date.",
        )
    })?;
    let earliest = NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid fixed date");
    if date < earliest || date > today {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invoice date must be between 2000-01-01 and today.",
        ));
    }
    Ok(date)
}

fn object_key(restaurant_id: Uuid, invoice_id: Uuid, extension: &str) -> String {
    format!("restaurants/{restaurant_id}/invoices/{invoice_id}/original.{extension}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reading_placeholder_is_stable() {
        assert_eq!(READING_SUPPLIER, "Reading invoice…");
        assert!(READING_SUPPLIER.chars().count() <= MAX_SUPPLIER_CHARS);
    }

    #[test]
    fn validates_supplier_and_date() {
        let today = NaiveDate::from_ymd_opt(2026, 7, 24).unwrap();
        assert_eq!(validate_supplier("  Acme Foods ").unwrap(), "Acme Foods");
        assert!(validate_supplier(" ").is_err());
        assert!(validate_supplier(&"x".repeat(121)).is_err());
        assert!(validate_date("not-a-date", today).is_err());
        assert!(validate_date("1999-12-31", today).is_err());
        assert!(validate_date("2999-01-01", today).is_err());
        assert_eq!(
            validate_date("2026-07-24", today).unwrap(),
            NaiveDate::from_ymd_opt(2026, 7, 24).unwrap()
        );
    }

    #[test]
    fn invoice_today_uses_restaurant_timezone() {
        // 2026-07-25 01:30 UTC is still 2026-07-24 evening in America/Chicago.
        let now = DateTime::parse_from_rfc3339("2026-07-25T01:30:00Z")
            .unwrap()
            .with_timezone(&Utc);
        let chicago_today = restaurant_local_today("America/Chicago", now);
        assert_eq!(chicago_today, NaiveDate::from_ymd_opt(2026, 7, 24).unwrap());
        assert!(validate_date("2026-07-24", chicago_today).is_ok());
        assert!(validate_date("2026-07-25", chicago_today).is_err());

        let utc_today = restaurant_local_today("UTC", now);
        assert_eq!(utc_today, NaiveDate::from_ymd_opt(2026, 7, 25).unwrap());
        assert!(validate_date("2026-07-25", utc_today).is_ok());
    }

    #[test]
    fn generates_tenant_scoped_key() {
        let restaurant = Uuid::nil();
        let invoice = Uuid::from_u128(1);
        assert_eq!(
            object_key(restaurant, invoice, "pdf"),
            format!("restaurants/{restaurant}/invoices/{invoice}/original.pdf")
        );
    }

    #[test]
    fn validates_decimal_scale_and_format() {
        assert!(parse_decimal(&Some("12.3456".into()), 4).is_ok());
        assert!(parse_decimal(&Some("12.34567".into()), 4).is_err());
        assert!(parse_decimal(&Some("$12.00".into()), 4).is_err());
        assert!(parse_decimal(&Some("1e3".into()), 4).is_err());
        assert!(parse_decimal(&Some("1_000".into()), 4).is_err());
        assert!(parse_decimal(&Some("1000000000000".into()), 6).is_err());
        assert!(parse_decimal(&None, 4).unwrap().is_none());
    }

    #[test]
    fn builds_conservative_item_comparison_keys() {
        assert_eq!(
            comparison_key(Some(" CHK-42 "), "Chicken").as_deref(),
            Some("sku:chk42")
        );
        assert_eq!(
            comparison_key(None, "Chicken Breast, 10 KG").as_deref(),
            Some("description:chickenbreast10kg")
        );
        assert_eq!(
            normalized_value(" Case / 10 lb ").as_deref(),
            Some("case10lb")
        );
        assert_eq!(comparison_key(None, "鶏肉"), None);
    }
}
