use std::collections::HashMap;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Datelike, Days, Duration, LocalResult, NaiveDate, TimeZone, Utc, Weekday};
use chrono_tz::Tz;
use serde::Serialize;
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject};

const WEEK_DAYS: u32 = 7;
const MAX_GROUPS: usize = 5;

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    role: String,
    timezone: String,
}

#[derive(sqlx::FromRow)]
struct SalesAmountRow {
    reported_net_sales: Option<BigDecimal>,
    currency: Option<String>,
}

#[derive(sqlx::FromRow)]
struct PurchaseLineRow {
    currency: String,
    line_total: Option<BigDecimal>,
}

#[derive(Clone, sqlx::FromRow)]
struct LossRow {
    inventory_item_id: Uuid,
    event_type: String,
    inventory_item_name: String,
    count_unit: String,
    quantity: Option<BigDecimal>,
    reason: String,
    created_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct WeeklyBriefResponse {
    timezone: String,
    week_start: NaiveDate,
    week_end: NaiveDate,
    utc_start: DateTime<Utc>,
    utc_end: DateTime<Utc>,
    generated_at: DateTime<Utc>,
    is_live_preview: bool,
    days_elapsed: u32,
    caveats: Vec<&'static str>,
    sales: SalesSection,
    purchases: PurchaseSection,
    losses: LossSection,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SalesSection {
    days_with_data: i64,
    days_elapsed: u32,
    days_in_week: u32,
    reported_line_count: usize,
    lines_without_reported_sales: usize,
    entered_sales_by_currency: Vec<CurrencyAmount>,
    top_items_by_entered_quantity: Vec<EnteredQuantity>,
    caveats: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PurchaseSection {
    receipt_count: i64,
    usable_positive_line_total_count: usize,
    lines_missing_or_nonpositive_total_count: usize,
    recorded_invoice_line_purchases_by_currency: Vec<CurrencyAmount>,
    caveats: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LossSection {
    waste_count: usize,
    stockout_count: usize,
    waste_affected_items: Vec<CountGroup>,
    stockout_affected_items: Vec<CountGroup>,
    waste_reasons: Vec<ReasonCount>,
    stockout_reasons: Vec<ReasonCount>,
    recent_waste_quantities: Vec<WasteQuantity>,
    caveats: Vec<&'static str>,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CurrencyAmount {
    currency: String,
    amount: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct EnteredQuantity {
    menu_item_id: Uuid,
    menu_item_name: String,
    entered_quantity: String,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct CountGroup {
    inventory_item_id: Uuid,
    inventory_item_name: String,
    event_count: usize,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct ReasonCount {
    reason: String,
    event_count: usize,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
struct WasteQuantity {
    inventory_item_id: Uuid,
    inventory_item_name: String,
    count_unit: String,
    entered_quantity: String,
    event_count: usize,
    last_logged_at: DateTime<Utc>,
}

struct WeekWindow {
    local_start: NaiveDate,
    local_end: NaiveDate,
    utc_start: DateTime<Utc>,
    utc_end: DateTime<Utc>,
    days_elapsed: u32,
}

struct SalesAmounts {
    reported_line_count: usize,
    lines_without_reported_sales: usize,
    by_currency: Vec<CurrencyAmount>,
}

struct PurchaseAmounts {
    usable_positive_line_total_count: usize,
    lines_missing_or_nonpositive_total_count: usize,
    by_currency: Vec<CurrencyAmount>,
}

struct ItemAccumulator {
    inventory_item_id: Uuid,
    inventory_item_name: String,
    event_count: usize,
    last_logged_at: DateTime<Utc>,
}

struct WasteAccumulator {
    inventory_item_id: Uuid,
    inventory_item_name: String,
    count_unit: String,
    entered_quantity: BigDecimal,
    event_count: usize,
    last_logged_at: DateTime<Utc>,
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<WeeklyBriefResponse>, ApiError> {
    let subject = authenticated_subject(&state, &headers).await?;
    let member = sqlx::query_as::<_, Membership>(
        "SELECT membership.restaurant_id,membership.role,restaurant.timezone
         FROM users user_account
         JOIN restaurant_memberships membership ON membership.user_id=user_account.id
         JOIN restaurants restaurant ON restaurant.id=membership.restaurant_id
         WHERE user_account.auth_subject=$1",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(weekly_brief_database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "A restaurant membership is required.",
    ))?;

    // The brief combines sales and purchase financial facts. Reject other roles before any of
    // those queries execute, rather than relying on the web navigation to hide the workspace.
    require_owner(&member.role)?;

    let generated_at = Utc::now();
    let timezone = member.timezone.parse::<Tz>().unwrap_or_else(|_| {
        tracing::warn!(
            restaurant_id = %member.restaurant_id,
            timezone = %member.timezone,
            "invalid restaurant timezone; weekly brief is falling back to UTC"
        );
        chrono_tz::UTC
    });
    let window = current_week(generated_at, timezone).ok_or_else(week_boundary_error)?;
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(weekly_brief_database_error)?;
    sqlx::query("SET TRANSACTION ISOLATION LEVEL REPEATABLE READ READ ONLY")
        .execute(&mut *tx)
        .await
        .map_err(weekly_brief_database_error)?;

    let days_with_data = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM sales_days
         WHERE restaurant_id=$1 AND business_date >= $2 AND business_date < $3",
    )
    .bind(member.restaurant_id)
    .bind(window.local_start)
    .bind(window.local_end)
    .fetch_one(&mut *tx)
    .await
    .map_err(weekly_brief_database_error)?;
    let sales_rows = sqlx::query_as::<_, SalesAmountRow>(
        "SELECT line.reported_net_sales,line.currency
         FROM sales_days day
         JOIN sales_lines line
           ON line.sales_day_id=day.id AND line.restaurant_id=day.restaurant_id
         WHERE day.restaurant_id=$1 AND day.business_date >= $2 AND day.business_date < $3",
    )
    .bind(member.restaurant_id)
    .bind(window.local_start)
    .bind(window.local_end)
    .fetch_all(&mut *tx)
    .await
    .map_err(weekly_brief_database_error)?;
    let top_items_by_entered_quantity = sqlx::query_as::<_, EnteredQuantity>(
        "WITH weekly_lines AS (
             SELECT line.menu_item_id,line.menu_item_name,line.quantity,day.business_date,day.id
             FROM sales_days day
             JOIN sales_lines line
               ON line.sales_day_id=day.id AND line.restaurant_id=day.restaurant_id
             WHERE day.restaurant_id=$1 AND day.business_date >= $2 AND day.business_date < $3
         ), quantities AS (
             SELECT menu_item_id,SUM(quantity) AS entered_quantity
             FROM weekly_lines GROUP BY menu_item_id
         ), latest_names AS (
             SELECT DISTINCT ON (menu_item_id) menu_item_id,menu_item_name
             FROM weekly_lines
             ORDER BY menu_item_id,business_date DESC,id DESC
         )
         SELECT quantity.menu_item_id,name.menu_item_name,quantity.entered_quantity::text
         FROM quantities quantity
         JOIN latest_names name ON name.menu_item_id=quantity.menu_item_id
         ORDER BY quantity.entered_quantity DESC,LOWER(name.menu_item_name),quantity.menu_item_id
         LIMIT 5",
    )
    .bind(member.restaurant_id)
    .bind(window.local_start)
    .bind(window.local_end)
    .fetch_all(&mut *tx)
    .await
    .map_err(weekly_brief_database_error)?;
    let sales_amounts = aggregate_sales(sales_rows);

    let receipt_count = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM purchase_receipts
         WHERE restaurant_id=$1 AND invoice_date >= $2 AND invoice_date < $3",
    )
    .bind(member.restaurant_id)
    .bind(window.local_start)
    .bind(window.local_end)
    .fetch_one(&mut *tx)
    .await
    .map_err(weekly_brief_database_error)?;
    let purchase_rows = sqlx::query_as::<_, PurchaseLineRow>(
        "SELECT receipt.currency,line.line_total
         FROM purchase_receipts receipt
         JOIN purchase_receipt_lines line
           ON line.invoice_id=receipt.invoice_id AND line.restaurant_id=receipt.restaurant_id
         WHERE receipt.restaurant_id=$1
           AND receipt.invoice_date >= $2 AND receipt.invoice_date < $3",
    )
    .bind(member.restaurant_id)
    .bind(window.local_start)
    .bind(window.local_end)
    .fetch_all(&mut *tx)
    .await
    .map_err(weekly_brief_database_error)?;
    let purchase_amounts = aggregate_purchases(purchase_rows);

    let loss_rows = sqlx::query_as::<_, LossRow>(
        "SELECT inventory_item_id,event_type,inventory_item_name,count_unit,
                quantity,reason,created_at
         FROM loss_events
         WHERE restaurant_id=$1 AND created_at >= $2 AND created_at < $3",
    )
    .bind(member.restaurant_id)
    .bind(window.utc_start)
    .bind(window.utc_end)
    .fetch_all(&mut *tx)
    .await
    .map_err(weekly_brief_database_error)?;
    let losses = aggregate_losses(loss_rows);
    tx.commit().await.map_err(weekly_brief_database_error)?;

    Ok(Json(WeeklyBriefResponse {
        timezone: timezone.name().to_owned(),
        week_start: window.local_start,
        week_end: window.local_end,
        utc_start: window.utc_start,
        utc_end: window.utc_end,
        generated_at,
        is_live_preview: true,
        days_elapsed: window.days_elapsed,
        caveats: vec![
            "This is a live preview of entered current-week records, not a saved snapshot. Values can change as records are added or corrected.",
        ],
        sales: SalesSection {
            days_with_data,
            days_elapsed: window.days_elapsed,
            days_in_week: WEEK_DAYS,
            reported_line_count: sales_amounts.reported_line_count,
            lines_without_reported_sales: sales_amounts.lines_without_reported_sales,
            entered_sales_by_currency: sales_amounts.by_currency,
            top_items_by_entered_quantity,
            caveats: vec![
                "Entered sales are not total or POS sales. Reported net sales are included only when directly entered on a sales line.",
                "Entered quantity is the sum of per-menu-item servings recorded on sales lines.",
            ],
        },
        purchases: PurchaseSection {
            receipt_count,
            usable_positive_line_total_count: purchase_amounts.usable_positive_line_total_count,
            lines_missing_or_nonpositive_total_count: purchase_amounts
                .lines_missing_or_nonpositive_total_count,
            recorded_invoice_line_purchases_by_currency: purchase_amounts.by_currency,
            caveats: vec![
                "Recorded invoice-line purchases are not COGS or spend coverage. Only positive saved line totals are included; quantities and unit prices are not used to infer totals.",
                "Separately uploaded duplicate invoices are not detected and can affect totals.",
            ],
        },
        losses: LossSection {
            caveats: vec![
                "Waste and stockout values are direct log counts only. They do not show causality, trends, current stock, or financial impact.",
                "Waste quantities stay separated by exact inventory item and saved count unit. Stockouts never include quantity.",
            ],
            ..losses
        },
    }))
}

fn require_owner(role: &str) -> Result<(), ApiError> {
    if role == "owner" {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Owner access is required to view the weekly brief.",
        ))
    }
}

fn current_week(generated_at: DateTime<Utc>, timezone: Tz) -> Option<WeekWindow> {
    let local_date = generated_at.with_timezone(&timezone).date_naive();
    let days_since_monday = i64::from(local_date.weekday().num_days_from_monday());
    let local_start = local_date.checked_sub_signed(Duration::days(days_since_monday))?;
    let local_end = local_start.checked_add_days(Days::new(u64::from(WEEK_DAYS)))?;
    Some(WeekWindow {
        local_start,
        local_end,
        utc_start: local_date_start_utc(local_start, timezone)?,
        utc_end: local_date_start_utc(local_end, timezone)?,
        days_elapsed: elapsed_days(local_date.weekday()),
    })
}

fn local_date_start_utc(date: NaiveDate, timezone: Tz) -> Option<DateTime<Utc>> {
    let midnight = date.and_hms_opt(0, 0, 0)?;
    // Most zones have a single local midnight. For a midnight fold, the earliest instant starts
    // the local date. For a political/DST gap, the first representable instant on that local date
    // is the safe inclusive boundary.
    for minute in 0..=24 * 60 {
        let local = midnight.checked_add_signed(Duration::minutes(minute))?;
        match timezone.from_local_datetime(&local) {
            LocalResult::Single(value) => return Some(value.with_timezone(&Utc)),
            LocalResult::Ambiguous(first, second) => {
                return Some(first.with_timezone(&Utc).min(second.with_timezone(&Utc)));
            }
            LocalResult::None => {}
        }
    }
    None
}

fn elapsed_days(weekday: Weekday) -> u32 {
    weekday.num_days_from_monday() + 1
}

fn aggregate_sales(rows: Vec<SalesAmountRow>) -> SalesAmounts {
    let mut reported_line_count = 0;
    let mut lines_without_reported_sales = 0;
    let mut by_currency = HashMap::<String, BigDecimal>::new();
    for row in rows {
        match (row.reported_net_sales, row.currency) {
            (Some(amount), Some(currency)) => {
                reported_line_count += 1;
                *by_currency.entry(currency).or_default() += amount;
            }
            (None, None) => lines_without_reported_sales += 1,
            // The sales_lines constraint prevents mismatched amount/currency pairs.
            _ => unreachable!("sales amount and currency must be present together"),
        }
    }
    SalesAmounts {
        reported_line_count,
        lines_without_reported_sales,
        by_currency: currency_amounts(by_currency),
    }
}

fn aggregate_purchases(rows: Vec<PurchaseLineRow>) -> PurchaseAmounts {
    let mut usable_positive_line_total_count = 0;
    let mut lines_missing_or_nonpositive_total_count = 0;
    let mut by_currency = HashMap::<String, BigDecimal>::new();
    for row in rows {
        match row.line_total {
            Some(line_total) if line_total > 0 => {
                usable_positive_line_total_count += 1;
                *by_currency.entry(row.currency).or_default() += line_total;
            }
            _ => lines_missing_or_nonpositive_total_count += 1,
        }
    }
    PurchaseAmounts {
        usable_positive_line_total_count,
        lines_missing_or_nonpositive_total_count,
        by_currency: currency_amounts(by_currency),
    }
}

fn currency_amounts(values: HashMap<String, BigDecimal>) -> Vec<CurrencyAmount> {
    let mut values = values
        .into_iter()
        .map(|(currency, amount)| CurrencyAmount {
            currency,
            amount: decimal_string(amount),
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| left.currency.cmp(&right.currency));
    values
}

fn aggregate_losses(rows: Vec<LossRow>) -> LossSection {
    let mut waste_count = 0;
    let mut stockout_count = 0;
    let mut waste_items = HashMap::<Uuid, ItemAccumulator>::new();
    let mut stockout_items = HashMap::<Uuid, ItemAccumulator>::new();
    let mut waste_reasons = HashMap::<String, usize>::new();
    let mut stockout_reasons = HashMap::<String, usize>::new();
    let mut waste_quantities = HashMap::<(Uuid, String), WasteAccumulator>::new();

    for row in rows {
        match row.event_type.as_str() {
            "waste" => {
                waste_count += 1;
                increment_item(&mut waste_items, &row);
                *waste_reasons.entry(row.reason).or_default() += 1;
                let quantity = row
                    .quantity
                    .expect("the loss_events constraint requires waste quantity");
                let quantity_key = (row.inventory_item_id, row.count_unit.clone());
                let accumulator =
                    waste_quantities
                        .entry(quantity_key)
                        .or_insert_with(|| WasteAccumulator {
                            inventory_item_id: row.inventory_item_id,
                            inventory_item_name: row.inventory_item_name.clone(),
                            count_unit: row.count_unit.clone(),
                            entered_quantity: BigDecimal::from(0),
                            event_count: 0,
                            last_logged_at: row.created_at,
                        });
                accumulator.entered_quantity += quantity;
                accumulator.event_count += 1;
                if row.created_at > accumulator.last_logged_at
                    || (row.created_at == accumulator.last_logged_at
                        && row.inventory_item_name < accumulator.inventory_item_name)
                {
                    accumulator.inventory_item_name = row.inventory_item_name;
                    accumulator.last_logged_at = row.created_at;
                }
            }
            "stockout" => {
                stockout_count += 1;
                increment_item(&mut stockout_items, &row);
                *stockout_reasons.entry(row.reason).or_default() += 1;
            }
            _ => unreachable!("loss_events has a closed event type constraint"),
        }
    }

    let mut recent_waste_quantities = waste_quantities
        .into_values()
        .map(|value| WasteQuantity {
            inventory_item_id: value.inventory_item_id,
            inventory_item_name: value.inventory_item_name,
            count_unit: value.count_unit,
            entered_quantity: decimal_string(value.entered_quantity),
            event_count: value.event_count,
            last_logged_at: value.last_logged_at,
        })
        .collect::<Vec<_>>();
    recent_waste_quantities.sort_by(|left, right| {
        right
            .last_logged_at
            .cmp(&left.last_logged_at)
            .then_with(|| {
                left.inventory_item_name
                    .to_lowercase()
                    .cmp(&right.inventory_item_name.to_lowercase())
            })
            .then_with(|| left.count_unit.cmp(&right.count_unit))
            .then_with(|| left.inventory_item_id.cmp(&right.inventory_item_id))
    });
    recent_waste_quantities.truncate(MAX_GROUPS);

    LossSection {
        waste_count,
        stockout_count,
        waste_affected_items: count_groups(waste_items),
        stockout_affected_items: count_groups(stockout_items),
        waste_reasons: reason_groups(waste_reasons),
        stockout_reasons: reason_groups(stockout_reasons),
        recent_waste_quantities,
        caveats: Vec::new(),
    }
}

fn increment_item(values: &mut HashMap<Uuid, ItemAccumulator>, row: &LossRow) {
    let accumulator = values
        .entry(row.inventory_item_id)
        .or_insert_with(|| ItemAccumulator {
            inventory_item_id: row.inventory_item_id,
            inventory_item_name: row.inventory_item_name.clone(),
            event_count: 0,
            last_logged_at: row.created_at,
        });
    accumulator.event_count += 1;
    if row.created_at > accumulator.last_logged_at
        || (row.created_at == accumulator.last_logged_at
            && row.inventory_item_name < accumulator.inventory_item_name)
    {
        accumulator.inventory_item_name = row.inventory_item_name.clone();
        accumulator.last_logged_at = row.created_at;
    }
}

fn count_groups(values: HashMap<Uuid, ItemAccumulator>) -> Vec<CountGroup> {
    let mut values = values
        .into_values()
        .map(|value| CountGroup {
            inventory_item_id: value.inventory_item_id,
            inventory_item_name: value.inventory_item_name,
            event_count: value.event_count,
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .event_count
            .cmp(&left.event_count)
            .then_with(|| {
                left.inventory_item_name
                    .to_lowercase()
                    .cmp(&right.inventory_item_name.to_lowercase())
            })
            .then_with(|| left.inventory_item_id.cmp(&right.inventory_item_id))
    });
    values.truncate(MAX_GROUPS);
    values
}

fn reason_groups(values: HashMap<String, usize>) -> Vec<ReasonCount> {
    let mut values = values
        .into_iter()
        .map(|(reason, event_count)| ReasonCount {
            reason,
            event_count,
        })
        .collect::<Vec<_>>();
    values.sort_by(|left, right| {
        right
            .event_count
            .cmp(&left.event_count)
            .then_with(|| left.reason.cmp(&right.reason))
    });
    values.truncate(MAX_GROUPS);
    values
}

fn decimal_string(value: BigDecimal) -> String {
    value.normalized().to_string()
}

fn week_boundary_error() -> ApiError {
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "We couldn't determine this restaurant's current local week. Please try again.",
    )
}

fn weekly_brief_database_error(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "weekly brief query failed");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "We couldn't load the weekly brief. Please try again.",
    )
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 3, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn loss(
        id: u128,
        event_type: &str,
        name: &str,
        unit: &str,
        quantity: Option<&str>,
        reason: &str,
        created_at: DateTime<Utc>,
    ) -> LossRow {
        LossRow {
            inventory_item_id: Uuid::from_u128(id),
            event_type: event_type.into(),
            inventory_item_name: name.into(),
            count_unit: unit.into(),
            quantity: quantity.map(|value| value.parse().unwrap()),
            reason: reason.into(),
            created_at,
        }
    }

    #[test]
    fn monday_boundaries_use_local_dates_and_elapsed_days_are_inclusive() {
        let generated = Utc
            .with_ymd_and_hms(2026, 7, 20, 15, 30, 0)
            .single()
            .unwrap();
        let window = current_week(generated, chrono_tz::America::Chicago).unwrap();
        assert_eq!(window.local_start.to_string(), "2026-07-20");
        assert_eq!(window.local_end.to_string(), "2026-07-27");
        assert_eq!(window.utc_start.to_rfc3339(), "2026-07-20T05:00:00+00:00");
        assert_eq!(window.utc_end.to_rfc3339(), "2026-07-27T05:00:00+00:00");
        assert_eq!(window.days_elapsed, 1);
        assert_eq!(elapsed_days(Weekday::Sun), 7);
    }

    #[test]
    fn dst_transition_week_uses_distinct_utc_offsets() {
        let generated = Utc.with_ymd_and_hms(2026, 3, 8, 16, 0, 0).single().unwrap();
        let window = current_week(generated, chrono_tz::America::New_York).unwrap();
        assert_eq!(window.local_start.to_string(), "2026-03-02");
        assert_eq!(window.local_end.to_string(), "2026-03-09");
        assert_eq!(window.utc_start.to_rfc3339(), "2026-03-02T05:00:00+00:00");
        assert_eq!(window.utc_end.to_rfc3339(), "2026-03-09T04:00:00+00:00");
        assert_eq!(window.utc_end - window.utc_start, Duration::hours(167));
        assert_eq!(window.days_elapsed, 7);
    }

    #[test]
    fn only_owner_can_pass_the_brief_guard() {
        assert!(require_owner("owner").is_ok());
        for role in ["manager", "staff"] {
            let error = require_owner(role).unwrap_err();
            assert_eq!(error.0, StatusCode::FORBIDDEN);
        }
    }

    #[test]
    fn sales_currencies_stay_separate_and_missing_sales_are_not_inferred() {
        let amounts = aggregate_sales(vec![
            SalesAmountRow {
                reported_net_sales: Some("10.00".parse().unwrap()),
                currency: Some("USD".into()),
            },
            SalesAmountRow {
                reported_net_sales: Some("2.50".parse().unwrap()),
                currency: Some("USD".into()),
            },
            SalesAmountRow {
                reported_net_sales: Some("8.00".parse().unwrap()),
                currency: Some("CAD".into()),
            },
            SalesAmountRow {
                reported_net_sales: None,
                currency: None,
            },
        ]);
        assert_eq!(amounts.reported_line_count, 3);
        assert_eq!(amounts.lines_without_reported_sales, 1);
        assert_eq!(
            amounts.by_currency,
            vec![
                CurrencyAmount {
                    currency: "CAD".into(),
                    amount: "8".into()
                },
                CurrencyAmount {
                    currency: "USD".into(),
                    amount: "12.5".into()
                }
            ]
        );
    }

    #[test]
    fn purchase_totals_use_only_positive_saved_line_totals() {
        let amounts = aggregate_purchases(vec![
            PurchaseLineRow {
                currency: "USD".into(),
                line_total: Some("12.25".parse().unwrap()),
            },
            PurchaseLineRow {
                currency: "CAD".into(),
                line_total: Some("4".parse().unwrap()),
            },
            PurchaseLineRow {
                currency: "USD".into(),
                line_total: Some("0".parse().unwrap()),
            },
            PurchaseLineRow {
                currency: "USD".into(),
                line_total: Some("-2".parse().unwrap()),
            },
            PurchaseLineRow {
                currency: "USD".into(),
                line_total: None,
            },
        ]);
        assert_eq!(amounts.usable_positive_line_total_count, 2);
        assert_eq!(amounts.lines_missing_or_nonpositive_total_count, 3);
        assert_eq!(amounts.by_currency.len(), 2);
        assert_eq!(amounts.by_currency[0].currency, "CAD");
        assert_eq!(amounts.by_currency[1].currency, "USD");
    }

    #[test]
    fn losses_keep_types_and_waste_item_units_separate() {
        let summary = aggregate_losses(vec![
            loss(
                1,
                "waste",
                "Chicken",
                "lb",
                Some("1.5"),
                "spoilage",
                at(2, 10),
            ),
            loss(
                1,
                "waste",
                "Chicken breast",
                "lb",
                Some("2"),
                "spoilage",
                at(3, 10),
            ),
            loss(1, "waste", "Chicken", "case", Some("1"), "other", at(4, 10)),
            loss(
                1,
                "stockout",
                "Chicken",
                "lb",
                None,
                "delivery_late_or_missed",
                at(5, 10),
            ),
        ]);
        assert_eq!(summary.waste_count, 3);
        assert_eq!(summary.stockout_count, 1);
        assert_eq!(summary.recent_waste_quantities.len(), 2);
        let pounds = summary
            .recent_waste_quantities
            .iter()
            .find(|value| value.count_unit == "lb")
            .unwrap();
        assert_eq!(pounds.entered_quantity, "3.5");
        assert_eq!(pounds.event_count, 2);
        assert_eq!(pounds.inventory_item_name, "Chicken breast");
        assert_eq!(summary.waste_affected_items.len(), 1);
        assert_eq!(summary.waste_affected_items[0].event_count, 3);
        assert_eq!(summary.stockout_affected_items[0].event_count, 1);
    }
}
