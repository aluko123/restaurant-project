use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error, invoices::strict_decimal};

const WASTE_REASONS: &[&str] = &[
    "spoilage",
    "overproduction",
    "prep_mistake",
    "portioning",
    "dropped_damaged",
    "returned",
    "expired",
    "other",
];
const STOCKOUT_REASONS: &[&str] = &[
    "delivery_late_or_missed",
    "ordered_too_little",
    "demand_higher_than_expected",
    "prep_or_portion_issue",
    "waste_or_spoilage",
    "other",
];
const STOCKOUT_SEVERITIES: &[&str] = &["some_orders", "menu_item_unavailable", "service_blocker"];

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    user_id: Uuid,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct LossEventInput {
    event_type: String,
    inventory_item_id: Uuid,
    quantity: Option<String>,
    severity: Option<String>,
    reason: String,
    note: Option<String>,
}

struct ValidLossEvent {
    event_type: String,
    inventory_item_id: Uuid,
    quantity: Option<BigDecimal>,
    severity: Option<String>,
    reason: String,
    note: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct LossEvent {
    id: Uuid,
    inventory_item_id: Uuid,
    event_type: String,
    inventory_item_name: String,
    count_unit: String,
    quantity: Option<String>,
    severity: Option<String>,
    reason: String,
    note: Option<String>,
    created_at: DateTime<Utc>,
}

pub(crate) async fn list(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<LossEvent>>, ApiError> {
    let member = membership(&state, &headers).await?;
    let events = sqlx::query_as::<_, LossEvent>(
        "SELECT id,inventory_item_id,event_type,inventory_item_name,count_unit,
                quantity::text quantity,severity,reason,note,created_at
         FROM loss_events
         WHERE restaurant_id=$1
         ORDER BY created_at DESC,id DESC
         LIMIT 50",
    )
    .bind(member.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    Ok(Json(events))
}

pub(crate) async fn create(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(input): Json<LossEventInput>,
) -> Result<(StatusCode, Json<LossEvent>), ApiError> {
    let member = membership(&state, &headers).await?;
    let input = input.validated()?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let item = sqlx::query_as::<_, (String, String)>(
        "SELECT name,count_unit FROM inventory_items
         WHERE restaurant_id=$1 AND id=$2 AND active
         FOR SHARE",
    )
    .bind(member.restaurant_id)
    .bind(input.inventory_item_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Choose an active inventory item from this restaurant.",
    ))?;
    let event = sqlx::query_as::<_, LossEvent>(
        "INSERT INTO loss_events(
            id,restaurant_id,inventory_item_id,created_by,event_type,
            inventory_item_name,count_unit,quantity,severity,reason,note
         )
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11)
         RETURNING id,inventory_item_id,event_type,inventory_item_name,count_unit,
                   quantity::text quantity,severity,reason,note,created_at",
    )
    .bind(Uuid::now_v7())
    .bind(member.restaurant_id)
    .bind(input.inventory_item_id)
    .bind(member.user_id)
    .bind(input.event_type)
    .bind(item.0)
    .bind(item.1)
    .bind(input.quantity)
    .bind(input.severity)
    .bind(input.reason)
    .bind(input.note)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok((StatusCode::CREATED, Json(event)))
}

async fn membership(state: &AppState, headers: &HeaderMap) -> Result<Membership, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as::<_, Membership>(
        "SELECT m.restaurant_id,u.id AS user_id
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

impl LossEventInput {
    fn validated(self) -> Result<ValidLossEvent, ApiError> {
        let note = self.note.and_then(|note| {
            let note = note.trim();
            (!note.is_empty()).then(|| note.to_owned())
        });
        if note.as_ref().is_some_and(|note| note.chars().count() > 500) {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Note must be no more than 500 characters.",
            ));
        }

        let quantity = match self.event_type.as_str() {
            "waste" => {
                if self.severity.is_some() {
                    return Err(invalid_combination());
                }
                let value = self.quantity.as_deref().ok_or_else(invalid_combination)?;
                if !WASTE_REASONS.contains(&self.reason.as_str()) {
                    return Err(ApiError(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Choose a listed waste reason.",
                    ));
                }
                Some(positive_quantity(value)?)
            }
            "stockout" => {
                if self.quantity.is_some() {
                    return Err(invalid_combination());
                }
                if !self
                    .severity
                    .as_deref()
                    .is_some_and(|severity| STOCKOUT_SEVERITIES.contains(&severity))
                {
                    return Err(ApiError(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Choose a listed stockout severity.",
                    ));
                }
                if !STOCKOUT_REASONS.contains(&self.reason.as_str()) {
                    return Err(ApiError(
                        StatusCode::UNPROCESSABLE_ENTITY,
                        "Choose a listed stockout reason.",
                    ));
                }
                None
            }
            _ => return Err(invalid_combination()),
        };

        Ok(ValidLossEvent {
            event_type: self.event_type,
            inventory_item_id: self.inventory_item_id,
            quantity,
            severity: self.severity,
            reason: self.reason,
            note,
        })
    }
}

fn positive_quantity(value: &str) -> Result<BigDecimal, ApiError> {
    let unsigned = value.strip_prefix('-').unwrap_or(value);
    let (integer, fraction) = unsigned.split_once('.').unwrap_or((unsigned, ""));
    let exact_syntax = !value.starts_with('+')
        && !integer.is_empty()
        && integer.bytes().all(|byte| byte.is_ascii_digit())
        && fraction.bytes().all(|byte| byte.is_ascii_digit())
        && !(value.contains('.') && fraction.is_empty())
        && unsigned.matches('.').count() <= 1;
    let quantity = exact_syntax
        .then(|| strict_decimal(value, 6))
        .transpose()
        .map_err(|_| quantity_error())?
        .ok_or_else(quantity_error)?;
    if quantity <= 0 {
        return Err(quantity_error());
    }
    Ok(quantity)
}

fn invalid_combination() -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Waste needs a positive quantity and no severity; stockouts need a severity and no quantity.",
    )
}

fn quantity_error() -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Waste quantity must be a positive decimal string with at most 6 decimal places.",
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(event_type: &str) -> LossEventInput {
        LossEventInput {
            event_type: event_type.into(),
            inventory_item_id: Uuid::now_v7(),
            quantity: (event_type == "waste").then(|| "1.25".into()),
            severity: (event_type == "stockout").then(|| "some_orders".into()),
            reason: if event_type == "waste" {
                "spoilage".into()
            } else {
                "delivery_late_or_missed".into()
            },
            note: None,
        }
    }

    #[test]
    fn accepts_every_closed_reason_and_severity() {
        for reason in WASTE_REASONS {
            let mut value = input("waste");
            value.reason = (*reason).into();
            assert!(value.validated().is_ok(), "waste reason {reason}");
        }
        for reason in STOCKOUT_REASONS {
            for severity in STOCKOUT_SEVERITIES {
                let mut value = input("stockout");
                value.reason = (*reason).into();
                value.severity = Some((*severity).into());
                assert!(
                    value.validated().is_ok(),
                    "stockout reason {reason}, severity {severity}"
                );
            }
        }
    }

    #[test]
    fn rejects_unknown_closed_values_and_invalid_combinations() {
        let mut waste_reason = input("waste");
        waste_reason.reason = "unknown".into();
        assert!(waste_reason.validated().is_err());
        let mut stockout_reason = input("stockout");
        stockout_reason.reason = "unknown".into();
        assert!(stockout_reason.validated().is_err());
        let mut severity = input("stockout");
        severity.severity = Some("critical".into());
        assert!(severity.validated().is_err());

        let mut waste_without_quantity = input("waste");
        waste_without_quantity.quantity = None;
        assert!(waste_without_quantity.validated().is_err());
        let mut waste_with_severity = input("waste");
        waste_with_severity.severity = Some("some_orders".into());
        assert!(waste_with_severity.validated().is_err());
        let mut stockout_with_quantity = input("stockout");
        stockout_with_quantity.quantity = Some("1".into());
        assert!(stockout_with_quantity.validated().is_err());
        let mut stockout_without_severity = input("stockout");
        stockout_without_severity.severity = None;
        assert!(stockout_without_severity.validated().is_err());
        assert!(input("other").validated().is_err());
    }

    #[test]
    fn rejects_zero_negative_non_exact_and_excess_scale_quantities() {
        for quantity in ["0", "-0.1", "1.1234567", "1e2", "+1", "1."] {
            let mut value = input("waste");
            value.quantity = Some(quantity.into());
            assert!(value.validated().is_err(), "quantity {quantity}");
        }
        for quantity in ["0.000001", "12", "12.123456"] {
            let mut value = input("waste");
            value.quantity = Some(quantity.into());
            assert!(value.validated().is_ok(), "quantity {quantity}");
        }
    }

    #[test]
    fn trims_notes_drops_blanks_and_enforces_length() {
        let mut trimmed = input("waste");
        trimmed.note = Some("  Prep table spill  ".into());
        assert_eq!(
            trimmed.validated().unwrap().note.as_deref(),
            Some("Prep table spill")
        );
        let mut blank = input("waste");
        blank.note = Some("   ".into());
        assert_eq!(blank.validated().unwrap().note, None);
        let mut maximum = input("waste");
        maximum.note = Some("a".repeat(500));
        assert!(maximum.validated().is_ok());
        let mut too_long = input("waste");
        too_long.note = Some("a".repeat(501));
        assert!(too_long.validated().is_err());
    }
}
