use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{NaiveDate, Utc};
use serde::Serialize;
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

    let result = sqlx::query_as::<_, Invoice>(
        "INSERT INTO invoices
         (id, restaurant_id, uploaded_by, supplier_name, invoice_date, original_filename,
          content_type, size_bytes, object_key)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)
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
    .fetch_one(&state.pool)
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
}
