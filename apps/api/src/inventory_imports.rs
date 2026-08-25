use crate::{
    ApiError, AppState, authenticated_subject, database_error,
    extraction::{ExtractedInventoryCsv, GeminiClient, MAX_ATTEMPTS, ProviderError, STALE_MINUTES},
    inventory::ItemInput,
    storage::ObjectStorage,
    uploads::multipart_error,
};
use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::{collections::HashSet, time::Duration};
use uuid::Uuid;
const MAX_BYTES: usize = 1024 * 1024;
const RETRY_DELAYS_SECS: [u64; 5] = [30, 5 * 60, 60 * 60, 6 * 60 * 60, 18 * 60 * 60];
#[cfg(test)]
const MAX_ROWS: usize = 2000;
#[cfg(test)]
const MAX_ERRORS: usize = 25;
/// Spreadsheet imports parse deterministically in production too, so their
/// limits are not test-only.
const SPREADSHEET_MAX_ROWS: usize = 2000;
const SPREADSHEET_MAX_ERRORS: usize = 25;
#[derive(sqlx::FromRow)]
struct Member {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}
#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Row {
    id: Uuid,
    row_number: i32,
    name: String,
    category: Option<String>,
    count_unit: String,
    par_level: Option<String>,
    validation_errors: serde_json::Value,
    selected: Option<bool>,
    created_inventory_item_id: Option<Uuid>,
}
#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct Head {
    id: Uuid,
    original_filename: String,
    content_hash: String,
    status: String,
    delayed: bool,
    revision: i64,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Import {
    #[serde(flatten)]
    head: Head,
    rows: Vec<Row>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct Apply {
    revision: i64,
    rows: Vec<ApplyRow>,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ApplyRow {
    id: Uuid,
    selected: bool,
    name: String,
    category: Option<String>,
    count_unit: String,
    par_level: Option<String>,
}
async fn member(s: &AppState, h: &HeaderMap) -> Result<Member, ApiError> {
    let sub = authenticated_subject(s, h).await?;
    let m:Member=sqlx::query_as("SELECT m.restaurant_id,u.id user_id,m.role FROM users u JOIN restaurant_memberships m ON m.user_id=u.id WHERE u.auth_subject=$1").bind(sub).fetch_optional(&s.pool).await.map_err(database_error)?.ok_or(ApiError(StatusCode::FORBIDDEN,"A restaurant membership is required."))?;
    if !matches!(m.role.as_str(), "owner" | "manager") {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Owner or manager access is required.",
        ));
    }
    Ok(m)
}
pub(crate) async fn create(
    State(s): State<AppState>,
    h: HeaderMap,
    mut mp: Multipart,
) -> Result<(StatusCode, Json<Import>), ApiError> {
    let m = member(&s, &h).await?;
    let mut file = None;
    while let Some(f) = mp.next_field().await.map_err(multipart_error)? {
        if f.name() == Some("file") {
            let name = f.file_name().unwrap_or("inventory.csv").to_owned();
            let b = f.bytes().await.map_err(multipart_error)?;
            file = Some((name, b));
        }
    }
    let (name, b) = file.ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Attach one CSV or spreadsheet file in the file field.",
    ))?;
    if b.is_empty() || b.len() > MAX_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "Inventory files must be between 1 byte and 1 MiB.",
        ));
    }
    let hash = format!("{:x}", Sha256::digest(&b));

    // Spreadsheets have a fixed grid, so they parse deterministically right
    // here — no extraction model, and the preview is ready immediately.
    if is_spreadsheet(&name) {
        let parsed = parse_spreadsheet(&b)?;
        // The original is kept for audit only; nothing re-reads it, so a
        // storage hiccup must not fail an already-parsed import.
        let key = format!(
            "restaurants/{}/inventory-imports/{}/original.xlsx",
            m.restaurant_id,
            Uuid::now_v7()
        );
        if let Err(error) = s.storage.put(&key, "application/vnd.ms-excel", b).await {
            tracing::error!(%error, "inventory spreadsheet archive upload failed");
        }
        return insert_import_sync(&s, &m, name, hash, parsed).await;
    }

    // Re-uploading the same file returns the existing import so retries are
    // safe and never duplicate records.
    if let Some(existing) = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM inventory_imports WHERE restaurant_id=$1 AND content_hash=$2",
    )
    .bind(m.restaurant_id)
    .bind(&hash)
    .fetch_optional(&s.pool)
    .await
    .map_err(database_error)?
    {
        return Ok((
            StatusCode::OK,
            Json(load(&s, existing, m.restaurant_id).await?),
        ));
    }

    // Release tests use the deterministic parser and expect the preview to be
    // ready immediately; production enqueues a background extraction job.
    #[cfg(test)]
    if s.gemini.is_inert_for_tests() {
        let parsed = parse(&b)?;
        return insert_import_sync(&s, &m, name, hash, parsed).await;
    }

    let id = Uuid::now_v7();
    let key = format!(
        "restaurants/{}/inventory-imports/{id}/original.csv",
        m.restaurant_id
    );
    s.storage.put(&key, "text/csv", b).await.map_err(|error| {
        tracing::error!(%error, "inventory import upload to object storage failed");
        ApiError(
            StatusCode::BAD_GATEWAY,
            "We couldn't store this inventory file. Please try again.",
        )
    })?;

    let result: Result<Uuid, sqlx::Error> = async {
        let mut tx = s.pool.begin().await?;
        let inserted = sqlx::query_scalar::<_, Uuid>(
            "INSERT INTO inventory_imports(id,restaurant_id,original_filename,content_hash,status,object_key,created_by)
             VALUES($1,$2,$3,$4,'processing',$5,$6)
             ON CONFLICT (restaurant_id,content_hash) DO NOTHING RETURNING id",
        )
        .bind(id)
        .bind(m.restaurant_id)
        .bind(&name)
        .bind(&hash)
        .bind(&key)
        .bind(m.user_id)
        .fetch_optional(&mut *tx)
        .await?;
        let Some(inserted) = inserted else {
            tx.commit().await?;
            let existing = sqlx::query_scalar::<_, Uuid>(
                "SELECT id FROM inventory_imports WHERE restaurant_id=$1 AND content_hash=$2",
            )
        .bind(m.restaurant_id)
        .bind(hash.clone())
            .fetch_one(&s.pool)
            .await?;
            return Ok(existing);
        };
        sqlx::query("INSERT INTO inventory_import_jobs(import_id) VALUES($1)")
            .bind(inserted)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(inserted)
    }
    .await;

    match result {
        Ok(import_id) => {
            if import_id != id {
                // A concurrent upload of the same file won the race.
                if let Err(delete_error) = s.storage.delete(&key).await {
                    tracing::error!(%delete_error, object_key = %key, "inventory import R2 cleanup failed");
                }
                return Ok((
                    StatusCode::OK,
                    Json(load(&s, import_id, m.restaurant_id).await?),
                ));
            }
            Ok((
                StatusCode::CREATED,
                Json(load(&s, id, m.restaurant_id).await?),
            ))
        }
        Err(error) => {
            tracing::error!(%error, "inventory import metadata insert failed");
            if let Err(delete_error) = s.storage.delete(&key).await {
                tracing::error!(%delete_error, object_key = %key, "inventory import R2 cleanup failed");
            }
            Err(ApiError(
                StatusCode::INTERNAL_SERVER_ERROR,
                "We couldn't save this inventory file. Please try again.",
            ))
        }
    }
}

async fn insert_import_sync(
    s: &AppState,
    m: &Member,
    name: String,
    hash: String,
    parsed: Vec<(Row, Vec<String>)>,
) -> Result<(StatusCode, Json<Import>), ApiError> {
    let id = Uuid::now_v7();
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let inserted = sqlx::query_scalar::<_, Uuid>("INSERT INTO inventory_imports(id,restaurant_id,original_filename,content_hash,created_by)VALUES($1,$2,$3,$4,$5) ON CONFLICT (restaurant_id,content_hash) DO NOTHING RETURNING id").bind(id).bind(m.restaurant_id).bind(name).bind(&hash).bind(m.user_id).fetch_optional(&mut*tx).await.map_err(database_error)?;
    let Some(import_id) = inserted else {
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM inventory_imports WHERE restaurant_id=$1 AND content_hash=$2",
        )
        .bind(m.restaurant_id)
        .bind(&hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        return Ok((
            StatusCode::OK,
            Json(load(s, existing, m.restaurant_id).await?),
        ));
    };
    insert_rows(&mut tx, m.restaurant_id, import_id, parsed).await?;
    tx.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(load(s, import_id, m.restaurant_id).await?),
    ))
}

async fn insert_rows(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    restaurant_id: Uuid,
    import_id: Uuid,
    parsed: Vec<(Row, Vec<String>)>,
) -> Result<(), ApiError> {
    let existing: HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT LOWER(BTRIM(name)) FROM inventory_items WHERE restaurant_id=$1",
    )
    .bind(restaurant_id)
    .fetch_all(&mut **tx)
    .await
    .map_err(database_error)?
    .into_iter()
    .collect();
    let mut seen = HashSet::new();
    for (r, mut errors) in parsed {
        let key = r.name.trim().to_lowercase();
        if !seen.insert(key.clone()) {
            errors.push("Duplicate normalized name in this file.".into())
        }
        if existing.contains(&key) {
            errors.push("An inventory item with this name already exists.".into())
        }
        sqlx::query("INSERT INTO inventory_import_rows(id,restaurant_id,import_id,row_number,name,category,count_unit,par_level,validation_errors)VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(Uuid::now_v7()).bind(restaurant_id).bind(import_id).bind(r.row_number).bind(r.name).bind(r.category).bind(r.count_unit).bind(r.par_level).bind(serde_json::json!(errors)).execute(&mut **tx).await.map_err(database_error)?;
    }
    Ok(())
}

fn validate_extracted(
    extracted: ExtractedInventoryCsv,
) -> Result<Vec<(Row, Vec<String>)>, ApiError> {
    if extracted.items.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "We couldn't find inventory items in this file.",
        ));
    }
    Ok(extracted
        .items
        .into_iter()
        .take(200)
        .enumerate()
        .map(|(index, item)| {
            let name = item.name.trim().to_owned();
            let category = item.category.and_then(|value| {
                let value = value.trim().to_owned();
                (!value.is_empty()).then_some(value)
            });
            let count_unit = item
                .count_unit
                .map(|value| value.trim().to_owned())
                .unwrap_or_default();
            let par_level = item.par_level.and_then(|value| {
                let value = value.trim().to_owned();
                (!value.is_empty()).then_some(value)
            });
            let input = ItemInput {
                name: name.clone(),
                category: category.clone(),
                count_unit: count_unit.clone(),
                par_level: par_level.clone(),
                storage_area_id: None,
                shelf_order: 0,
                preferred_supplier_id: None,
                active: true,
            };
            let errors = input
                .validated()
                .err()
                .map(|error| vec![error.1.into()])
                .unwrap_or_default();
            (
                Row {
                    id: Uuid::nil(),
                    row_number: (index + 2) as i32,
                    name,
                    category,
                    count_unit,
                    par_level,
                    validation_errors: serde_json::json!([]),
                    selected: None,
                    created_inventory_item_id: None,
                },
                errors,
            )
        })
        .collect())
}
pub(crate) async fn get(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Import>, ApiError> {
    let m = member(&s, &h).await?;
    Ok(Json(load(&s, id, m.restaurant_id).await?))
}
pub(crate) async fn latest(
    State(s): State<AppState>,
    h: HeaderMap,
) -> Result<Json<Option<Import>>, ApiError> {
    let m = member(&s, &h).await?;
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT id FROM inventory_imports WHERE restaurant_id=$1 AND status='needs_review' ORDER BY updated_at DESC,id DESC LIMIT 1",
    )
    .bind(m.restaurant_id)
    .fetch_optional(&s.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(match id {
        Some(id) => Some(load(&s, id, m.restaurant_id).await?),
        None => None,
    }))
}
pub(crate) async fn discard(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let m = member(&s, &h).await?;
    let result = sqlx::query(
        "DELETE FROM inventory_imports
         WHERE id=$1 AND restaurant_id=$2 AND status IN ('needs_review','failed')",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .execute(&s.pool)
    .await
    .map_err(database_error)?;
    if result.rows_affected() == 0 {
        return Err(ApiError(
            StatusCode::NOT_FOUND,
            "Open inventory import not found.",
        ));
    }
    Ok(StatusCode::NO_CONTENT)
}

pub(crate) async fn retry(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<StatusCode, ApiError> {
    let m = member(&s, &h).await?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let n = sqlx::query(
        "UPDATE inventory_imports SET status='processing',updated_at=NOW()
         WHERE id=$1 AND restaurant_id=$2 AND status='failed'",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?
    .rows_affected();
    if n == 0 {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Only a failed inventory import can be retried.",
        ));
    }
    sqlx::query(
        "UPDATE inventory_import_jobs SET status='queued',attempts=0,available_at=NOW(),
         locked_at=NULL,lock_token=NULL,last_error=NULL,updated_at=NOW()
         WHERE import_id=$1 AND status='failed'",
    )
    .bind(id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(StatusCode::ACCEPTED)
}
pub(crate) async fn apply(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
    Json(i): Json<Apply>,
) -> Result<Json<Import>, ApiError> {
    let m = member(&s, &h).await?;
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let (st, rev) = sqlx::query_as::<_, (String, i64)>(
        "SELECT status,revision FROM inventory_imports WHERE id=$1 AND restaurant_id=$2 FOR UPDATE",
    )
    .bind(id)
    .bind(m.restaurant_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::NOT_FOUND,
        "Inventory import not found.",
    ))?;
    if st == "applied" {
        drop(tx);
        return Ok(Json(load(&s, id, m.restaurant_id).await?));
    }
    if rev != i.revision {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "This import changed. Reload it before applying.",
        ));
    }
    let expected: HashSet<Uuid> =
        sqlx::query_scalar("SELECT id FROM inventory_import_rows WHERE import_id=$1")
            .bind(id)
            .fetch_all(&mut *tx)
            .await
            .map_err(database_error)?
            .into_iter()
            .collect();
    let submitted: HashSet<_> = i.rows.iter().map(|row| row.id).collect();
    if expected.len() != i.rows.len() || submitted != expected {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Submit every import row exactly once.",
        ));
    }
    let mut names = HashSet::new();
    for r in i.rows {
        if !r.selected {
            sqlx::query("UPDATE inventory_import_rows SET name=$3,category=$4,count_unit=$5,par_level=$6,selected=false,created_inventory_item_id=NULL WHERE id=$1 AND import_id=$2 AND restaurant_id=$7").bind(r.id).bind(id).bind(r.name).bind(r.category).bind(r.count_unit).bind(r.par_level).bind(m.restaurant_id).execute(&mut*tx).await.map_err(database_error)?;
            continue;
        }
        let input = ItemInput {
            name: r.name,
            category: r.category,
            count_unit: r.count_unit,
            par_level: r.par_level,
            storage_area_id: None,
            shelf_order: 0,
            preferred_supplier_id: None,
            active: true,
        };
        let v = input.validated()?;
        if !names.insert(v.name.to_lowercase()) {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Selected inventory item names must be unique.",
            ));
        }
        let item = Uuid::now_v7();
        sqlx::query("INSERT INTO inventory_items(id,restaurant_id,name,category,count_unit,par_level,active)VALUES($1,$2,$3,$4,$5,$6,true)").bind(item).bind(m.restaurant_id).bind(&v.name).bind(&v.category).bind(&v.count_unit).bind(&v.par_level).execute(&mut*tx).await.map_err(|e|if e.as_database_error().and_then(|x|x.code()).is_some_and(|x|x=="23505"){ApiError(StatusCode::CONFLICT,"A selected inventory item already exists.")}else{database_error(e)})?;
        sqlx::query("UPDATE inventory_import_rows SET name=$3,category=$4,count_unit=$5,par_level=$6,selected=true,created_inventory_item_id=$7 WHERE id=$1 AND import_id=$2 AND restaurant_id=$8").bind(r.id).bind(id).bind(v.name).bind(v.category).bind(v.count_unit).bind(v.par_level.map(|x|x.to_string())).bind(item).bind(m.restaurant_id).execute(&mut*tx).await.map_err(database_error)?;
    }
    sqlx::query("UPDATE inventory_imports SET status='applied',revision=revision+1,applied_by=$2,applied_at=NOW(),updated_at=NOW() WHERE id=$1").bind(id).bind(m.user_id).execute(&mut*tx).await.map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(&s, id, m.restaurant_id).await?))
}
async fn load(s: &AppState, id: Uuid, r: Uuid) -> Result<Import, ApiError> {
    let head=sqlx::query_as("SELECT id,original_filename,content_hash,status,status='processing' AND updated_at<NOW()-INTERVAL '5 minutes' AS delayed,revision FROM inventory_imports WHERE id=$1 AND restaurant_id=$2").bind(id).bind(r).fetch_optional(&s.pool).await.map_err(database_error)?.ok_or(ApiError(StatusCode::NOT_FOUND,"Inventory import not found."))?;
    let rows=sqlx::query_as("SELECT id,row_number,name,category,count_unit,par_level,validation_errors,selected,created_inventory_item_id FROM inventory_import_rows WHERE import_id=$1 ORDER BY row_number").bind(id).fetch_all(&s.pool).await.map_err(database_error)?;
    Ok(Import { head, rows })
}
#[derive(sqlx::FromRow)]
struct Job {
    import_id: Uuid,
    restaurant_id: Uuid,
    object_key: String,
    lock_token: Uuid,
}

pub(crate) async fn run_worker(pool: PgPool, storage: ObjectStorage, gemini: GeminiClient) {
    loop {
        match run_once(&pool, &storage, &gemini).await {
            Ok(true) => {}
            Ok(false) => tokio::time::sleep(Duration::from_secs(15)).await,
            Err(error) => {
                tracing::error!(%error, "inventory import claim failed");
                tokio::time::sleep(Duration::from_secs(10)).await;
            }
        }
    }
}

/// Claims and processes at most one job. Returns whether a job was found so
/// release tests can drain the queue deterministically.
pub(crate) async fn run_once(
    pool: &PgPool,
    storage: &ObjectStorage,
    gemini: &GeminiClient,
) -> Result<bool> {
    match claim(pool).await? {
        Some(job) => {
            process(pool, storage, gemini, job).await;
            Ok(true)
        }
        None => Ok(false),
    }
}

async fn claim(pool: &PgPool) -> Result<Option<Job>> {
    let mut tx = pool.begin().await?;
    let exhausted = sqlx::query_scalar::<_, Uuid>(
        "SELECT import_id FROM inventory_import_jobs
         WHERE status='processing' AND attempts >= $1
           AND locked_at < NOW()-make_interval(mins => $2)
         ORDER BY locked_at FOR UPDATE SKIP LOCKED LIMIT 1",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STALE_MINUTES)
    .fetch_optional(&mut *tx)
    .await?;
    if let Some(id) = exhausted {
        sqlx::query("UPDATE inventory_import_jobs SET status='failed',locked_at=NULL,lock_token=NULL,last_error='Inventory worker stopped during the final attempt.',updated_at=NOW() WHERE import_id=$1")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE inventory_imports SET status='failed',updated_at=NOW() WHERE id=$1 AND status='processing'")
            .bind(id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        return Ok(None);
    }
    let id = sqlx::query_scalar::<_, Uuid>(
        "SELECT import_id FROM inventory_import_jobs
         WHERE attempts<$1
           AND ((status='queued' AND available_at<=NOW())
             OR (status='processing' AND locked_at<NOW()-make_interval(mins => $2)))
         ORDER BY available_at,created_at FOR UPDATE SKIP LOCKED LIMIT 1",
    )
    .bind(MAX_ATTEMPTS)
    .bind(STALE_MINUTES)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(id) = id else {
        tx.commit().await?;
        return Ok(None);
    };
    let token = Uuid::now_v7();
    sqlx::query("UPDATE inventory_import_jobs SET status='processing',attempts=attempts+1,locked_at=NOW(),lock_token=$2,updated_at=NOW() WHERE import_id=$1")
        .bind(id)
        .bind(token)
        .execute(&mut *tx)
        .await?;
    let job = sqlx::query_as(
        "SELECT i.id import_id,i.restaurant_id,i.object_key,$2::uuid lock_token
         FROM inventory_imports i WHERE i.id=$1 AND i.object_key IS NOT NULL",
    )
    .bind(id)
    .bind(token)
    .fetch_one(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Some(job))
}

async fn process(pool: &PgPool, storage: &ObjectStorage, g: &GeminiClient, j: Job) {
    let bytes = match storage.get(&j.object_key).await {
        Ok(bytes) => bytes,
        Err(error) => {
            failure(
                pool,
                &j,
                error.context("could not read the stored file"),
                false,
                None,
            )
            .await;
            return;
        }
    };
    // Release tests use the deterministic parser so the whole queue runs
    // without network access; production always extracts via Gemini.
    #[cfg(test)]
    let result = if g.is_inert_for_tests() {
        match parse(&bytes) {
            Ok(parsed) => Ok(parsed),
            Err(error) => Err(ProviderError::Terminal(anyhow!(error.1))),
        }
    } else {
        extract(g, bytes).await
    };
    #[cfg(not(test))]
    let result = extract(g, bytes).await;
    match result {
        Ok(parsed) => {
            if let Err(error) = persist_rows_and_finish(pool, &j, parsed).await {
                failure(pool, &j, error, false, None).await;
            }
        }
        Err(ProviderError::Retryable { error, retry_after }) => {
            failure(pool, &j, error, false, retry_after).await
        }
        Err(ProviderError::Terminal(error)) => failure(pool, &j, error, true, None).await,
    }
}

async fn extract(
    g: &GeminiClient,
    bytes: bytes::Bytes,
) -> Result<Vec<(Row, Vec<String>)>, ProviderError> {
    let extracted = g.extract_inventory_csv(bytes).await?;
    validate_extracted(extracted.extracted)
        .map_err(|error| ProviderError::Terminal(anyhow!(error.1)))
}

async fn persist_rows_and_finish(
    pool: &PgPool,
    j: &Job,
    parsed: Vec<(Row, Vec<String>)>,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let own = sqlx::query_scalar::<_, Uuid>(
        "SELECT import_id FROM inventory_import_jobs
         WHERE import_id=$1 AND status='processing' AND lock_token=$2 FOR UPDATE",
    )
    .bind(j.import_id)
    .bind(j.lock_token)
    .fetch_optional(&mut *tx)
    .await?;
    if own.is_none() {
        tx.commit().await?;
        return Ok(());
    }
    sqlx::query("DELETE FROM inventory_import_rows WHERE import_id=$1")
        .bind(j.import_id)
        .execute(&mut *tx)
        .await?;
    insert_rows(&mut tx, j.restaurant_id, j.import_id, parsed)
        .await
        .map_err(|error| anyhow!(error.1.to_string()))?;
    sqlx::query("UPDATE inventory_imports SET status='needs_review',updated_at=NOW() WHERE id=$1")
        .bind(j.import_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query(
        "UPDATE inventory_import_jobs SET status='completed',locked_at=NULL,lock_token=NULL,
         last_error=NULL,updated_at=NOW()
         WHERE import_id=$1 AND lock_token=$2",
    )
    .bind(j.import_id)
    .bind(j.lock_token)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(())
}

async fn failure(
    pool: &PgPool,
    j: &Job,
    error: anyhow::Error,
    terminal: bool,
    retry_after: Option<Duration>,
) {
    tracing::warn!(import_id=%j.import_id, terminal, %error, "inventory extraction attempt failed");
    if let Err(db_error) = fail_or_retry(pool, j, &error.to_string(), terminal, retry_after).await {
        tracing::error!(%db_error, import_id=%j.import_id, "could not update inventory import job");
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
    let attempts = sqlx::query_scalar::<_, i32>(
        "SELECT attempts FROM inventory_import_jobs
         WHERE import_id=$1 AND status='processing' AND lock_token=$2 FOR UPDATE",
    )
    .bind(j.import_id)
    .bind(j.lock_token)
    .fetch_optional(&mut *tx)
    .await?;
    let Some(attempts) = attempts else {
        tx.commit().await?;
        return Ok(());
    };
    let safe_error = error.chars().take(500).collect::<String>();
    if terminal || attempts >= MAX_ATTEMPTS {
        sqlx::query("UPDATE inventory_import_jobs SET status='failed',locked_at=NULL,lock_token=NULL,last_error=$3,updated_at=NOW() WHERE import_id=$1 AND lock_token=$2")
            .bind(j.import_id)
            .bind(j.lock_token)
            .bind(safe_error)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE inventory_imports SET status='failed',updated_at=NOW() WHERE id=$1 AND status='processing'")
            .bind(j.import_id)
            .execute(&mut *tx)
            .await?;
    } else {
        let delay = retry_delay(attempts, retry_after);
        sqlx::query("UPDATE inventory_import_jobs SET status='queued',available_at=NOW()+make_interval(secs=>$3::double precision),locked_at=NULL,lock_token=NULL,last_error=$4,updated_at=NOW() WHERE import_id=$1 AND lock_token=$2")
            .bind(j.import_id)
            .bind(j.lock_token)
            .bind(delay.as_secs_f64())
            .bind(safe_error)
            .execute(&mut *tx)
            .await?;
        tracing::info!(import_id=%j.import_id, attempts, retry_in_seconds=delay.as_secs(), "inventory extraction retry scheduled");
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
fn parse(b: &[u8]) -> Result<Vec<(Row, Vec<String>)>, ApiError> {
    use std::collections::HashMap;
    let mut rd = csv::ReaderBuilder::new()
        .flexible(false)
        .from_reader(b.strip_prefix(b"\xef\xbb\xbf").unwrap_or(b));
    let hs = rd
        .headers()
        .map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The CSV header is invalid.",
            )
        })?
        .clone();
    let allowed = ["name", "count_unit", "category", "par_level"];
    let mut ix = HashMap::new();
    for (i, x) in hs.iter().enumerate() {
        if !allowed.contains(&x) || ix.insert(x, i).is_some() {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "CSV headers must be unique v1 inventory headers.",
            ));
        }
    }
    if !ix.contains_key("name") || !ix.contains_key("count_unit") {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "CSV requires name and count_unit headers.",
        ));
    }
    let mut out = vec![];
    let mut errors = 0;
    for (n, x) in rd.records().enumerate() {
        if n >= MAX_ROWS {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "A CSV may contain no more than 2000 rows.",
            ));
        }
        let x =
            x.map_err(|_| ApiError(StatusCode::UNPROCESSABLE_ENTITY, "A CSV row is malformed."))?;
        let val = |k: &str| ix.get(k).map(|i| x[*i].trim().to_owned());
        let name = val("name").unwrap();
        let count_unit = val("count_unit").unwrap();
        let category = val("category").filter(|x| !x.is_empty());
        let par_level = val("par_level").filter(|x| !x.is_empty());
        let input = ItemInput {
            name: name.clone(),
            count_unit: count_unit.clone(),
            category: category.clone(),
            par_level: par_level.clone(),
            storage_area_id: None,
            shelf_order: 0,
            preferred_supplier_id: None,
            active: true,
        };
        let mut es = vec![];
        if let Err(e) = input.validated() {
            es.push(e.1.into());
            errors += 1;
        }
        if errors > MAX_ERRORS {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The CSV has too many validation errors.",
            ));
        }
        out.push((
            Row {
                id: Uuid::nil(),
                row_number: (n + 2) as i32,
                name,
                category,
                count_unit,
                par_level,
                validation_errors: serde_json::json!([]),
                selected: None,
                created_inventory_item_id: None,
            },
            es,
        ));
    }
    if out.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The CSV must contain at least one row.",
        ));
    }
    Ok(out)
}

fn is_spreadsheet(filename: &str) -> bool {
    let lower = filename.to_lowercase();
    lower.ends_with(".xlsx") || lower.ends_with(".xlsm") || lower.ends_with(".xls")
}

/// Renders a cell the way a CSV export would have: trimmed text, whole
/// numbers without a decimal tail, decimals without float noise.
fn cell_text(cell: &calamine::Data) -> String {
    match cell {
        calamine::Data::Empty => String::new(),
        calamine::Data::String(value) => value.trim().to_owned(),
        calamine::Data::Float(value) => format_number(*value),
        calamine::Data::Int(value) => value.to_string(),
        calamine::Data::Bool(value) => value.to_string(),
        calamine::Data::DateTime(value) => format_number(value.as_f64()),
        calamine::Data::DateTimeIso(value) | calamine::Data::DurationIso(value) => {
            value.trim().to_owned()
        }
        calamine::Data::Error(_) => String::new(),
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 9e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

/// Same v1 rules as the CSV parser — exact header set, unique columns,
/// required name/count_unit — applied to the workbook's first sheet.
fn parse_spreadsheet(b: &[u8]) -> Result<Vec<(Row, Vec<String>)>, ApiError> {
    use calamine::Reader as _;
    let mut workbook =
        calamine::open_workbook_auto_from_rs(std::io::Cursor::new(b)).map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "We couldn't read this spreadsheet. Try exporting it again.",
            )
        })?;
    let range = workbook
        .worksheet_range_at(0)
        .ok_or(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The spreadsheet has no sheets.",
        ))?
        .map_err(|_| {
            ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "We couldn't read this spreadsheet. Try exporting it again.",
            )
        })?;
    let mut rows = range.rows();
    let headers = rows.next().ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "The spreadsheet must have a header row.",
    ))?;
    let allowed = ["name", "count_unit", "category", "par_level"];
    let mut index = std::collections::HashMap::new();
    for (i, header) in headers.iter().enumerate() {
        let header = cell_text(header);
        if !allowed.contains(&header.as_str()) || index.insert(header.clone(), i).is_some() {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Spreadsheet headers must be unique v1 inventory headers.",
            ));
        }
    }
    if !index.contains_key("name") || !index.contains_key("count_unit") {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Spreadsheets require name and count_unit headers.",
        ));
    }

    let mut out = vec![];
    let mut errors = 0;
    for (n, record) in rows.enumerate() {
        if n >= SPREADSHEET_MAX_ROWS {
            return Err(ApiError(
                StatusCode::PAYLOAD_TOO_LARGE,
                "A spreadsheet may contain no more than 2000 rows.",
            ));
        }
        let value = |key: &str| -> String {
            index
                .get(key)
                .and_then(|i| record.get(*i))
                .map(cell_text)
                .unwrap_or_default()
        };
        let name = value("name");
        let count_unit = value("count_unit");
        let category = Some(value("category")).filter(|v| !v.is_empty());
        let par_level = Some(value("par_level")).filter(|v| !v.is_empty());
        let input = ItemInput {
            name: name.clone(),
            count_unit: count_unit.clone(),
            category: category.clone(),
            par_level: par_level.clone(),
            storage_area_id: None,
            shelf_order: 0,
            preferred_supplier_id: None,
            active: true,
        };
        let mut row_errors = vec![];
        if let Err(e) = input.validated() {
            row_errors.push(e.1.into());
            errors += 1;
        }
        if errors > SPREADSHEET_MAX_ERRORS {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "The spreadsheet has too many validation errors.",
            ));
        }
        out.push((
            Row {
                id: Uuid::nil(),
                row_number: (n + 2) as i32,
                name,
                category,
                count_unit,
                par_level,
                validation_errors: serde_json::json!([]),
                selected: None,
                created_inventory_item_id: None,
            },
            row_errors,
        ));
    }
    if out.is_empty() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "The spreadsheet must contain at least one row.",
        ));
    }
    Ok(out)
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::extraction::{ExtractedInventoryCsv, ExtractedInventoryItem};

    #[test]
    fn keeps_unclear_llm_inventory_values_for_review() {
        let rows = validate_extracted(ExtractedInventoryCsv {
            items: vec![ExtractedInventoryItem {
                name: " Chicken breast ".into(),
                category: Some(" Protein ".into()),
                count_unit: None,
                par_level: Some("25".into()),
            }],
        })
        .unwrap();
        assert_eq!(rows[0].0.name, "Chicken breast");
        assert_eq!(rows[0].0.category.as_deref(), Some("Protein"));
        assert!(!rows[0].1.is_empty());
    }

    #[test]
    fn csv_v1_validation() {
        assert!(parse(b"name,count_unit,category,par_level\nFlour,bag,Dry,2.5\n").is_ok());
        assert!(parse(b"name,quantity\nFlour,2\n").is_err());
        assert!(parse(b"name,count_unit,quantity\nFlour,bag,2\n").is_err());
    }

    mod cell {
        pub enum Cell {
            Text(&'static str),
            Number(f64),
        }
    }

    fn build_xlsx(headers: &[&'static str], rows: &[Vec<cell::Cell>]) -> Vec<u8> {
        use rust_xlsxwriter::Workbook;
        let mut workbook = Workbook::new();
        let sheet = workbook.add_worksheet();
        for (col, header) in headers.iter().enumerate() {
            sheet.write(0, col as u16, *header).unwrap();
        }
        for (i, row) in rows.iter().enumerate() {
            for (col, value) in row.iter().enumerate() {
                match value {
                    cell::Cell::Text(v) => {
                        sheet.write(i as u32 + 1, col as u16, *v).unwrap();
                    }
                    cell::Cell::Number(v) => {
                        sheet.write_number(i as u32 + 1, col as u16, *v).unwrap();
                    }
                }
            }
        }
        workbook.save_to_buffer().unwrap()
    }

    #[test]
    fn spreadsheet_v1_validation() {
        let valid = build_xlsx(
            &["name", "count_unit", "category", "par_level"],
            &[vec![
                cell::Cell::Text("Flour"),
                cell::Cell::Text("bag"),
                cell::Cell::Text("Dry"),
                cell::Cell::Number(2.5),
            ]],
        );
        let parsed = parse_spreadsheet(&valid).unwrap();
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].0.name, "Flour");
        // Numeric par levels read like a CSV export would have written them.
        assert_eq!(parsed[0].0.par_level.as_deref(), Some("2.5"));
        assert!(parsed[0].1.is_empty());

        // Only name and count_unit are required headers.
        let minimal = parse_spreadsheet(&build_xlsx(
            &["name", "count_unit"],
            &[vec![cell::Cell::Text("Flour"), cell::Cell::Number(12.0)]],
        ))
        .unwrap();
        assert_eq!(minimal.len(), 1);
        assert_eq!(minimal[0].0.count_unit, "12");
        assert!(
            parse_spreadsheet(&build_xlsx(
                &["name", "count_unit", "quantity"],
                &[vec![
                    cell::Cell::Text("Flour"),
                    cell::Cell::Text("bag"),
                    cell::Cell::Number(2.0)
                ]]
            ))
            .is_err()
        );
        assert!(parse_spreadsheet(&build_xlsx(&["name", "count_unit"], &[],)).is_err());

        // Whole numbers keep no decimal tail.
        let whole = parse_spreadsheet(&build_xlsx(
            &["name", "count_unit", "par_level"],
            &[vec![
                cell::Cell::Text("Onions"),
                cell::Cell::Text("case"),
                cell::Cell::Number(4.0),
            ]],
        ))
        .unwrap();
        assert_eq!(whole[0].0.par_level.as_deref(), Some("4"));
    }
}
