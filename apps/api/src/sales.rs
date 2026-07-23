use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::PgConnection;
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error, invoices::strict_decimal};

const MAX_SALES_LINES: usize = 200;

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

pub(crate) async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(business_date): Path<String>,
    Json(input): Json<SaveSalesDay>,
) -> Result<Json<SalesDay>, ApiError> {
    let member = membership(&state, &headers).await?;
    require_manager(&member.role)?;
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
            "Owner or manager access is required to save sales.",
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
            lines.push(ValidatedLine {
                menu_item_id: line.menu_item_id,
                quantity,
                reported_net_sales,
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
    fn only_owner_and_manager_can_write() {
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
}
