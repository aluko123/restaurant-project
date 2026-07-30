use crate::{
    ApiError, AppState, authenticated_subject, database_error, inventory::ItemInput,
    uploads::multipart_error,
};
use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;
const MAX_BYTES: usize = 1024 * 1024;
const MAX_ROWS: usize = 2000;
const MAX_ERRORS: usize = 25;
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
        "Attach one CSV file in the file field.",
    ))?;
    if b.is_empty() || b.len() > MAX_BYTES {
        return Err(ApiError(
            StatusCode::PAYLOAD_TOO_LARGE,
            "CSV files must be between 1 byte and 1 MiB.",
        ));
    }
    let parsed = parse(&b)?;
    let hash = format!("{:x}", Sha256::digest(&b));
    let existing: HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT LOWER(BTRIM(name)) FROM inventory_items WHERE restaurant_id=$1",
    )
    .bind(m.restaurant_id)
    .fetch_all(&s.pool)
    .await
    .map_err(database_error)?
    .into_iter()
    .collect();
    let id = Uuid::now_v7();
    let mut tx = s.pool.begin().await.map_err(database_error)?;
    let inserted = sqlx::query_scalar::<_, Uuid>("INSERT INTO inventory_imports(id,restaurant_id,original_filename,content_hash,created_by)VALUES($1,$2,$3,$4,$5) ON CONFLICT (restaurant_id,content_hash) DO NOTHING RETURNING id").bind(id).bind(m.restaurant_id).bind(name).bind(&hash).bind(m.user_id).fetch_optional(&mut*tx).await.map_err(database_error)?;
    if inserted.is_none() {
        let existing = sqlx::query_scalar::<_, Uuid>(
            "SELECT id FROM inventory_imports WHERE restaurant_id=$1 AND content_hash=$2",
        )
        .bind(m.restaurant_id)
        .bind(hash)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        tx.commit().await.map_err(database_error)?;
        return Ok((
            StatusCode::OK,
            Json(load(&s, existing, m.restaurant_id).await?),
        ));
    }
    let mut seen = HashSet::new();
    for (r, mut errors) in parsed {
        let key = r.name.trim().to_lowercase();
        if !seen.insert(key.clone()) {
            errors.push("Duplicate normalized name in this file.".into())
        }
        if existing.contains(&key) {
            errors.push("An inventory item with this name already exists.".into())
        }
        sqlx::query("INSERT INTO inventory_import_rows(id,restaurant_id,import_id,row_number,name,category,count_unit,par_level,validation_errors)VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(Uuid::now_v7()).bind(m.restaurant_id).bind(id).bind(r.row_number).bind(r.name).bind(r.category).bind(r.count_unit).bind(r.par_level).bind(serde_json::json!(errors)).execute(&mut*tx).await.map_err(database_error)?;
    }
    tx.commit().await.map_err(database_error)?;
    Ok((
        StatusCode::CREATED,
        Json(load(&s, id, m.restaurant_id).await?),
    ))
}
pub(crate) async fn get(
    State(s): State<AppState>,
    h: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<Import>, ApiError> {
    let m = member(&s, &h).await?;
    Ok(Json(load(&s, id, m.restaurant_id).await?))
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
    let head=sqlx::query_as("SELECT id,original_filename,content_hash,status,revision FROM inventory_imports WHERE id=$1 AND restaurant_id=$2").bind(id).bind(r).fetch_optional(&s.pool).await.map_err(database_error)?.ok_or(ApiError(StatusCode::NOT_FOUND,"Inventory import not found."))?;
    let rows=sqlx::query_as("SELECT id,row_number,name,category,count_unit,par_level,validation_errors,selected,created_inventory_item_id FROM inventory_import_rows WHERE import_id=$1 ORDER BY row_number").bind(id).fetch_all(&s.pool).await.map_err(database_error)?;
    Ok(Import { head, rows })
}
fn parse(b: &[u8]) -> Result<Vec<(Row, Vec<String>)>, ApiError> {
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
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn csv_v1_validation() {
        assert!(parse(b"name,count_unit,category,par_level\nFlour,bag,Dry,2.5\n").is_ok());
        assert!(parse(b"name,quantity\nFlour,2\n").is_err());
        assert!(parse(b"name,count_unit,quantity\nFlour,bag,2\n").is_err());
    }
}
