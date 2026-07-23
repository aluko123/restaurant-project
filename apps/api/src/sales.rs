use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Multipart, Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authenticated_subject, database_error, invoices::strict_decimal,
    uploads::multipart_error,
};

const MAX_SALES_LINES: usize = 200;
const MAX_CSV_BYTES: usize = 1024 * 1024;
const MAX_CSV_ROWS: usize = 2_000;
const MAX_CSV_ERRORS: usize = 25;

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SalesDaySummary {
    business_date: NaiveDate,
    revision: i64,
    line_count: i64,
    total_quantity: String,
    reported_line_count: i64,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct MenuOption {
    id: Uuid,
    name: String,
    category: Option<String>,
    currency: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SalesDay {
    id: Uuid,
    business_date: NaiveDate,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    lines: Vec<SalesLine>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SalesLine {
    menu_item_id: Uuid,
    menu_item_name: String,
    quantity: String,
    reported_net_sales: Option<String>,
    currency: Option<String>,
}

#[derive(sqlx::FromRow)]
struct DayHeader {
    id: Uuid,
    business_date: NaiveDate,
    revision: i64,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug, PartialEq, sqlx::FromRow)]
struct StoredLine {
    menu_item_id: Uuid,
    menu_item_name: String,
    quantity: BigDecimal,
    reported_net_sales: Option<BigDecimal>,
    currency: Option<String>,
}

#[derive(sqlx::FromRow)]
struct MenuRecord {
    id: Uuid,
    name: String,
    currency: String,
    active: bool,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SalesImportPreview {
    original_filename: String,
    business_date: NaiveDate,
    rows: Vec<SalesImportRow>,
    existing_day: Option<SalesDay>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SalesImportRow {
    row_number: u64,
    raw_item_label: String,
    item_code: Option<String>,
    quantity: String,
    reported_net_sales: Option<String>,
    currency: Option<String>,
    match_status: &'static str,
    matched_menu_item_id: Option<Uuid>,
    matched_menu_item_name: Option<String>,
    matched_menu_item_currency: Option<String>,
    validation_errors: Vec<String>,
}

#[derive(Debug)]
struct ParsedCsv {
    business_date: NaiveDate,
    rows: Vec<ParsedCsvRow>,
}

#[derive(Debug)]
struct ParsedCsvRow {
    row_number: u64,
    raw_item_label: String,
    item_code: Option<String>,
    quantity: String,
    reported_net_sales: Option<String>,
    currency: Option<String>,
}

#[derive(Debug, PartialEq)]
struct CsvIssue {
    row_number: Option<u64>,
    message: String,
}

#[derive(Debug)]
pub(crate) struct SalesImportError {
    status: StatusCode,
    message: String,
}

#[derive(Serialize)]
struct SalesImportErrorBody<'a> {
    error: &'a str,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct SaveSalesDay {
    expected_revision: ExpectedRevision,
    lines: Vec<SaveLine>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ExpectedRevision {
    Revision(i64),
    Create(()),
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SaveLine {
    menu_item_id: Uuid,
    quantity: String,
    reported_net_sales: Option<String>,
    #[serde(default)]
    currency: Option<String>,
}

#[derive(Debug)]
struct ValidatedSave {
    expected_revision: Option<i64>,
    lines: Vec<ValidatedLine>,
}

#[derive(Clone, Debug, PartialEq)]
struct ValidatedLine {
    menu_item_id: Uuid,
    quantity: BigDecimal,
    reported_net_sales: Option<BigDecimal>,
    reported_currency: Option<String>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<SalesDaySummary>>, ApiError> {
    let member = membership(&state, &headers).await?;
    let days = sqlx::query_as::<_, SalesDaySummary>(
        "SELECT d.business_date,d.revision,COUNT(l.menu_item_id)::bigint AS line_count,
                COALESCE(SUM(l.quantity),0)::text AS total_quantity,
                COUNT(l.reported_net_sales)::bigint AS reported_line_count,d.updated_at
         FROM sales_days d
         LEFT JOIN sales_lines l ON l.sales_day_id=d.id AND l.restaurant_id=d.restaurant_id
         WHERE d.restaurant_id=$1
         GROUP BY d.id,d.business_date,d.revision,d.updated_at
         ORDER BY d.business_date DESC,d.updated_at DESC,d.id DESC
         LIMIT 30",
    )
    .bind(member.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(days))
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(business_date): Path<String>,
) -> Result<Json<SalesDay>, ApiError> {
    let member = membership(&state, &headers).await?;
    let business_date = parse_business_date(&business_date)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let header = load_header(&mut tx, member.restaurant_id, business_date, "FOR SHARE")
        .await?
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "No sales are saved for this date.",
        ))?;
    let lines = load_lines(&mut tx, header.id, member.restaurant_id).await?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(day_response(header, lines)))
}

pub(crate) async fn menu_options(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<MenuOption>>, ApiError> {
    let member = membership(&state, &headers).await?;
    let options = sqlx::query_as::<_, MenuOption>(
        "SELECT id,name,category,currency FROM menu_items
         WHERE restaurant_id=$1 AND active
         ORDER BY category NULLS LAST,LOWER(name),id",
    )
    .bind(member.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(options))
}

pub(crate) async fn preview_import(
    State(state): State<AppState>,
    headers: HeaderMap,
    mut multipart: Multipart,
) -> Result<Json<SalesImportPreview>, SalesImportError> {
    let member = membership(&state, &headers).await?;
    require_manager(&member.role)?;
    let mut upload = None;
    while let Some(field) = multipart
        .next_field()
        .await
        .map_err(|error| SalesImportError::from(multipart_error(error)))?
    {
        if field.name() != Some("file") {
            continue;
        }
        if upload.is_some() {
            return Err(SalesImportError::unprocessable(
                "Upload one CSV file at a time.".into(),
            ));
        }
        let filename = field.file_name().unwrap_or("").to_owned();
        let bytes = field
            .bytes()
            .await
            .map_err(|error| SalesImportError::from(multipart_error(error)))?;
        upload = Some((filename, bytes));
    }
    let (original_filename, bytes) = upload.ok_or_else(|| {
        SalesImportError::unprocessable("Choose a Parline CSV file to preview.".into())
    })?;
    validate_csv_upload(&original_filename, &bytes)?;
    let parsed = parse_sales_csv(&bytes).map_err(SalesImportError::csv_validation)?;

    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let menu = sqlx::query_as::<_, MenuRecord>(
        "SELECT id,name,currency,active FROM menu_items
         WHERE restaurant_id=$1 AND active
         ORDER BY LOWER(name),id",
    )
    .bind(member.restaurant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(database_error)?;
    let menu_by_name = menu_name_index(menu);
    let rows = parsed
        .rows
        .into_iter()
        .map(|row| {
            let matched = menu_by_name.get(&normalized_menu_name(&row.raw_item_label));
            let mut validation_errors = Vec::new();
            if let (Some(currency), Some(item)) = (&row.currency, matched)
                && currency != &item.currency
            {
                validation_errors.push(format!(
                    "Reported net sales use {currency}, but {} uses {}.",
                    item.name, item.currency
                ));
            }
            SalesImportRow {
                row_number: row.row_number,
                raw_item_label: row.raw_item_label,
                item_code: row.item_code,
                quantity: row.quantity,
                reported_net_sales: row.reported_net_sales,
                currency: row.currency,
                match_status: if matched.is_some() {
                    "matched"
                } else {
                    "unmatched"
                },
                matched_menu_item_id: matched.map(|item| item.id),
                matched_menu_item_name: matched.map(|item| item.name.clone()),
                matched_menu_item_currency: matched.map(|item| item.currency.clone()),
                validation_errors,
            }
        })
        .collect();
    let existing_day = match load_header(
        &mut tx,
        member.restaurant_id,
        parsed.business_date,
        "FOR SHARE",
    )
    .await?
    {
        Some(header) => {
            let lines = load_lines(&mut tx, header.id, member.restaurant_id).await?;
            Some(day_response(header, lines))
        }
        None => None,
    };
    tx.commit().await.map_err(database_error)?;
    Ok(Json(SalesImportPreview {
        original_filename,
        business_date: parsed.business_date,
        rows,
        existing_day,
    }))
}

pub(crate) async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(business_date): Path<String>,
    Json(input): Json<SaveSalesDay>,
) -> Result<Json<SalesDay>, ApiError> {
    let member = membership(&state, &headers).await?;
    let business_date = parse_business_date(&business_date)?;
    let input = input.validated()?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;

    // A restaurant row provides a stable lock even before the canonical day exists.
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(member.restaurant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
    let current = load_header(&mut tx, member.restaurant_id, business_date, "FOR UPDATE").await?;
    let current_lines = match &current {
        Some(header) => load_lines(&mut tx, header.id, member.restaurant_id).await?,
        None => Vec::new(),
    };

    if let Some(header) = current {
        if replay_matches(&input.lines, &current_lines)
            && replay_revision_matches(input.expected_revision, header.revision)
        {
            tx.commit().await.map_err(database_error)?;
            return Ok(Json(day_response(header, current_lines)));
        }
        if input.expected_revision != Some(header.revision) {
            return Err(stale_error());
        }

        let existing_ids = current_lines.iter().map(|line| line.menu_item_id).collect();
        let lines =
            hydrate_lines(&mut tx, member.restaurant_id, input.lines, &existing_ids).await?;
        sqlx::query("DELETE FROM sales_lines WHERE sales_day_id=$1 AND restaurant_id=$2")
            .bind(header.id)
            .bind(member.restaurant_id)
            .execute(&mut *tx)
            .await
            .map_err(database_error)?;
        insert_lines(&mut tx, header.id, member.restaurant_id, &lines).await?;
        let header = sqlx::query_as::<_, DayHeader>(
            "UPDATE sales_days
             SET revision=revision+1,updated_by=$3,updated_at=clock_timestamp()
             WHERE id=$1 AND restaurant_id=$2
             RETURNING id,business_date,revision,created_at,updated_at",
        )
        .bind(header.id)
        .bind(member.restaurant_id)
        .bind(member.user_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
        let lines = load_lines(&mut tx, header.id, member.restaurant_id).await?;
        tx.commit().await.map_err(database_error)?;
        return Ok(Json(day_response(header, lines)));
    }

    if input.expected_revision.is_some() {
        return Err(stale_error());
    }
    let lines = hydrate_lines(&mut tx, member.restaurant_id, input.lines, &HashSet::new()).await?;
    let header = sqlx::query_as::<_, DayHeader>(
        "INSERT INTO sales_days(id,restaurant_id,business_date,created_by,updated_by)
         VALUES($1,$2,$3,$4,$4)
         RETURNING id,business_date,revision,created_at,updated_at",
    )
    .bind(Uuid::now_v7())
    .bind(member.restaurant_id)
    .bind(business_date)
    .bind(member.user_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    insert_lines(&mut tx, header.id, member.restaurant_id, &lines).await?;
    let lines = load_lines(&mut tx, header.id, member.restaurant_id).await?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(day_response(header, lines)))
}

async fn membership(state: &AppState, headers: &HeaderMap) -> Result<Membership, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as::<_, Membership>(
        "SELECT m.restaurant_id,u.id AS user_id,m.role
         FROM users u JOIN restaurant_memberships m ON m.user_id=u.id
         WHERE u.auth_subject=$1",
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

fn require_manager(role: &str) -> Result<(), ApiError> {
    if matches!(role, "owner" | "manager") {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Owner or manager access is required to import sales.",
        ))
    }
}

fn parse_business_date(value: &str) -> Result<NaiveDate, ApiError> {
    let date = NaiveDate::parse_from_str(value, "%Y-%m-%d").map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Business date must use YYYY-MM-DD.",
        )
    })?;
    if date.format("%Y-%m-%d").to_string() != value {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Business date must use YYYY-MM-DD.",
        ));
    }
    Ok(date)
}

fn stale_error() -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        "Sales for this date changed. Reload the saved day before trying again.",
    )
}

impl SaveSalesDay {
    fn validated(self) -> Result<ValidatedSave, ApiError> {
        let expected_revision = match self.expected_revision {
            ExpectedRevision::Create(()) => None,
            ExpectedRevision::Revision(revision) if revision >= 1 => Some(revision),
            ExpectedRevision::Revision(_) => {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Expected revision must be null or a positive integer.",
                ));
            }
        };
        if self.lines.is_empty() || self.lines.len() > MAX_SALES_LINES {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose between 1 and 200 menu items for the sales day.",
            ));
        }
        let mut seen = HashSet::with_capacity(self.lines.len());
        let mut lines = Vec::with_capacity(self.lines.len());
        for line in self.lines {
            if !seen.insert(line.menu_item_id) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Each menu item may appear only once in a sales day.",
                ));
            }
            let quantity = sales_decimal(&line.quantity, 6).map_err(|_| {
                ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Quantity must be a positive plain decimal with at most 6 decimal places.",
                )
            })?;
            if quantity <= 0 {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Quantity must be greater than zero.",
                ));
            }
            let reported_net_sales = line
                .reported_net_sales
                .as_deref()
                .map(|value| sales_decimal(value, 4))
                .transpose()
                .map_err(|_| {
                    ApiError(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Reported net sales must be a nonnegative plain decimal with at most 4 decimal places.",
                    )
                })?;
            let reported_currency = line
                .currency
                .map(|currency| currency.trim().to_ascii_uppercase());
            if reported_currency.as_ref().is_some_and(|currency| {
                currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase())
            }) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Currency must be a three-letter code such as USD.",
                ));
            }
            if reported_currency.is_some() && reported_net_sales.is_none() {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Currency may only be provided with reported net sales.",
                ));
            }
            lines.push(ValidatedLine {
                menu_item_id: line.menu_item_id,
                quantity,
                reported_net_sales,
                reported_currency,
            });
        }
        lines.sort_by_key(|line| line.menu_item_id);
        Ok(ValidatedSave {
            expected_revision,
            lines,
        })
    }
}

fn sales_decimal(value: &str, scale: usize) -> Result<BigDecimal, ()> {
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    if value.is_empty()
        || value.starts_with(['+', '-'])
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || value.contains('.') && fraction.is_empty()
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(());
    }
    strict_decimal(value, scale)
        .map(|decimal| decimal.normalized())
        .map_err(|_| ())
}

fn replay_matches(input: &[ValidatedLine], stored: &[StoredLine]) -> bool {
    input.len() == stored.len()
        && input.iter().zip(stored).all(|(input, stored)| {
            input.menu_item_id == stored.menu_item_id
                && input.quantity == stored.quantity
                && input.reported_net_sales == stored.reported_net_sales
                && input
                    .reported_currency
                    .as_ref()
                    .is_none_or(|currency| stored.currency.as_ref() == Some(currency))
        })
}

fn replay_revision_matches(expected: Option<i64>, current: i64) -> bool {
    match expected {
        None => current == 1,
        Some(expected) => expected == current || expected.checked_add(1) == Some(current),
    }
}

async fn load_header(
    connection: &mut PgConnection,
    restaurant_id: Uuid,
    business_date: NaiveDate,
    lock: &str,
) -> Result<Option<DayHeader>, ApiError> {
    let query = format!(
        "SELECT id,business_date,revision,created_at,updated_at FROM sales_days
         WHERE restaurant_id=$1 AND business_date=$2 {lock}"
    );
    sqlx::query_as::<_, DayHeader>(&query)
        .bind(restaurant_id)
        .bind(business_date)
        .fetch_optional(connection)
        .await
        .map_err(database_error)
}

async fn load_lines(
    connection: &mut PgConnection,
    sales_day_id: Uuid,
    restaurant_id: Uuid,
) -> Result<Vec<StoredLine>, ApiError> {
    sqlx::query_as::<_, StoredLine>(
        "SELECT menu_item_id,menu_item_name,quantity,reported_net_sales,currency
         FROM sales_lines WHERE sales_day_id=$1 AND restaurant_id=$2
         ORDER BY menu_item_id",
    )
    .bind(sales_day_id)
    .bind(restaurant_id)
    .fetch_all(connection)
    .await
    .map_err(database_error)
}

async fn hydrate_lines(
    connection: &mut PgConnection,
    restaurant_id: Uuid,
    lines: Vec<ValidatedLine>,
    existing_ids: &HashSet<Uuid>,
) -> Result<Vec<StoredLine>, ApiError> {
    let ids = lines
        .iter()
        .map(|line| line.menu_item_id)
        .collect::<Vec<_>>();
    let menu = sqlx::query_as::<_, MenuRecord>(
        "SELECT id,name,currency,active FROM menu_items
         WHERE restaurant_id=$1 AND id=ANY($2)",
    )
    .bind(restaurant_id)
    .bind(&ids)
    .fetch_all(connection)
    .await
    .map_err(database_error)?
    .into_iter()
    .map(|item| (item.id, item))
    .collect::<HashMap<_, _>>();
    if menu.len() != lines.len() {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Every sales line must use a menu item from this restaurant.",
        ));
    }
    lines
        .into_iter()
        .map(|line| {
            let item = &menu[&line.menu_item_id];
            if !item.active && !existing_ids.contains(&line.menu_item_id) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "New sales lines must use active menu items.",
                ));
            }
            validate_reported_currency(
                line.reported_currency.as_deref(),
                line.reported_net_sales.is_some(),
                &item.currency,
            )?;
            Ok(StoredLine {
                menu_item_id: line.menu_item_id,
                menu_item_name: item.name.clone(),
                quantity: line.quantity,
                currency: line
                    .reported_net_sales
                    .as_ref()
                    .map(|_| item.currency.clone()),
                reported_net_sales: line.reported_net_sales,
            })
        })
        .collect()
}

async fn insert_lines(
    connection: &mut PgConnection,
    sales_day_id: Uuid,
    restaurant_id: Uuid,
    lines: &[StoredLine],
) -> Result<(), ApiError> {
    for line in lines {
        sqlx::query(
            "INSERT INTO sales_lines(
                sales_day_id,restaurant_id,menu_item_id,menu_item_name,quantity,
                reported_net_sales,currency)
             VALUES($1,$2,$3,$4,$5,$6,$7)",
        )
        .bind(sales_day_id)
        .bind(restaurant_id)
        .bind(line.menu_item_id)
        .bind(&line.menu_item_name)
        .bind(&line.quantity)
        .bind(&line.reported_net_sales)
        .bind(&line.currency)
        .execute(&mut *connection)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

fn day_response(header: DayHeader, lines: Vec<StoredLine>) -> SalesDay {
    SalesDay {
        id: header.id,
        business_date: header.business_date,
        revision: header.revision,
        created_at: header.created_at,
        updated_at: header.updated_at,
        lines: lines
            .into_iter()
            .map(|line| SalesLine {
                menu_item_id: line.menu_item_id,
                menu_item_name: line.menu_item_name,
                quantity: line.quantity.to_string(),
                reported_net_sales: line.reported_net_sales.map(|value| value.to_string()),
                currency: line.currency,
            })
            .collect(),
    }
}

fn validate_reported_currency(
    requested: Option<&str>,
    has_reported_sales: bool,
    menu_currency: &str,
) -> Result<(), ApiError> {
    if has_reported_sales && requested.is_some_and(|currency| currency != menu_currency) {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Reported net sales currency must match the selected menu item.",
        ));
    }
    Ok(())
}

fn normalized_menu_name(value: &str) -> String {
    value.trim().to_lowercase()
}

fn menu_name_index(menu: Vec<MenuRecord>) -> HashMap<String, MenuRecord> {
    let mut index = HashMap::with_capacity(menu.len());
    let mut ambiguous = HashSet::new();
    for item in menu {
        let key = normalized_menu_name(&item.name);
        if ambiguous.contains(&key) {
            continue;
        }
        if index.insert(key.clone(), item).is_some() {
            index.remove(&key);
            ambiguous.insert(key);
        }
    }
    index
}

fn validate_csv_upload(filename: &str, bytes: &[u8]) -> Result<(), SalesImportError> {
    if filename.trim().is_empty()
        || filename.chars().count() > 255
        || filename.chars().any(char::is_control)
    {
        return Err(SalesImportError::unprocessable(
            "The CSV filename is missing or too long.".into(),
        ));
    }
    if bytes.is_empty() || bytes.len() > MAX_CSV_BYTES {
        return Err(SalesImportError {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: "CSV files must be between 1 byte and 1 MiB.".into(),
        });
    }
    Ok(())
}

fn parse_sales_csv(bytes: &[u8]) -> Result<ParsedCsv, Vec<CsvIssue>> {
    const REQUIRED_HEADERS: [&str; 3] = ["business_date", "item_name", "quantity"];
    const ALLOWED_HEADERS: [&str; 6] = [
        "business_date",
        "item_name",
        "quantity",
        "item_code",
        "net_sales",
        "currency",
    ];

    let bytes = bytes.strip_prefix(b"\xef\xbb\xbf").unwrap_or(bytes);
    let mut reader = csv::ReaderBuilder::new()
        .has_headers(true)
        .flexible(false)
        .from_reader(bytes);
    let headers = match reader.headers() {
        Ok(headers) => headers.clone(),
        Err(error) => {
            return Err(vec![CsvIssue {
                row_number: error.position().map(|position| position.line()),
                message: format!("The CSV header could not be read: {error}."),
            }]);
        }
    };
    let mut issues = Vec::new();
    let mut indexes = HashMap::new();
    for (index, header) in headers.iter().enumerate() {
        if !ALLOWED_HEADERS.contains(&header) {
            push_csv_issue(
                &mut issues,
                None,
                format!("Unknown header '{header}'. Use the Parline CSV template headers."),
            );
        } else if indexes.insert(header, index).is_some() {
            push_csv_issue(
                &mut issues,
                None,
                format!("Header '{header}' appears more than once."),
            );
        }
    }
    for required in REQUIRED_HEADERS {
        if !indexes.contains_key(required) {
            push_csv_issue(
                &mut issues,
                None,
                format!("Required header '{required}' is missing."),
            );
        }
    }
    if !issues.is_empty() {
        return Err(issues);
    }

    let mut business_date = None;
    let mut rows = Vec::new();
    for result in reader.records() {
        let record = match result {
            Ok(record) => record,
            Err(error) => {
                push_csv_issue(
                    &mut issues,
                    error.position().map(|position| position.line()),
                    format!("This row is not valid comma-delimited CSV: {error}."),
                );
                continue;
            }
        };
        let row_number = record
            .position()
            .map(|position| position.line())
            .unwrap_or((rows.len() + 2) as u64);
        if rows.len() >= MAX_CSV_ROWS {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                format!("A CSV may contain no more than {MAX_CSV_ROWS} data rows."),
            );
            break;
        }
        let date_value = csv_value(&record, &indexes, "business_date").trim();
        let row_date = NaiveDate::parse_from_str(date_value, "%Y-%m-%d")
            .ok()
            .filter(|date| date.format("%Y-%m-%d").to_string() == date_value);
        match row_date {
            None => push_csv_issue(
                &mut issues,
                Some(row_number),
                "Business date must use YYYY-MM-DD.".into(),
            ),
            Some(date) if business_date.is_some_and(|current| current != date) => push_csv_issue(
                &mut issues,
                Some(row_number),
                "Every row must use the same business date.".into(),
            ),
            Some(date) => business_date = Some(date),
        }

        let raw_item_label = csv_value(&record, &indexes, "item_name").to_owned();
        let item_name = raw_item_label.trim();
        if item_name.is_empty() || item_name.chars().count() > 50 {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Item name must be between 1 and 50 characters.".into(),
            );
        }
        let item_code = indexes.get("item_code").and_then(|index| {
            let item_code = record.get(*index).unwrap_or("").trim();
            (!item_code.is_empty()).then(|| item_code.to_owned())
        });
        if item_code
            .as_ref()
            .is_some_and(|item_code| item_code.chars().count() > 120)
        {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Item code must be no more than 120 characters.".into(),
            );
        }

        let quantity = csv_value(&record, &indexes, "quantity").trim().to_owned();
        let parsed_quantity = sales_decimal(&quantity, 6);
        if parsed_quantity.is_err()
            || parsed_quantity
                .as_ref()
                .is_ok_and(|quantity| quantity <= &BigDecimal::from(0))
        {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Quantity must be a positive plain decimal with at most 6 decimal places.".into(),
            );
        }
        let reported_net_sales = indexes.get("net_sales").and_then(|index| {
            let net_sales = record.get(*index).unwrap_or("").trim();
            (!net_sales.is_empty()).then(|| net_sales.to_owned())
        });
        if reported_net_sales.as_ref().is_some_and(|net_sales| {
            sales_decimal(net_sales, 4)
                .map(|net_sales| net_sales < 0)
                .unwrap_or(true)
        }) {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Net sales must be a nonnegative plain decimal with at most 4 decimal places."
                    .into(),
            );
        }
        let currency = indexes.get("currency").and_then(|index| {
            let currency = record.get(*index).unwrap_or("").trim();
            (!currency.is_empty()).then(|| currency.to_ascii_uppercase())
        });
        if currency.as_ref().is_some_and(|currency| {
            currency.len() != 3 || !currency.bytes().all(|byte| byte.is_ascii_uppercase())
        }) {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Currency must be a three-letter code such as USD.".into(),
            );
        }
        if reported_net_sales.is_some() && currency.is_none() {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Currency is required when net sales are provided.".into(),
            );
        } else if reported_net_sales.is_none() && currency.is_some() {
            push_csv_issue(
                &mut issues,
                Some(row_number),
                "Leave currency blank when net sales are blank.".into(),
            );
        }
        rows.push(ParsedCsvRow {
            row_number,
            raw_item_label,
            item_code,
            quantity,
            reported_net_sales,
            currency,
        });
    }
    if rows.is_empty() {
        push_csv_issue(
            &mut issues,
            None,
            "The CSV must contain at least one data row.".into(),
        );
    }
    if !issues.is_empty() {
        return Err(issues);
    }
    Ok(ParsedCsv {
        business_date: business_date.expect("a valid nonempty CSV has a business date"),
        rows,
    })
}

fn push_csv_issue(issues: &mut Vec<CsvIssue>, row_number: Option<u64>, message: String) {
    if issues.len() < MAX_CSV_ERRORS {
        issues.push(CsvIssue {
            row_number,
            message,
        });
    }
}

fn csv_value<'a>(
    record: &'a csv::StringRecord,
    indexes: &HashMap<&str, usize>,
    name: &str,
) -> &'a str {
    record.get(indexes[name]).unwrap_or("")
}

impl SalesImportError {
    fn unprocessable(message: String) -> Self {
        Self {
            status: StatusCode::UNPROCESSABLE_ENTITY,
            message,
        }
    }

    fn csv_validation(issues: Vec<CsvIssue>) -> Self {
        let details = issues
            .into_iter()
            .map(|issue| match issue.row_number {
                Some(row) => format!("Row {row}: {}", issue.message),
                None => issue.message,
            })
            .collect::<Vec<_>>()
            .join(" • ");
        Self::unprocessable(format!("The CSV needs correction. {details}"))
    }
}

impl From<ApiError> for SalesImportError {
    fn from(error: ApiError) -> Self {
        Self {
            status: error.0,
            message: error.1.into(),
        }
    }
}

impl IntoResponse for SalesImportError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(SalesImportErrorBody {
                error: &self.message,
            }),
        )
            .into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(expected_revision: serde_json::Value, lines: serde_json::Value) -> SaveSalesDay {
        serde_json::from_value(serde_json::json!({
            "expectedRevision": expected_revision,
            "lines": lines
        }))
        .unwrap()
    }

    #[test]
    fn validates_and_canonicalizes_sales_payload() {
        let first = Uuid::from_u128(2);
        let second = Uuid::from_u128(1);
        let value = input(
            serde_json::Value::Null,
            serde_json::json!([
                {"menuItemId": first, "quantity": "2.500000", "reportedNetSales": "0.00"},
                {"menuItemId": second, "quantity": "1.25", "reportedNetSales": null}
            ]),
        )
        .validated()
        .unwrap();
        assert_eq!(value.expected_revision, None);
        assert_eq!(value.lines[0].menu_item_id, second);
        assert_eq!(value.lines[0].quantity.to_string(), "1.25");
        assert_eq!(value.lines[1].quantity.to_string(), "2.5");
        assert_eq!(
            value.lines[1].reported_net_sales.as_ref().unwrap(),
            &BigDecimal::from(0)
        );
    }

    #[test]
    fn rejects_incomplete_duplicate_and_invalid_decimals() {
        assert!(serde_json::from_value::<SaveSalesDay>(serde_json::json!({"lines": []})).is_err());
        let id = Uuid::from_u128(1);
        assert!(
            input(
                serde_json::json!(1),
                serde_json::json!([
                    {"menuItemId": id, "quantity": "1"},
                    {"menuItemId": id, "quantity": "2"}
                ]),
            )
            .validated()
            .is_err()
        );
        for quantity in ["0", "+1", "1.", "1.0000001", "1e2", " 1"] {
            assert!(
                input(
                    serde_json::Value::Null,
                    serde_json::json!([{"menuItemId": id, "quantity": quantity}]),
                )
                .validated()
                .is_err(),
                "quantity {quantity} should be rejected"
            );
        }
        assert!(
            input(
                serde_json::Value::Null,
                serde_json::json!([{"menuItemId": id, "quantity": "1", "reportedNetSales": "-1"}]),
            )
            .validated()
            .is_err()
        );
    }

    #[test]
    fn replay_comparison_ignores_order_and_decimal_scale_after_canonicalization() {
        let first = Uuid::from_u128(1);
        let second = Uuid::from_u128(2);
        let saved = vec![
            StoredLine {
                menu_item_id: first,
                menu_item_name: "Burger".into(),
                quantity: "1.000000".parse().unwrap(),
                reported_net_sales: Some("12.5000".parse().unwrap()),
                currency: Some("USD".into()),
            },
            StoredLine {
                menu_item_id: second,
                menu_item_name: "Fries".into(),
                quantity: "2.500000".parse().unwrap(),
                reported_net_sales: None,
                currency: None,
            },
        ];
        let request = input(
            serde_json::json!(1),
            serde_json::json!([
                {"menuItemId": second, "quantity": "2.5"},
                {"menuItemId": first, "quantity": "1", "reportedNetSales": "12.50"}
            ]),
        )
        .validated()
        .unwrap();
        assert!(replay_matches(&request.lines, &saved));
    }

    #[test]
    fn replay_requires_the_current_or_immediately_preceding_revision() {
        assert!(replay_revision_matches(None, 1));
        assert!(!replay_revision_matches(None, 2));
        assert!(replay_revision_matches(Some(4), 4));
        assert!(replay_revision_matches(Some(4), 5));
        assert!(!replay_revision_matches(Some(3), 5));
        assert!(!replay_revision_matches(Some(6), 5));
    }

    #[test]
    fn only_owner_and_manager_can_import() {
        assert!(require_manager("owner").is_ok());
        assert!(require_manager("manager").is_ok());
        assert!(require_manager("staff").is_err());
    }

    #[test]
    fn business_date_must_be_canonical() {
        assert!(parse_business_date("2026-07-22").is_ok());
        assert!(parse_business_date("2026-7-22").is_err());
        assert!(parse_business_date("not-a-date").is_err());
    }

    #[test]
    fn parses_bom_quoted_csv_without_losing_exact_decimal_text() {
        let parsed = parse_sales_csv(
            b"\xef\xbb\xbfbusiness_date,item_name,quantity,item_code,net_sales,currency\n\
              2026-07-21,\"Burger, large\",2.500000,BURGER-L,25.0000,usd\n\
              2026-07-21,Fries,3,,,\n",
        )
        .unwrap();

        assert_eq!(parsed.business_date.to_string(), "2026-07-21");
        assert_eq!(parsed.rows.len(), 2);
        assert_eq!(parsed.rows[0].row_number, 2);
        assert_eq!(parsed.rows[0].raw_item_label, "Burger, large");
        assert_eq!(parsed.rows[0].quantity, "2.500000");
        assert_eq!(
            parsed.rows[0].reported_net_sales.as_deref(),
            Some("25.0000")
        );
        assert_eq!(parsed.rows[0].currency.as_deref(), Some("USD"));
        assert_eq!(parsed.rows[1].reported_net_sales, None);
    }

    #[test]
    fn csv_rejects_unknown_duplicate_and_missing_headers() {
        let unknown =
            parse_sales_csv(b"business_date,item_name,quantity,total\n2026-07-21,Taco,1,2\n")
                .unwrap_err();
        assert!(
            unknown
                .iter()
                .any(|issue| issue.message.contains("Unknown header 'total'"))
        );

        let duplicate =
            parse_sales_csv(b"business_date,item_name,quantity,quantity\n2026-07-21,Taco,1,1\n")
                .unwrap_err();
        assert!(
            duplicate
                .iter()
                .any(|issue| issue.message.contains("appears more than once"))
        );

        let missing = parse_sales_csv(b"business_date,item_name\n2026-07-21,Taco\n").unwrap_err();
        assert!(missing.iter().any(|issue| {
            issue
                .message
                .contains("Required header 'quantity' is missing")
        }));
    }

    #[test]
    fn csv_reports_row_numbered_date_decimal_and_currency_errors() {
        let issues = parse_sales_csv(
            b"business_date,item_name,quantity,net_sales,currency\n\
              2026-07-21,Taco,1,12.00,\n\
              2026-07-22,Fries,1.0000001,-1,USD\n",
        )
        .unwrap_err();

        assert!(issues.iter().any(|issue| {
            issue.row_number == Some(2) && issue.message.contains("Currency is required")
        }));
        assert!(issues.iter().any(|issue| {
            issue.row_number == Some(3) && issue.message.contains("same business date")
        }));
        assert!(issues.iter().any(|issue| {
            issue.row_number == Some(3) && issue.message.contains("Quantity must be")
        }));
        assert!(issues.iter().any(|issue| {
            issue.row_number == Some(3) && issue.message.contains("Net sales must be")
        }));
    }

    #[test]
    fn csv_enforces_the_data_row_limit() {
        let mut csv = String::from("business_date,item_name,quantity\n");
        for _ in 0..=MAX_CSV_ROWS {
            csv.push_str("2026-07-21,Taco,1\n");
        }
        let issues = parse_sales_csv(csv.as_bytes()).unwrap_err();
        assert!(
            issues
                .iter()
                .any(|issue| issue.message.contains("no more than 2000"))
        );
    }

    #[test]
    fn csv_rejects_malformed_rows_and_uploads_over_one_mibibyte() {
        let issues =
            parse_sales_csv(b"business_date,item_name,quantity\n2026-07-21,Taco,1,unexpected\n")
                .unwrap_err();
        assert!(issues.iter().any(|issue| {
            issue.row_number == Some(2) && issue.message.contains("not valid comma-delimited CSV")
        }));

        assert!(validate_csv_upload("sales.csv", b"business_date").is_ok());
        let oversized = vec![b'x'; MAX_CSV_BYTES + 1];
        let error = validate_csv_upload("sales.csv", &oversized).unwrap_err();
        assert_eq!(error.status, StatusCode::PAYLOAD_TOO_LARGE);
    }

    #[test]
    fn exact_matching_only_trims_and_folds_case() {
        assert_eq!(normalized_menu_name("  Chicken Taco "), "chicken taco");
        assert_eq!(
            normalized_menu_name("Chicken  Taco"),
            "chicken  taco",
            "internal whitespace must not be fuzzily normalized"
        );
        assert_ne!(
            normalized_menu_name("Chicken-Taco"),
            normalized_menu_name("Chicken Taco")
        );
    }

    #[test]
    fn lowercasing_collision_is_left_unmatched_instead_of_chosen_arbitrarily() {
        let item = |id, name: &str| MenuRecord {
            id: Uuid::from_u128(id),
            name: name.into(),
            currency: "USD".into(),
            active: true,
        };
        let index = menu_name_index(vec![
            item(1, "İ"),
            item(2, "i\u{307}"),
            item(3, "Chicken Taco"),
        ]);

        assert!(!index.contains_key(&normalized_menu_name("İ")));
        assert_eq!(
            index[&normalized_menu_name("chicken taco")].id,
            Uuid::from_u128(3)
        );
    }

    #[test]
    fn imported_currency_is_checked_for_apply_and_replay() {
        let id = Uuid::from_u128(1);
        assert!(validate_reported_currency(Some("USD"), true, "USD").is_ok());
        assert!(validate_reported_currency(Some("EUR"), true, "USD").is_err());
        assert!(validate_reported_currency(None, true, "USD").is_ok());

        let stored = vec![StoredLine {
            menu_item_id: id,
            menu_item_name: "Burger".into(),
            quantity: "1".parse().unwrap(),
            reported_net_sales: Some("12.50".parse().unwrap()),
            currency: Some("USD".into()),
        }];
        let same = input(
            serde_json::json!(1),
            serde_json::json!([{
                "menuItemId": id,
                "quantity": "1",
                "reportedNetSales": "12.5000",
                "currency": "usd"
            }]),
        )
        .validated()
        .unwrap();
        assert!(replay_matches(&same.lines, &stored));

        let different_currency = input(
            serde_json::json!(1),
            serde_json::json!([{
                "menuItemId": id,
                "quantity": "1",
                "reportedNetSales": "12.50",
                "currency": "EUR"
            }]),
        )
        .validated()
        .unwrap();
        assert!(!replay_matches(&different_currency.lines, &stored));
    }
}
