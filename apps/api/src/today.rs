use std::collections::HashMap;

use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, NaiveDate, Utc};
use chrono_tz::Tz;
use serde::Serialize;
use uuid::Uuid;

use crate::{
    ApiError, AppState, authenticated_subject,
    invoices::{TodayPriceChange, restaurant_price_changes},
};

const MAX_ACTIONS: usize = 5;
const MAX_PER_CATEGORY: usize = 2;
const COUNT_CADENCE_DAYS: i64 = 7;

const OWNER_WORKFLOW_SQL: &str = "SELECT
    (SELECT COUNT(*) FROM invoices WHERE restaurant_id=$1 AND status='needs_review') AS invoice_review_count,
    (SELECT MAX(created_at) FROM invoices WHERE restaurant_id=$1 AND status='needs_review') AS invoice_review_at,
    (SELECT COUNT(*) FROM menu_imports WHERE restaurant_id=$1 AND status='needs_review') AS menu_review_count,
    (SELECT MAX(created_at) FROM menu_imports WHERE restaurant_id=$1 AND status='needs_review') AS menu_review_at,
    (SELECT COUNT(*) FROM invoices WHERE restaurant_id=$1 AND status='failed') AS failed_invoice_count,
    (SELECT MAX(updated_at) FROM invoices WHERE restaurant_id=$1 AND status='failed') AS failed_invoice_at";

const BELOW_PAR_SQL: &str = "WITH latest_counts AS (
    SELECT item.id,item.name,item.count_unit,item.par_level,entry.quantity,session.completed_at,
           ROW_NUMBER() OVER (
               PARTITION BY item.id ORDER BY session.completed_at DESC,session.id DESC
           ) AS recency
    FROM inventory_items item
    JOIN inventory_count_entries entry ON entry.inventory_item_id=item.id
    JOIN inventory_count_sessions session ON session.id=entry.session_id
    WHERE item.restaurant_id=$1 AND item.active AND item.par_level IS NOT NULL
      AND session.restaurant_id=$1 AND session.status='completed' AND entry.quantity IS NOT NULL
)
SELECT id AS item_id,name,count_unit,quantity::text AS quantity,par_level::text AS par_level,completed_at
FROM latest_counts
WHERE recency=1 AND quantity<par_level
ORDER BY completed_at DESC,LOWER(name),count_unit";

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    role: String,
    timezone: String,
}

#[derive(sqlx::FromRow, Default)]
struct WorkflowFacts {
    invoice_review_count: i64,
    invoice_review_at: Option<DateTime<Utc>>,
    menu_review_count: i64,
    menu_review_at: Option<DateTime<Utc>>,
    failed_invoice_count: i64,
    failed_invoice_at: Option<DateTime<Utc>>,
}

#[derive(sqlx::FromRow)]
struct DraftFact {
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct BelowParFact {
    item_id: Uuid,
    name: String,
    count_unit: String,
    quantity: String,
    par_level: String,
    completed_at: DateTime<Utc>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct TodayResponse {
    timezone: String,
    restaurant_local_date: NaiveDate,
    generated_at: DateTime<Utc>,
    actions: Vec<Action>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct Action {
    action_id: String,
    rule_key: &'static str,
    category: &'static str,
    priority: Priority,
    confidence: Confidence,
    title: String,
    why_it_matters: String,
    next_action: String,
    evidence: Evidence,
    limitation: String,
    target: Target,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Priority {
    Urgent,
    High,
    Normal,
}

#[derive(Clone, Serialize)]
struct Confidence {
    level: ConfidenceLevel,
    reason: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum ConfidenceLevel {
    High,
    Medium,
}

#[derive(Clone, Serialize)]
struct Evidence {
    timestamp: DateTime<Utc>,
    value: String,
    source: String,
}

#[derive(Clone, Serialize)]
struct Target {
    workspace: &'static str,
    path: &'static str,
    label: &'static str,
}

struct Sources {
    workflows: Option<WorkflowFacts>,
    prices: Vec<TodayPriceChange>,
    draft: Option<DraftFact>,
    last_completed_count: Option<DateTime<Utc>>,
    below_par: Vec<BelowParFact>,
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<TodayResponse>, ApiError> {
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
    .map_err(today_database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "A restaurant membership is required.",
    ))?;

    let draft = sqlx::query_as::<_, DraftFact>(
        "SELECT updated_at FROM inventory_count_sessions
         WHERE restaurant_id=$1 AND status='draft'",
    )
    .bind(member.restaurant_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(today_database_error)?;
    let last_completed_count = sqlx::query_scalar::<_, Option<DateTime<Utc>>>(
        "SELECT MAX(completed_at) FROM inventory_count_sessions
         WHERE restaurant_id=$1 AND status='completed'",
    )
    .bind(member.restaurant_id)
    .fetch_one(&state.pool)
    .await
    .map_err(today_database_error)?;
    let below_par = sqlx::query_as::<_, BelowParFact>(BELOW_PAR_SQL)
        .bind(member.restaurant_id)
        .fetch_all(&state.pool)
        .await
        .map_err(today_database_error)?;

    let workflows = if matches!(member.role.as_str(), "owner" | "manager") {
        Some(
            sqlx::query_as::<_, WorkflowFacts>(OWNER_WORKFLOW_SQL)
                .bind(member.restaurant_id)
                .fetch_one(&state.pool)
                .await
                .map_err(today_database_error)?,
        )
    } else {
        None
    };
    // Supplier pricing remains owner-only financial evidence.
    let prices = if member.role == "owner" {
        restaurant_price_changes(&state.pool, member.restaurant_id)
            .await
            .map_err(today_database_error)?
    } else {
        Vec::new()
    };

    let generated_at = Utc::now();
    let timezone = member.timezone.parse::<Tz>().unwrap_or_else(|_| {
        tracing::warn!(
            restaurant_id = %member.restaurant_id,
            timezone = %member.timezone,
            "invalid restaurant timezone; Today is falling back to UTC"
        );
        chrono_tz::UTC
    });
    let restaurant_local_date = generated_at.with_timezone(&timezone).date_naive();
    let actions = build_actions(
        &member.role,
        timezone,
        restaurant_local_date,
        Sources {
            workflows,
            prices,
            draft,
            last_completed_count,
            below_par,
        },
    );

    Ok(Json(TodayResponse {
        timezone: timezone.name().to_owned(),
        restaurant_local_date,
        generated_at,
        actions,
    }))
}

fn today_database_error(error: sqlx::Error) -> ApiError {
    tracing::error!(%error, "Today query failed");
    ApiError(
        StatusCode::INTERNAL_SERVER_ERROR,
        "We couldn't load today's actions. Please try again.",
    )
}

fn build_actions(role: &str, timezone: Tz, local_date: NaiveDate, sources: Sources) -> Vec<Action> {
    let mut actions = Vec::new();

    if let Some(facts) = sources.workflows {
        push_grouped_action(
            &mut actions,
            facts.invoice_review_count,
            facts.invoice_review_at,
            GroupedAction {
                action_id: "today:invoice_review",
                rule_key: "invoice_review",
                category: "document_review",
                priority: Priority::Urgent,
                singular: "invoice needs review",
                plural: "invoices need review",
                title: "Review supplier invoices",
                why: "These invoices are still waiting for an owner or manager to verify the extracted details.",
                next: "Open Invoices and compare each record with its original document.",
                source: "invoices.status = needs_review",
                limitation: "This is a grouped status count; each original invoice still needs review.",
                target: Target {
                    workspace: "invoices",
                    path: "/invoices",
                    label: "Review invoices",
                },
            },
        );
        push_grouped_action(
            &mut actions,
            facts.menu_review_count,
            facts.menu_review_at,
            GroupedAction {
                action_id: "today:menu_review",
                rule_key: "menu_review",
                category: "document_review",
                priority: Priority::Urgent,
                singular: "menu import needs review",
                plural: "menu imports need review",
                title: "Review menu imports",
                why: "These menu files are still waiting for an owner or manager to check the extracted items and prices.",
                next: "Open Menu and verify each import against its original file.",
                source: "menu_imports.status = needs_review",
                limitation: "This is a grouped status count; nothing is imported until an owner or manager approves it.",
                target: Target {
                    workspace: "menu",
                    path: "/menu",
                    label: "Review menu",
                },
            },
        );
        push_grouped_action(
            &mut actions,
            facts.failed_invoice_count,
            facts.failed_invoice_at,
            GroupedAction {
                action_id: "today:import_retry",
                rule_key: "import_retry",
                category: "import_retry",
                priority: Priority::High,
                singular: "invoice import failed",
                plural: "invoice imports failed",
                title: "Retry failed invoice imports",
                why: "These invoice imports have a failed status and can be tried again.",
                next: "Open Invoices and retry each failed import.",
                source: "invoices.status = failed",
                limitation: "Only failed imports are included; delayed processing is not treated as retryable.",
                target: Target {
                    workspace: "invoices",
                    path: "/invoices",
                    label: "Open failed imports",
                },
            },
        );
    }
    if role == "owner" {
        for change in sources.prices {
            if let Some(action) = price_action(change) {
                actions.push(action);
            }
        }
    }

    if let Some(draft) = sources.draft {
        actions.push(Action {
            action_id: "today:inventory_count:resume".to_owned(),
            rule_key: "inventory_count",
            category: "inventory_count",
            priority: Priority::High,
            confidence: high_confidence("A draft inventory count exists for this restaurant."),
            title: "Resume inventory count".to_owned(),
            why_it_matters: "The crew can continue the saved count instead of starting over."
                .to_owned(),
            next_action: "Open Inventory and resume the draft count.".to_owned(),
            evidence: Evidence {
                timestamp: draft.updated_at,
                value: "Draft count is open".to_owned(),
                source: "inventory_count_sessions.status = draft".to_owned(),
            },
            limitation: "The draft may contain blank quantities until the crew completes it."
                .to_owned(),
            target: Target {
                workspace: "inventory",
                path: "/inventory",
                label: "Resume count",
            },
        });
    } else if sources
        .last_completed_count
        .is_some_and(|completed| count_is_due(completed, timezone, local_date))
    {
        let completed_at = sources
            .last_completed_count
            .expect("checked completed count above");
        actions.push(Action {
            action_id: "today:inventory_count:due".to_owned(),
            rule_key: "inventory_count",
            category: "inventory_count",
            priority: Priority::Normal,
            confidence: high_confidence(
                "The last completed restaurant count is at least 7 local calendar days old.",
            ),
            title: "Count tracked items".to_owned(),
            why_it_matters:
                "A new count will replace an inventory snapshot that is at least a week old."
                    .to_owned(),
            next_action: "Open Inventory and start a count of active tracked items.".to_owned(),
            evidence: Evidence {
                timestamp: completed_at,
                value: format!(
                    "Last completed count: {}",
                    completed_at.with_timezone(&timezone).date_naive()
                ),
                source: "latest completed inventory count".to_owned(),
            },
            limitation: "This cadence uses calendar age only; it does not infer current stock."
                .to_owned(),
            target: Target {
                workspace: "inventory",
                path: "/inventory",
                label: "Open inventory",
            },
        });
    }

    for item in sources.below_par {
        if let Some(action) = below_par_action(item, timezone, local_date) {
            actions.push(action);
        }
    }

    ordered_and_capped(actions)
}

struct GroupedAction {
    action_id: &'static str,
    rule_key: &'static str,
    category: &'static str,
    priority: Priority,
    singular: &'static str,
    plural: &'static str,
    title: &'static str,
    why: &'static str,
    next: &'static str,
    source: &'static str,
    limitation: &'static str,
    target: Target,
}

fn push_grouped_action(
    actions: &mut Vec<Action>,
    count: i64,
    timestamp: Option<DateTime<Utc>>,
    input: GroupedAction,
) {
    if count <= 0 {
        return;
    }
    let Some(timestamp) = timestamp else {
        return;
    };
    actions.push(Action {
        action_id: input.action_id.to_owned(),
        rule_key: input.rule_key,
        category: input.category,
        priority: input.priority,
        confidence: high_confidence("The action comes directly from current record statuses."),
        title: input.title.to_owned(),
        why_it_matters: input.why.to_owned(),
        next_action: input.next.to_owned(),
        evidence: Evidence {
            timestamp,
            value: format!(
                "{count} {}",
                if count == 1 {
                    input.singular
                } else {
                    input.plural
                }
            ),
            source: input.source.to_owned(),
        },
        limitation: input.limitation.to_owned(),
        target: input.target,
    });
}

fn price_action(change: TodayPriceChange) -> Option<Action> {
    if !change.increased {
        return None;
    }
    let percentage = trim_decimal(&change.percentage_change);
    let unit = change
        .unit
        .as_deref()
        .map(|unit| format!(" per {unit}"))
        .unwrap_or_default();
    let action_id = format!(
        "today:supplier_price_increase:{}:{}:{}:{}:{}:{}:{}:{}",
        change.invoice_id,
        canonical_component(&change.supplier_name),
        canonical_component(&change.comparison_key),
        canonical_component(&change.comparison_unit),
        change.currency.to_ascii_lowercase(),
        change.invoice_date,
        canonical_component(&change.previous_unit_price),
        canonical_component(&change.current_unit_price),
    );
    let confidence = if change.comparison_key.starts_with("sku:") {
        high_confidence(
            "Matched the same supplier, SKU, currency, and normalized unit on approved invoices.",
        )
    } else {
        medium_confidence(
            "Matched the same supplier, normalized description, currency, and unit; no SKU was available.",
        )
    };
    Some(Action {
        action_id,
        rule_key: "supplier_price_increase",
        category: "supplier_price",
        priority: if change.at_least_ten_percent {
            Priority::High
        } else {
            Priority::Normal
        },
        confidence,
        title: format!(
            "{} invoice price increased {percentage}%",
            change.description
        ),
        why_it_matters: format!(
            "{} changed from {} {}{} on {} to {} {}{} on {}.",
            change.supplier_name,
            change.currency,
            trim_decimal(&change.previous_unit_price),
            unit,
            change.previous_invoice_date,
            change.currency,
            trim_decimal(&change.current_unit_price),
            unit,
            change.invoice_date,
        ),
        next_action: "Review the invoice and confirm the supplier price.".to_owned(),
        evidence: Evidence {
            timestamp: change.created_at,
            value: format!(
                "{} {}{} · up {percentage}%",
                change.currency,
                trim_decimal(&change.current_unit_price),
                unit,
            ),
            source: "approved supplier invoice comparison".to_owned(),
        },
        limitation: "This compares invoice unit prices only; it does not show current inventory."
            .to_owned(),
        target: Target {
            workspace: "invoices",
            path: "/invoices",
            label: "Review invoices",
        },
    })
}

fn below_par_action(item: BelowParFact, timezone: Tz, local_date: NaiveDate) -> Option<Action> {
    if !is_recent(item.completed_at, timezone, local_date) {
        return None;
    }
    let local_timestamp = item.completed_at.with_timezone(&timezone);
    let timestamp_label = local_timestamp.format("%Y-%m-%d %H:%M %Z");
    Some(Action {
        action_id: format!("today:inventory_below_par:{}", item.item_id),
        rule_key: "inventory_below_par",
        category: "inventory_item",
        priority: Priority::High,
        confidence: high_confidence(
            "The latest non-blank completed count is recent and below the item's saved par.",
        ),
        title: format!("{} was below par at {timestamp_label}", item.name),
        why_it_matters: format!(
            "That completed count recorded {} {} against a par of {} {}.",
            trim_decimal(&item.quantity),
            item.count_unit,
            trim_decimal(&item.par_level),
            item.count_unit,
        ),
        next_action: "Check the item before deciding what to do next.".to_owned(),
        evidence: Evidence {
            timestamp: item.completed_at,
            value: format!(
                "{} {} counted · par {} {}",
                trim_decimal(&item.quantity),
                item.count_unit,
                trim_decimal(&item.par_level),
                item.count_unit,
            ),
            source: "latest non-blank completed inventory count".to_owned(),
        },
        limitation:
            "This was below par at the stated count time; it is not a claim about current stock."
                .to_owned(),
        target: Target {
            workspace: "inventory",
            path: "/inventory",
            label: "Check inventory",
        },
    })
}

fn count_is_due(completed_at: DateTime<Utc>, timezone: Tz, local_date: NaiveDate) -> bool {
    local_age_days(completed_at, timezone, local_date) >= COUNT_CADENCE_DAYS
}

fn is_recent(timestamp: DateTime<Utc>, timezone: Tz, local_date: NaiveDate) -> bool {
    let age = local_age_days(timestamp, timezone, local_date);
    (0..COUNT_CADENCE_DAYS).contains(&age)
}

fn local_age_days(timestamp: DateTime<Utc>, timezone: Tz, local_date: NaiveDate) -> i64 {
    local_date
        .signed_duration_since(timestamp.with_timezone(&timezone).date_naive())
        .num_days()
}

fn ordered_and_capped(mut actions: Vec<Action>) -> Vec<Action> {
    actions.sort_by(|left, right| {
        priority_rank(left.priority)
            .cmp(&priority_rank(right.priority))
            .then_with(|| right.evidence.timestamp.cmp(&left.evidence.timestamp))
            .then_with(|| left.action_id.cmp(&right.action_id))
    });
    let mut category_counts = HashMap::new();
    actions
        .into_iter()
        .filter(|action| {
            let count = category_counts.entry(action.category).or_insert(0usize);
            if *count >= MAX_PER_CATEGORY {
                false
            } else {
                *count += 1;
                true
            }
        })
        .take(MAX_ACTIONS)
        .collect()
}

fn priority_rank(priority: Priority) -> u8 {
    match priority {
        Priority::Urgent => 0,
        Priority::High => 1,
        Priority::Normal => 2,
    }
}

fn high_confidence(reason: &str) -> Confidence {
    Confidence {
        level: ConfidenceLevel::High,
        reason: reason.to_owned(),
    }
}

fn medium_confidence(reason: &str) -> Confidence {
    Confidence {
        level: ConfidenceLevel::Medium,
        reason: reason.to_owned(),
    }
}

fn trim_decimal(value: &str) -> String {
    if !value.contains('.') {
        return value.to_owned();
    }
    let trimmed = value.trim_end_matches('0').trim_end_matches('.');
    if trimmed.is_empty() || trimmed == "-" {
        "0".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn canonical_component(value: &str) -> String {
    let mut output = String::new();
    let mut separator = false;
    for character in value.trim().chars().flat_map(char::to_lowercase) {
        if character.is_alphanumeric() {
            if separator && !output.is_empty() {
                output.push('-');
            }
            output.push(character);
            separator = false;
        } else {
            separator = true;
        }
    }
    if output.is_empty() {
        "unknown".to_owned()
    } else {
        output
    }
}

#[cfg(test)]
mod tests {
    use chrono::TimeZone;

    use super::*;

    fn at(day: u32, hour: u32) -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 7, day, hour, 0, 0)
            .single()
            .unwrap()
    }

    fn workflow_facts() -> WorkflowFacts {
        WorkflowFacts {
            invoice_review_count: 2,
            invoice_review_at: Some(at(22, 10)),
            menu_review_count: 1,
            menu_review_at: Some(at(22, 9)),
            failed_invoice_count: 1,
            failed_invoice_at: Some(at(22, 8)),
        }
    }

    fn price(increased: bool, at_least_ten_percent: bool) -> TodayPriceChange {
        TodayPriceChange {
            invoice_id: Uuid::from_u128(100),
            supplier_name: "Acme Foods".into(),
            invoice_date: NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
            created_at: at(22, 7),
            description: "Chicken".into(),
            unit: Some("case".into()),
            currency: "USD".into(),
            previous_unit_price: "10.0000".into(),
            current_unit_price: if increased {
                "11.0000".into()
            } else {
                "9.0000".into()
            },
            percentage_change: if increased {
                "10.00".into()
            } else {
                "-10.00".into()
            },
            previous_invoice_date: NaiveDate::from_ymd_opt(2026, 7, 15).unwrap(),
            comparison_key: "sku:chk42".into(),
            comparison_unit: "case".into(),
            increased,
            at_least_ten_percent,
        }
    }

    fn sources(role_is_owner: bool) -> Sources {
        Sources {
            workflows: role_is_owner.then(workflow_facts),
            prices: if role_is_owner {
                vec![price(true, true)]
            } else {
                Vec::new()
            },
            draft: None,
            last_completed_count: None,
            below_par: Vec::new(),
        }
    }

    #[test]
    fn managers_receive_document_actions_but_not_owner_price_actions() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 23).unwrap();
        let owner = build_actions("owner", chrono_tz::UTC, date, sources(true));
        assert!(
            owner
                .iter()
                .any(|action| action.rule_key == "invoice_review")
        );
        assert!(
            owner
                .iter()
                .any(|action| action.rule_key == "supplier_price_increase")
        );

        let manager = build_actions("manager", chrono_tz::UTC, date, sources(true));
        assert!(
            manager
                .iter()
                .any(|action| action.rule_key == "invoice_review")
        );
        assert!(
            !manager
                .iter()
                .any(|action| action.rule_key == "supplier_price_increase")
        );
        let staff = build_actions("staff", chrono_tz::UTC, date, sources(false));
        assert!(staff.is_empty());
    }

    #[test]
    fn orders_then_caps_overall_and_by_category() {
        let mut values = vec![
            test_action("normal", "other", Priority::Normal, at(23, 12)),
            test_action("high-old", "inventory_item", Priority::High, at(21, 12)),
            test_action("urgent-old", "review", Priority::Urgent, at(20, 12)),
            test_action("high-new", "inventory_item", Priority::High, at(23, 12)),
            test_action("urgent-new", "review", Priority::Urgent, at(22, 12)),
            test_action("high-capped", "inventory_item", Priority::High, at(23, 13)),
            test_action("high-other", "other-high", Priority::High, at(23, 11)),
        ];
        values.reverse();
        let actions = ordered_and_capped(values);
        let ids: Vec<_> = actions
            .iter()
            .map(|action| action.action_id.as_str())
            .collect();
        assert_eq!(
            ids,
            [
                "urgent-new",
                "urgent-old",
                "high-capped",
                "high-new",
                "high-other"
            ]
        );
        assert_eq!(
            actions
                .iter()
                .filter(|action| action.category == "inventory_item")
                .count(),
            2
        );
    }

    #[test]
    fn below_par_requires_a_recent_local_calendar_count_and_uses_past_tense() {
        let local_date = NaiveDate::from_ymd_opt(2026, 7, 23).unwrap();
        let recent = BelowParFact {
            item_id: Uuid::from_u128(1),
            name: "Avocados".into(),
            count_unit: "case".into(),
            quantity: "2.000000".into(),
            par_level: "4.000000".into(),
            completed_at: at(17, 1),
        };
        let action = below_par_action(recent, chrono_tz::UTC, local_date).unwrap();
        assert!(action.title.contains("was below par at"));
        assert!(
            action
                .limitation
                .contains("not a claim about current stock")
        );

        let stale = BelowParFact {
            item_id: Uuid::from_u128(1),
            name: "Avocados".into(),
            count_unit: "case".into(),
            quantity: "2".into(),
            par_level: "4".into(),
            completed_at: at(16, 23),
        };
        assert!(below_par_action(stale, chrono_tz::UTC, local_date).is_none());
        assert!(count_is_due(at(16, 23), chrono_tz::UTC, local_date));
    }

    #[test]
    fn suppresses_price_decreases() {
        assert!(price_action(price(false, false)).is_none());
        let increase = price_action(price(true, false)).unwrap();
        assert_eq!(increase.priority, Priority::Normal);
    }

    #[test]
    fn action_ids_use_stable_database_ids_and_description_matches_are_medium_confidence() {
        let first_price = price_action(price(true, true)).unwrap();
        assert_eq!(first_price.confidence.level, ConfidenceLevel::High);
        let mut second_price = price(true, true);
        second_price.invoice_id = Uuid::from_u128(101);
        let second_price = price_action(second_price).unwrap();
        assert_ne!(first_price.action_id, second_price.action_id);
        let mut description_price = price(true, true);
        description_price.comparison_key = "description:chicken".into();
        assert_eq!(
            price_action(description_price).unwrap().confidence.level,
            ConfidenceLevel::Medium
        );

        let local_date = NaiveDate::from_ymd_opt(2026, 7, 23).unwrap();
        let first_item = below_par_action(
            BelowParFact {
                item_id: Uuid::from_u128(1),
                name: "A-B".into(),
                count_unit: "case".into(),
                quantity: "1".into(),
                par_level: "2".into(),
                completed_at: at(22, 1),
            },
            chrono_tz::UTC,
            local_date,
        )
        .unwrap();
        let second_item = below_par_action(
            BelowParFact {
                item_id: Uuid::from_u128(2),
                name: "A B".into(),
                count_unit: "case".into(),
                quantity: "1".into(),
                par_level: "2".into(),
                completed_at: at(22, 1),
            },
            chrono_tz::UTC,
            local_date,
        )
        .unwrap();
        assert_ne!(first_item.action_id, second_item.action_id);
    }

    #[test]
    fn retry_rule_only_queries_failed_invoices() {
        let query = OWNER_WORKFLOW_SQL.to_ascii_lowercase();
        assert!(query.contains("status='failed'"));
        assert!(!query.contains("processing"));
        assert!(!query.contains("delayed"));
    }

    #[test]
    fn ids_and_order_are_deterministic() {
        let date = NaiveDate::from_ymd_opt(2026, 7, 23).unwrap();
        let first = build_actions("owner", chrono_tz::UTC, date, sources(true));
        let second = build_actions("owner", chrono_tz::UTC, date, sources(true));
        let first_ids: Vec<_> = first.iter().map(|action| &action.action_id).collect();
        let second_ids: Vec<_> = second.iter().map(|action| &action.action_id).collect();
        assert_eq!(first_ids, second_ids);
        assert!(first_ids.iter().all(|id| !id.contains("019")));
    }

    fn test_action(
        id: &str,
        category: &'static str,
        priority: Priority,
        timestamp: DateTime<Utc>,
    ) -> Action {
        Action {
            action_id: id.to_owned(),
            rule_key: "test",
            category,
            priority,
            confidence: Confidence {
                level: ConfidenceLevel::Medium,
                reason: "test".into(),
            },
            title: id.to_owned(),
            why_it_matters: "test".into(),
            next_action: "test".into(),
            evidence: Evidence {
                timestamp,
                value: "test".into(),
                source: "test".into(),
            },
            limitation: "test".into(),
            target: Target {
                workspace: "test",
                path: "/test",
                label: "test",
            },
        }
    }
}
