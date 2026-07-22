use std::{collections::HashSet, time::Duration};

use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Postgres, QueryBuilder};
use uuid::Uuid;

use crate::{
    ApiError, AppState,
    extraction::{GeminiClient, MAX_ATTEMPTS, MenuProviderResult, ProviderError, STALE_MINUTES},
    invoices::{membership, strict_decimal},
    storage::ObjectStorage,
    uploads::{UploadedFile, multipart_error},
};

type ValidatedItem = (
    String,
    Option<String>,
    Option<BigDecimal>,
    Option<String>,
    bool,
);

const RETRY_DELAYS_SECS: [u64; 5] = [30, 5 * 60, 60 * 60, 6 * 60 * 60, 18 * 60 * 60];

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Import {
    id: Uuid,
    original_filename: String,
    status: String,
    delayed: bool,
    created_at: chrono::DateTime<chrono::Utc>,
}
#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Item {
    id: Uuid,
    name: String,
    category: Option<String>,
    selling_price: Option<String>,
    currency: Option<String>,
    has_warnings: bool,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Review {
    import: Import,
    items: Vec<Item>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Approval {
    items: Vec<ApprovalItem>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApprovalItem {
    id: Option<Uuid>,
    name: String,
    category: Option<String>,
    selling_price: String,
    currency: String,
}
#[derive(Serialize)]
pub(crate) struct Counts {
    imported: usize,
    skipped: usize,
}
#[derive(Serialize)]
pub(crate) struct FileUrl {
    url: String,
}

pub(crate) async fn list(
    State(s): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<Import>>, ApiError> {
    let m = membership(&s, &headers).await?;
    Ok(Json(sqlx::query_as("SELECT id,original_filename,status,status='processing' AND updated_at<NOW()-INTERVAL '5 minutes' AS delayed,created_at FROM menu_imports WHERE restaurant_id=$1 ORDER BY created_at DESC LIMIT 20").bind(m.restaurant_id).fetch_all(&s.pool).await.map_err(crate::database_error)?))
}

pub(crate) async fn create(
    State(s): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<(StatusCode, Json<Import>), ApiError> {
    let m = membership(&s, &headers).await?;
    let mut file = None;
    while let Some(field) = multipart.next_field().await.map_err(multipart_error)? {
        if field.name() == Some("file") {
            let name = field.file_name().unwrap_or("").to_owned();
            let bytes = field.bytes().await.map_err(multipart_error)?;
            file = Some((name, bytes));
        }
    }
    let (filename, bytes) = file.ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Choose a menu photo or PDF.",
    ))?;
    let UploadedFile {
        original_filename,
        content_type,
        extension,
        bytes,
    } = UploadedFile::validate(filename, bytes)?;
    let id = Uuid::now_v7();
    let key = format!(
        "restaurants/{}/menu-imports/{id}/original.{}",
        m.restaurant_id, extension
    );
    let size_bytes = bytes.len() as i64;
    s.storage
        .put(&key, content_type, bytes)
        .await
        .map_err(|error| {
            tracing::error!(%error, "menu upload to R2 failed");
            ApiError(
                StatusCode::BAD_GATEWAY,
                "We couldn't store this menu. Please try again.",
            )
        })?;
    let result:Result<Import,sqlx::Error>=async { let mut tx=s.pool.begin().await?; let import=sqlx::query_as("INSERT INTO menu_imports(id,restaurant_id,uploaded_by,original_filename,content_type,size_bytes,object_key) VALUES($1,$2,$3,$4,$5,$6,$7) RETURNING id,original_filename,status,FALSE AS delayed,created_at")
        .bind(id).bind(m.restaurant_id).bind(m.user_id).bind(original_filename).bind(content_type).bind(size_bytes).bind(&key).fetch_one(&mut *tx).await?;
        sqlx::query("INSERT INTO menu_import_jobs(menu_import_id) VALUES($1)").bind(id).execute(&mut *tx).await?; tx.commit().await?; Ok(import)}.await;
    match result {
        Ok(i) => Ok((StatusCode::CREATED, Json(i))),
        Err(error) => {
            tracing::error!(%error, "menu import metadata insert failed");
            if let Err(delete_error) = s.storage.delete(&key).await {
                tracing::error!(%delete_error, object_key = %key, "menu R2 cleanup failed");
            }
            Err(ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "We couldn't save this menu. Please try again.",
            ))
        }
    }
}

pub(crate) async fn review(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Review>, ApiError> {
    let m = membership(&s, &headers).await?;
    let import=sqlx::query_as("SELECT id,original_filename,status,status='processing' AND updated_at<NOW()-INTERVAL '5 minutes' AS delayed,created_at FROM menu_imports WHERE id=$1 AND restaurant_id=$2").bind(id).bind(m.restaurant_id).fetch_optional(&s.pool).await.map_err(crate::database_error)?.ok_or(ApiError(StatusCode::NOT_FOUND,"Menu import not found."))?;
    let items=sqlx::query_as("SELECT id,name,category,selling_price::text selling_price,currency,has_warnings FROM menu_import_items WHERE menu_import_id=$1 ORDER BY position").bind(id).fetch_all(&s.pool).await.map_err(crate::database_error)?;
    Ok(Json(Review { import, items }))
}
pub(crate) async fn file_url(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<FileUrl>, ApiError> {
    let m = membership(&s, &headers).await?;
    let key = sqlx::query_scalar::<_, String>(
        "SELECT object_key FROM menu_imports WHERE id=$1 AND restaurant_id=$2",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(crate::database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Menu import not found."))?;
    Ok(Json(FileUrl {
        url: s.storage.signed_get_url(&key).await.map_err(|_| {
            ApiError(
                StatusCode::BAD_GATEWAY,
                "We couldn't open this menu. Please try again.",
            )
        })?,
    }))
}
pub(crate) async fn retry(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let m = membership(&s, &headers).await?;
    let mut tx = s.pool.begin().await.map_err(crate::database_error)?;
    let n=sqlx::query("UPDATE menu_imports SET status='processing',updated_at=NOW() WHERE id=$1 AND restaurant_id=$2 AND status='failed'").bind(id).bind(m.restaurant_id).execute(&mut *tx).await.map_err(crate::database_error)?.rows_affected();
    if n == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a failed menu import can be retried.",
        ));
    }
    sqlx::query("UPDATE menu_import_jobs SET status='queued',attempts=0,available_at=NOW(),locked_at=NULL,lock_token=NULL,last_error=NULL,updated_at=NOW() WHERE menu_import_id=$1 AND status='failed'").bind(id).execute(&mut *tx).await.map_err(crate::database_error)?;
    tx.commit().await.map_err(crate::database_error)?;
    Ok(StatusCode::ACCEPTED)
}

pub(crate) async fn approve(
    State(s): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<Approval>,
) -> Result<Json<Counts>, ApiError> {
    let m = membership(&s, &headers).await?;
    if input.items.is_empty() || input.items.len() > 200 {
        return Err(invalid());
    }
    let mut clean = Vec::new();
    let mut selected_ids = HashSet::new();
    for item in input.items {
        if item.id.is_some_and(|id| !selected_ids.insert(id)) {
            return Err(invalid());
        }
        let name = item.name.trim().to_owned();
        let category = item.category.and_then(|v| {
            let v = v.trim().to_owned();
            (!v.is_empty()).then_some(v)
        });
        let currency = item.currency.trim().to_ascii_uppercase();
        let price = strict_decimal(item.selling_price.trim(), 4).map_err(|_| invalid())?;
        if name.is_empty()
            || name.chars().count() > 50
            || category.as_ref().is_some_and(|v| v.chars().count() > 20)
            || currency.len() != 3
            || !currency.bytes().all(|v| v.is_ascii_uppercase())
            || price <= 0
        {
            return Err(invalid());
        }
        clean.push((item.id, name, category, price, currency));
    }
    let mut tx = s.pool.begin().await.map_err(crate::database_error)?;
    let status = sqlx::query_scalar::<_, String>(
        "SELECT status FROM menu_imports WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(crate::database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Menu import not found."))?;
    if status != "needs_review" {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This menu import is not waiting for review.",
        ));
    }
    let valid_ids =
        sqlx::query_scalar::<_, Uuid>("SELECT id FROM menu_import_items WHERE menu_import_id=$1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(crate::database_error)?
            .into_iter()
            .collect::<HashSet<_>>();
    if clean
        .iter()
        .filter_map(|value| value.0)
        .any(|id| !valid_ids.contains(&id))
    {
        return Err(invalid());
    }
    let mut insert = QueryBuilder::<Postgres>::new(
        "INSERT INTO menu_items(id,restaurant_id,name,category,selling_price,currency) ",
    );
    insert.push_values(&clean, |mut row, (_, name, category, price, currency)| {
        row.push_bind(Uuid::now_v7())
            .push_bind(m.restaurant_id)
            .push_bind(name)
            .push_bind(category)
            .push_bind(price)
            .push_bind(currency);
    });
    let imported = insert
        .push(" ON CONFLICT DO NOTHING")
        .build()
        .execute(&mut *tx)
        .await
        .map_err(crate::database_error)?
        .rows_affected() as usize;
    sqlx::query("UPDATE menu_imports SET status='imported',updated_at=NOW() WHERE id=$1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(crate::database_error)?;
    tx.commit().await.map_err(crate::database_error)?;
    Ok(Json(Counts {
        imported,
        skipped: clean.len() - imported,
    }))
}
fn invalid() -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Check selected names, prices, categories, and currencies.",
    )
}

#[derive(sqlx::FromRow)]
struct Job {
    id: Uuid,
    object_key: String,
    content_type: String,
    lock_token: Uuid,
}
pub(crate) async fn run_worker(pool: PgPool, storage: ObjectStorage, gemini: GeminiClient) {
    loop {
        match claim(&pool).await {
            Ok(Some(j)) => process(&pool, &storage, &gemini, j).await,
            Ok(None) => tokio::time::sleep(Duration::from_secs(15)).await,
            Err(e) => {
                tracing::error!(%e,"menu import claim failed");
                tokio::time::sleep(Duration::from_secs(10)).await
            }
        }
    }
}
async fn claim(pool: &PgPool) -> Result<Option<Job>> {
    let mut tx = pool.begin().await?;
    let exhausted = sqlx::query_scalar::<_, Uuid>(
        "SELECT menu_import_id FROM menu_import_jobs
         WHERE status='processing' AND attempts >= $1
           AND locked_at < NOW()-make_interval(mins => $2)
         ORDER BY locked_at FOR UPDATE SKIP LOCKED LIMIT 1",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STALE_MINUTES)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = exhausted {
        sqlx::query("UPDATE menu_import_jobs SET status='failed',locked_at=NULL,lock_token=NULL,last_error='Menu worker stopped during the final attempt.',updated_at=NOW() WHERE menu_import_id=$1")
            .bind(id).execute(&mut *tx).await?;
        sqlx::query("UPDATE menu_imports SET status='failed',updated_at=NOW() WHERE id=$1 AND status='processing'")
            .bind(id).execute(&mut *tx).await?;
        tx.commit().await?;
        return Ok(None);
    }
    let id=sqlx::query_scalar::<_,Uuid>("SELECT menu_import_id FROM menu_import_jobs WHERE attempts<$1 AND ((status='queued' AND available_at<=NOW()) OR (status='processing' AND locked_at<NOW()-make_interval(mins => $2))) ORDER BY available_at,created_at FOR UPDATE SKIP LOCKED LIMIT 1").bind(MAX_ATTEMPTS).bind(STALE_MINUTES).fetch_optional(&mut *tx).await?;
    let Some(id) = id else {
        tx.commit().await?;
        return Ok(None);
    };
    let token = Uuid::now_v7();
    sqlx::query("UPDATE menu_import_jobs SET status='processing',attempts=attempts+1,locked_at=NOW(),lock_token=$2,updated_at=NOW() WHERE menu_import_id=$1").bind(id).bind(token).execute(&mut *tx).await?;
    let j = sqlx::query_as(
        "SELECT id,object_key,content_type,$2::uuid lock_token FROM menu_imports WHERE id=$1",
    )
    .bind(id)
    .bind(token)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(j))
}
async fn process(pool: &PgPool, storage: &ObjectStorage, g: &GeminiClient, j: Job) {
    let result = match storage.get(&j.object_key).await {
        Ok(b) => g.extract_menu(b, &j.content_type).await,
        Err(e) => {
            failure(pool, &j, e, false, None).await;
            return;
        }
    };
    match result {
        Ok(r) => {
            let items = match validate_extracted(r.extracted.items.clone()) {
                Ok(items) => items,
                Err(error) => {
                    failure(pool, &j, error, true, None).await;
                    return;
                }
            };
            if let Err(e) = persist(pool, g.model(), &j, r, items).await {
                failure(pool, &j, e, false, None).await
            }
        }
        Err(ProviderError::Retryable { error, retry_after }) => {
            failure(pool, &j, error, false, retry_after).await
        }
        Err(ProviderError::Terminal(e)) => failure(pool, &j, e, true, None).await,
    }
}
async fn persist(
    pool: &PgPool,
    model: &str,
    j: &Job,
    r: MenuProviderResult,
    items: Vec<ValidatedItem>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let own=sqlx::query_scalar::<_,Uuid>("SELECT menu_import_id FROM menu_import_jobs WHERE menu_import_id=$1 AND status='processing' AND lock_token=$2 FOR UPDATE").bind(j.id).bind(j.lock_token).fetch_optional(&mut *tx).await?;
    if own.is_none() {
        tx.commit().await?;
        return Ok(());
    }
    sqlx::query("DELETE FROM menu_import_items WHERE menu_import_id=$1")
        .bind(j.id)
        .execute(&mut *tx)
        .await?;
    let mut insert = QueryBuilder::<Postgres>::new(
        "INSERT INTO menu_import_items(id,menu_import_id,position,name,category,selling_price,currency,has_warnings) ",
    );
    insert.push_values(
        items.into_iter().enumerate(),
        |mut row, (position, (name, category, price, currency, has_warnings))| {
            row.push_bind(Uuid::now_v7())
                .push_bind(j.id)
                .push_bind(position as i32)
                .push_bind(name)
                .push_bind(category)
                .push_bind(price)
                .push_bind(currency)
                .push_bind(has_warnings);
        },
    );
    insert.build().execute(&mut *tx).await?;
    sqlx::query("UPDATE menu_imports SET status='needs_review',provider='gemini',model_id=$2,raw_provider_json=$3,prompt_tokens=$4,candidate_tokens=$5,updated_at=NOW() WHERE id=$1").bind(j.id).bind(model).bind(r.raw).bind(r.prompt_tokens).bind(r.candidate_tokens).execute(&mut *tx).await?;
    sqlx::query("UPDATE menu_import_jobs SET status='completed',locked_at=NULL,lock_token=NULL,last_error=NULL,updated_at=NOW() WHERE menu_import_id=$1 AND lock_token=$2").bind(j.id).bind(j.lock_token).execute(&mut *tx).await?;
    tx.commit().await?;
    Ok(())
}
fn validate_extracted(
    items: Vec<crate::extraction::ExtractedMenuItem>,
) -> Result<Vec<ValidatedItem>> {
    if items.is_empty() {
        return Err(anyhow!("menu extraction did not contain any items"));
    }
    items
        .into_iter()
        .take(200)
        .map(|i| {
            let name = i.name.trim().to_owned();
            let category = i.category.and_then(|v| {
                let v = v.trim().to_owned();
                (!v.is_empty()).then_some(v)
            });
            let raw_currency = i.currency.map(|v| v.trim().to_ascii_uppercase());
            let currency = raw_currency
                .clone()
                .filter(|value| value.len() == 3 && value.bytes().all(|c| c.is_ascii_uppercase()));
            let raw_price = i.selling_price.map(|v| v.trim().to_owned());
            let price = raw_price
                .as_deref()
                .and_then(|value| strict_decimal(value, 4).ok());
            let has_warnings = name.is_empty()
                || name.chars().count() > 50
                || category
                    .as_ref()
                    .is_some_and(|value| value.chars().count() > 20)
                || raw_currency.is_some() && currency.is_none()
                || raw_price.is_some() && price.is_none()
                || price
                    .as_ref()
                    .is_some_and(|value| value <= &BigDecimal::from(0));
            Ok((name, category, price, currency, has_warnings))
        })
        .collect()
}
async fn failure(
    pool: &PgPool,
    j: &Job,
    e: anyhow::Error,
    terminal: bool,
    retry_after: Option<Duration>,
) {
    tracing::warn!(menu_import_id=%j.id, terminal, %e, "menu extraction attempt failed");
    if let Err(db_error) = fail_or_retry(pool, j, &e.to_string(), terminal, retry_after).await {
        tracing::error!(%db_error, menu_import_id=%j.id, "could not update menu import job");
    }
}

async fn fail_or_retry(
    pool: &PgPool,
    j: &Job,
    error: &str,
    terminal: bool,
    retry_after: Option<Duration>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let attempts=sqlx::query_scalar::<_,i32>("SELECT attempts FROM menu_import_jobs WHERE menu_import_id=$1 AND status='processing' AND lock_token=$2 FOR UPDATE").bind(j.id).bind(j.lock_token).fetch_optional(&mut *tx).await?;
    let Some(attempts) = attempts else {
        tx.commit().await?;
        return Ok(());
    };
    let safe_error = error.chars().take(500).collect::<String>();
    if terminal || attempts >= MAX_ATTEMPTS {
        sqlx::query("UPDATE menu_import_jobs SET status='failed',locked_at=NULL,lock_token=NULL,last_error=$3,updated_at=NOW() WHERE menu_import_id=$1 AND lock_token=$2").bind(j.id).bind(j.lock_token).bind(safe_error).execute(&mut *tx).await?;
        sqlx::query("UPDATE menu_imports SET status='failed',updated_at=NOW() WHERE id=$1 AND status='processing'")
            .bind(j.id)
            .execute(&mut *tx)
            .await?;
    } else {
        let delay = retry_delay(attempts, retry_after);
        sqlx::query("UPDATE menu_import_jobs SET status='queued',available_at=NOW()+make_interval(secs=>$3::double precision),locked_at=NULL,lock_token=NULL,last_error=$4,updated_at=NOW() WHERE menu_import_id=$1 AND lock_token=$2").bind(j.id).bind(j.lock_token).bind(delay.as_secs() as i64).bind(safe_error).execute(&mut *tx).await?;
        tracing::info!(menu_import_id=%j.id, attempts, retry_in_seconds=delay.as_secs(), "menu extraction retry scheduled");
    }
    tx.commit().await?;
    Ok(())
}

fn retry_delay(attempts: i32, retry_after: Option<Duration>) -> Duration {
    let index = attempts.saturating_sub(1) as usize;
    let base = Duration::from_secs(
        RETRY_DELAYS_SECS[index.min(RETRY_DELAYS_SECS.len().saturating_sub(1))],
    );
    let minimum = retry_after.unwrap_or_default().max(base);
    let jitter = fastrand::u64(0..=base.as_secs() / 4);
    minimum.saturating_add(Duration::from_secs(jitter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::ExtractedMenuItem;
    #[test]
    fn validates_and_normalizes() {
        let got = validate_extracted(vec![ExtractedMenuItem {
            name: " Taco ".into(),
            category: Some(" Tacos ".into()),
            selling_price: Some("12.50".into()),
            currency: Some(" usd ".into()),
        }])
        .unwrap();
        assert_eq!(got[0].0, "Taco");
        assert_eq!(got[0].3.as_deref(), Some("USD"));
        assert!(!got[0].4);
    }
    #[test]
    fn keeps_questionable_rows_for_review() {
        assert!(validate_extracted(vec![]).is_err());
        let got = validate_extracted(vec![ExtractedMenuItem {
            name: "x".repeat(51),
            category: Some("x".repeat(21)),
            selling_price: Some("market".into()),
            currency: Some("US dollars".into()),
        }])
        .unwrap();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].0.chars().count(), 51);
        assert!(got[0].2.is_none());
        assert!(got[0].3.is_none());
        assert!(got[0].4);
    }

    #[test]
    fn schedules_menu_retries_across_several_hours() {
        for (attempt, expected) in RETRY_DELAYS_SECS.into_iter().enumerate() {
            let delay = retry_delay(attempt as i32 + 1, None).as_secs();
            assert!((expected..=expected + expected / 4).contains(&delay));
        }
    }
}
