use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject};

const MAX_FILE_BYTES: usize = 10 * 1024 * 1024;
const MAX_SUPPLIER_CHARS: usize = 120;

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    user_id: Uuid,
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
}

struct Upload {
    supplier_name: String,
    invoice_date: NaiveDate,
    original_filename: String,
    content_type: &'static str,
    extension: &'static str,
    bytes: Vec<u8>,
}

#[derive(Serialize)]
pub(crate) struct FileUrl {
    url: String,
}

pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    multipart: Multipart,
) -> Result<(StatusCode, Json<Invoice>), ApiError> {
    let membership = membership(&state, &headers).await?;
    let upload = parse_upload(multipart).await?;
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
        let mut tx = state.pool.begin().await?;
        let invoice = sqlx::query_as::<_, Invoice>(
            "INSERT INTO invoices
         (id, restaurant_id, uploaded_by, supplier_name, invoice_date, original_filename,
          content_type, size_bytes, object_key, status)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'processing')
         RETURNING id, supplier_name, invoice_date, original_filename, content_type,
                   size_bytes, status, created_at",
        )
        .bind(id)
        .bind(membership.restaurant_id)
        .bind(membership.user_id)
        .bind(upload.supplier_name)
        .bind(upload.invoice_date)
        .bind(upload.original_filename)
        .bind(upload.content_type)
        .bind(size_bytes)
        .bind(&key)
        .fetch_one(&mut *tx)
        .await?;
        sqlx::query("INSERT INTO invoice_extraction_jobs (invoice_id) VALUES ($1)")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok::<_, sqlx::Error>(invoice)
    }
    .await;

    match result {
        Ok(invoice) => Ok((StatusCode::CREATED, Json(invoice))),
        Err(error) => {
            tracing::error!(%error, "invoice metadata insert failed");
            if let Err(delete_error) = state.storage.delete(&key).await {
                tracing::error!(%delete_error, object_key = %key, "invoice R2 cleanup failed");
            }
            Err(ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "We couldn't save this invoice. Please try again.",
            ))
        }
    }
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Invoice>>, ApiError> {
    let membership = membership(&state, &headers).await?;
    let invoices = sqlx::query_as::<_, Invoice>(
        "SELECT id, supplier_name, invoice_date, original_filename, content_type,
                size_bytes, status, created_at
         FROM invoices WHERE restaurant_id = $1
         ORDER BY created_at DESC, id DESC LIMIT 100",
    )
    .bind(membership.restaurant_id)
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

async fn membership(state: &AppState, headers: &HeaderMap) -> Result<Membership, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as::<_, Membership>(
        "SELECT m.restaurant_id, u.id AS user_id FROM users u
         JOIN restaurant_memberships m ON m.user_id = u.id
         WHERE u.auth_subject = $1 AND m.role = 'owner'",
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
        "An owner restaurant membership is required.",
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
        e.subtotal::text subtotal,e.tax::text tax,e.fees::text fees,e.discount::text discount,e.total::text total
        FROM invoice_extractions e JOIN invoices i ON i.id=e.invoice_id WHERE e.invoice_id=$1 AND i.restaurant_id=$2 AND i.status IN ('needs_review','ready')")
        .bind(id).bind(member.restaurant_id).fetch_optional(&state.pool).await.map_err(crate::database_error)?
        .ok_or(ApiError(StatusCode::NOT_FOUND, "Invoice review is not available."))?;
    let line_items = sqlx::query_as::<_, ReviewLine>("SELECT id,sku,description,quantity::text quantity,unit,unit_price::text unit_price,line_total::text line_total FROM invoice_line_items WHERE invoice_id=$1 ORDER BY position")
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
        line_items,
    };
    Ok(Json(review))
}

pub(crate) async fn put_review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReviewInput>,
) -> Result<Json<Review>, ApiError> {
    let member = membership(&state, &headers).await?;
    let input = validate_review(input)?;
    let mut tx = state.pool.begin().await.map_err(crate::database_error)?;
    let invoice_date = input
        .invoice_date
        .as_deref()
        .map(parse_review_date)
        .transpose()?;
    let changed = sqlx::query("UPDATE invoices SET supplier_name=$3,invoice_date=COALESCE($4,invoice_date),status='ready',updated_at=NOW() WHERE id=$1 AND restaurant_id=$2 AND status='needs_review'")
        .bind(id).bind(member.restaurant_id).bind(&input.supplier_name).bind(invoice_date)
        .execute(&mut *tx).await.map_err(crate::database_error)?.rows_affected();
    if changed == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This invoice is not waiting for review.",
        ));
    }
    sqlx::query("UPDATE invoice_extractions SET supplier_name=$2,invoice_number=$3,invoice_date=$4,currency=$5,subtotal=$6,tax=$7,fees=$8,discount=$9,total=$10,reviewed_by=$11,reviewed_at=NOW(),updated_at=NOW() WHERE invoice_id=$1")
        .bind(id).bind(&input.supplier_name).bind(&input.invoice_number).bind(invoice_date).bind(&input.currency)
        .bind(parse_decimal(&input.subtotal, 4)?).bind(parse_decimal(&input.tax, 4)?).bind(parse_decimal(&input.fees, 4)?).bind(parse_decimal(&input.discount, 4)?).bind(parse_decimal(&input.total, 4)?).bind(member.user_id)
        .execute(&mut *tx).await.map_err(crate::database_error)?;
    sqlx::query("DELETE FROM invoice_line_items WHERE invoice_id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(crate::database_error)?;
    for (position, line) in input.line_items.iter().enumerate() {
        sqlx::query("INSERT INTO invoice_line_items (id,invoice_id,position,sku,description,quantity,unit,unit_price,line_total) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9)")
            .bind(Uuid::now_v7()).bind(id).bind(position as i32).bind(&line.sku).bind(&line.description).bind(parse_decimal(&line.quantity,6)?).bind(&line.unit).bind(parse_decimal(&line.unit_price,4)?).bind(parse_decimal(&line.line_total,4)?)
            .execute(&mut *tx).await.map_err(crate::database_error)?;
    }
    tx.commit().await.map_err(crate::database_error)?;
    get_review(State(state), headers, Path(id)).await
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

fn validate_review(mut i: ReviewInput) -> Result<ReviewInput, ApiError> {
    i.supplier_name = i.supplier_name.trim().to_owned();
    i.currency = i.currency.trim().to_ascii_uppercase();
    if i.supplier_name.is_empty()
        || i.supplier_name.chars().count() > 120
        || i.currency.len() != 3
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
        parse_decimal(&line.quantity, 6)?;
        parse_decimal(&line.unit_price, 4)?;
        parse_decimal(&line.line_total, 4)?;
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
        parse_decimal(value, 4)?;
    }
    if let Some(date) = &i.invoice_date {
        parse_review_date(date)?;
    }
    Ok(i)
}
fn parse_review_date(v: &str) -> Result<NaiveDate, ApiError> {
    NaiveDate::parse_from_str(v, "%Y-%m-%d").map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Use a valid invoice date.",
        )
    })
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

pub(crate) fn strict_decimal(value: &str, scale: usize) -> Result<BigDecimal, &'static str> {
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
        || integer.trim_start_matches('0').len().max(1) > 18 - scale
        || unsigned.matches('.').count() > 1
    {
        return Err("invalid decimal");
    }
    value.parse().map_err(|_| "invalid decimal")
}

fn trim_optional(value: Option<String>) -> Option<String> {
    value.and_then(|value| {
        let value = value.trim();
        (!value.is_empty()).then(|| value.to_owned())
    })
}

async fn parse_upload(mut multipart: Multipart) -> Result<Upload, ApiError> {
    let mut supplier = None;
    let mut date = None;
    let mut file = None;
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        match field.name() {
            Some("supplierName") => supplier = Some(field.text().await.map_err(multipart_error)?),
            Some("invoiceDate") => date = Some(field.text().await.map_err(multipart_error)?),
            Some("file") => {
                let filename = field.file_name().unwrap_or("").to_owned();
                let bytes = field.bytes().await.map_err(multipart_error)?.to_vec();
                file = Some((filename, bytes));
            }
            _ => {}
        }
    }
    let supplier_name = validate_supplier(supplier.as_deref().unwrap_or(""))?;
    let invoice_date = validate_date(date.as_deref().unwrap_or(""))?;
    let (original_filename, bytes) = file.ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Choose an invoice file.",
    ))?;
    validate_filename(&original_filename)?;
    if bytes.is_empty() || bytes.len() > MAX_FILE_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Invoice files must be between 1 byte and 10 MiB.",
        ));
    }
    let (content_type, extension) = detect_file_type(&bytes).ok_or(ApiError(
        StatusCode::UNSUPPORTED_MEDIA_TYPE,
        "Upload a PDF, JPEG, PNG, or WebP invoice.",
    ))?;
    Ok(Upload {
        supplier_name,
        invoice_date,
        original_filename,
        content_type,
        extension,
        bytes,
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
    Ok(value.to_owned())
}

fn validate_date(value: &str) -> Result<NaiveDate, ApiError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invoice date must be a valid date.",
        )
    })?;
    let earliest = NaiveDate::from_ymd_opt(2000, 1, 1).expect("valid fixed date");
    if date < earliest || date > Utc::now().date_naive() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invoice date must be between 2000-01-01 and today.",
        ));
    }
    Ok(date)
}

fn validate_filename(value: &str) -> Result<(), ApiError> {
    if value.trim().is_empty() || value.chars().count() > 255 || value.chars().any(char::is_control)
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The original filename is missing or too long.",
        ));
    }
    Ok(())
}

fn detect_file_type(bytes: &[u8]) -> Option<(&'static str, &'static str)> {
    if bytes.starts_with(b"%PDF-") {
        Some(("application/pdf", "pdf"))
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some(("image/jpeg", "jpg"))
    } else if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some(("image/png", "png"))
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some(("image/webp", "webp"))
    } else {
        None
    }
}

fn object_key(restaurant_id: Uuid, invoice_id: Uuid, extension: &str) -> String {
    format!("restaurants/{restaurant_id}/invoices/{invoice_id}/original.{extension}")
}

fn multipart_error(error: axum::extract::multipart::MultipartError) -> ApiError {
    tracing::debug!(%error, "invalid invoice multipart request");
    ApiError(StatusCode::BAD_REQUEST, "The upload request was invalid.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_supported_signatures_and_rejects_extension_only() {
        assert_eq!(
            detect_file_type(b"%PDF-1.7"),
            Some(("application/pdf", "pdf"))
        );
        assert_eq!(
            detect_file_type(&[0xff, 0xd8, 0xff, 0x00]),
            Some(("image/jpeg", "jpg"))
        );
        assert_eq!(
            detect_file_type(b"\x89PNG\r\n\x1a\nrest"),
            Some(("image/png", "png"))
        );
        assert_eq!(
            detect_file_type(b"RIFF0000WEBPrest"),
            Some(("image/webp", "webp"))
        );
        assert_eq!(detect_file_type(b"invoice.pdf"), None);
    }

    #[test]
    fn validates_supplier_and_date() {
        assert_eq!(validate_supplier("  Acme Foods ").unwrap(), "Acme Foods");
        assert!(validate_supplier(" ").is_err());
        assert!(validate_supplier(&"x".repeat(121)).is_err());
        assert!(validate_date("not-a-date").is_err());
        assert!(validate_date("1999-12-31").is_err());
        assert!(validate_date("2999-01-01").is_err());
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
}
