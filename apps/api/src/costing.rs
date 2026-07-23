use std::collections::HashSet;

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::{BigDecimal, RoundingMode};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error, invoices::strict_decimal};

const MAX_INGREDIENTS: usize = 30;

#[derive(sqlx::FromRow)]
struct Membership {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[derive(sqlx::FromRow)]
struct MenuRecord {
    id: Uuid,
    name: String,
    selling_price: BigDecimal,
    currency: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CostingMenuItem {
    id: Uuid,
    name: String,
    selling_price: String,
    currency: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct InventoryChoice {
    id: Uuid,
    name: String,
    category: Option<String>,
    count_unit: String,
}

#[derive(sqlx::FromRow)]
struct ConfiguredRow {
    id: Uuid,
    inventory_item_id: Uuid,
    inventory_item_name: String,
    inventory_item_category: Option<String>,
    inventory_item_active: bool,
    quantity: BigDecimal,
    unit: String,
    source_line_id: Option<Uuid>,
    source_invoice_id: Option<Uuid>,
    supplier_name: Option<String>,
    invoice_date: Option<NaiveDate>,
    recorded_at: Option<DateTime<Utc>>,
    source_currency: Option<String>,
    source_description: Option<String>,
    purchase_quantity: Option<BigDecimal>,
    purchase_unit: Option<String>,
    unit_price: Option<BigDecimal>,
    line_total: Option<BigDecimal>,
    count_unit: Option<String>,
    count_units_per_purchase_unit: Option<BigDecimal>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Ingredient {
    id: Uuid,
    inventory_item_id: Uuid,
    inventory_item_name: String,
    inventory_item_category: Option<String>,
    inventory_item_active: bool,
    quantity: String,
    unit: String,
    calculation: IngredientCalculation,
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum IngredientCalculation {
    Available {
        cost_per_serving: String,
        currency: String,
        source: SourceFacts,
        arithmetic: Arithmetic,
    },
    Unavailable {
        reason: UnavailableReason,
        recovery: &'static str,
        source: Option<SourceFacts>,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
enum UnavailableReason {
    NoRecordedReceipt,
    CurrencyMismatch,
    UnsupportedReceiptUnit,
    IncompatibleUnits,
    InvalidConversion,
    UnusablePrice,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SourceFacts {
    invoice_id: Uuid,
    source_line_id: Uuid,
    supplier_name: String,
    invoice_date: NaiveDate,
    recorded_at: DateTime<Utc>,
    currency: String,
    description: String,
    purchase_quantity: Option<String>,
    purchase_unit: Option<String>,
    line_total: Option<String>,
    unit_price: Option<String>,
    count_unit: Option<String>,
    count_units_per_purchase_unit: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct Arithmetic {
    price_basis: PriceBasis,
    purchase_unit_cost: String,
    ingredient_quantity_in_count_unit: String,
    formula: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
enum PriceBasis {
    LineTotalDividedByPurchaseQuantity,
    UnitPrice,
}

#[derive(Debug, Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum Summary {
    Complete {
        known_subtotal: String,
        currency: String,
        approximate_ingredient_cost_percentage: String,
        configured_ingredient_count: usize,
        known_ingredient_count: usize,
    },
    Partial {
        known_subtotal: String,
        currency: String,
        configured_ingredient_count: usize,
        known_ingredient_count: usize,
    },
    Unavailable {
        currency: String,
        configured_ingredient_count: usize,
        known_ingredient_count: usize,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CostingResponse {
    menu_item: CostingMenuItem,
    inventory_items: Vec<InventoryChoice>,
    ingredients: Vec<Ingredient>,
    summary: Summary,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ReplaceIngredients {
    ingredients: Vec<IngredientInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct IngredientInput {
    inventory_item_id: Uuid,
    quantity: String,
    unit: String,
}

struct ValidatedIngredient {
    inventory_item_id: Uuid,
    quantity: BigDecimal,
    unit: String,
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum Unit {
    Gram,
    Kilogram,
    Ounce,
    Pound,
    Milliliter,
    Liter,
    UsFluidOunce,
    UsGallon,
    Each,
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<CostingResponse>, ApiError> {
    let member = membership(&state, &headers).await?;
    require_manager(&member.role)?;
    Ok(Json(
        load_costing(&state.pool, member.restaurant_id, id).await?,
    ))
}

pub(crate) async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<ReplaceIngredients>,
) -> Result<Json<CostingResponse>, ApiError> {
    let member = membership(&state, &headers).await?;
    require_manager(&member.role)?;
    let ingredients = input.validated()?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;

    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(member.restaurant_id)
        .fetch_one(&mut *tx)
        .await
        .map_err(database_error)?;
    let menu_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM menu_items WHERE id=$1 AND restaurant_id=$2)",
    )
    .bind(id)
    .bind(member.restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if !menu_exists {
        return Err(ApiError(StatusCode::NOT_FOUND, "Menu item not found."));
    }

    let existing = sqlx::query_scalar::<_, Uuid>(
        "SELECT inventory_item_id FROM menu_item_ingredients
         WHERE menu_item_id=$1 AND restaurant_id=$2",
    )
    .bind(id)
    .bind(member.restaurant_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(database_error)?
    .into_iter()
    .collect::<HashSet<_>>();

    let requested_ids = ingredients
        .iter()
        .map(|ingredient| ingredient.inventory_item_id)
        .collect::<Vec<_>>();
    if !requested_ids.is_empty() {
        let items = sqlx::query_as::<_, (Uuid, bool)>(
            "SELECT id,active FROM inventory_items
             WHERE restaurant_id=$1 AND id=ANY($2) FOR SHARE",
        )
        .bind(member.restaurant_id)
        .bind(&requested_ids)
        .fetch_all(&mut *tx)
        .await
        .map_err(database_error)?;
        if items.len() != requested_ids.len()
            || items
                .iter()
                .any(|(item_id, active)| !active && !existing.contains(item_id))
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose active inventory items from this restaurant. Archived ingredients can only remain in an existing setup.",
            ));
        }
    }

    sqlx::query("DELETE FROM menu_item_ingredients WHERE menu_item_id=$1 AND restaurant_id=$2")
        .bind(id)
        .bind(member.restaurant_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    for ingredient in ingredients {
        sqlx::query(
            "INSERT INTO menu_item_ingredients
             (id,restaurant_id,menu_item_id,inventory_item_id,quantity,unit,created_by,updated_by)
             VALUES($1,$2,$3,$4,$5,$6,$7,$7)",
        )
        .bind(Uuid::now_v7())
        .bind(member.restaurant_id)
        .bind(id)
        .bind(ingredient.inventory_item_id)
        .bind(ingredient.quantity)
        .bind(ingredient.unit)
        .bind(member.user_id)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    }
    tx.commit().await.map_err(database_error)?;

    Ok(Json(
        load_costing(&state.pool, member.restaurant_id, id).await?,
    ))
}

async fn load_costing(
    pool: &sqlx::PgPool,
    restaurant_id: Uuid,
    menu_item_id: Uuid,
) -> Result<CostingResponse, ApiError> {
    let menu = sqlx::query_as::<_, MenuRecord>(
        "SELECT id,name,selling_price,currency FROM menu_items
         WHERE id=$1 AND restaurant_id=$2",
    )
    .bind(menu_item_id)
    .bind(restaurant_id)
    .fetch_optional(pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(StatusCode::NOT_FOUND, "Menu item not found."))?;
    let inventory_items = sqlx::query_as::<_, InventoryChoice>(
        "SELECT id,name,category,count_unit FROM inventory_items
         WHERE restaurant_id=$1 AND active
         ORDER BY category NULLS LAST,LOWER(name),id",
    )
    .bind(restaurant_id)
    .fetch_all(pool)
    .await
    .map_err(database_error)?;
    let rows = sqlx::query_as::<_, ConfiguredRow>(
        "SELECT ingredient.id,ingredient.inventory_item_id,item.name AS inventory_item_name,
                item.category AS inventory_item_category,item.active AS inventory_item_active,
                ingredient.quantity,ingredient.unit,
                source.source_line_id,source.invoice_id AS source_invoice_id,
                source.supplier_name,source.invoice_date,source.recorded_at,
                source.currency AS source_currency,source.description AS source_description,
                source.purchase_quantity,source.purchase_unit,source.unit_price,source.line_total,
                source.count_unit,source.count_units_per_purchase_unit
         FROM menu_item_ingredients ingredient
         JOIN inventory_items item ON item.id=ingredient.inventory_item_id
           AND item.restaurant_id=ingredient.restaurant_id
         LEFT JOIN LATERAL (
             SELECT line.source_line_id,line.invoice_id,receipt.supplier_name,
                    receipt.invoice_date,receipt.recorded_at,receipt.currency,line.description,
                    line.purchase_quantity,line.purchase_unit,line.unit_price,line.line_total,
                    line.count_unit,line.count_units_per_purchase_unit
             FROM purchase_receipt_lines line
             JOIN purchase_receipts receipt ON receipt.invoice_id=line.invoice_id
               AND receipt.restaurant_id=line.restaurant_id
             WHERE line.restaurant_id=ingredient.restaurant_id
               AND line.inventory_item_id=ingredient.inventory_item_id
             ORDER BY receipt.invoice_date DESC,receipt.recorded_at DESC,
                      receipt.invoice_id DESC,line.source_line_id DESC
             LIMIT 1
         ) source ON TRUE
         WHERE ingredient.menu_item_id=$1 AND ingredient.restaurant_id=$2
         ORDER BY item.active DESC,item.category NULLS LAST,LOWER(item.name),item.id",
    )
    .bind(menu_item_id)
    .bind(restaurant_id)
    .fetch_all(pool)
    .await
    .map_err(database_error)?;

    let mut ingredients = Vec::with_capacity(rows.len());
    let mut known_costs = Vec::with_capacity(rows.len());
    for row in &rows {
        let (calculation, known_cost) = ingredient_calculation(row, &menu.currency);
        known_costs.push(known_cost);
        ingredients.push(Ingredient {
            id: row.id,
            inventory_item_id: row.inventory_item_id,
            inventory_item_name: row.inventory_item_name.clone(),
            inventory_item_category: row.inventory_item_category.clone(),
            inventory_item_active: row.inventory_item_active,
            quantity: exact(&row.quantity),
            unit: row.unit.clone(),
            calculation,
        });
    }
    let summary = summarize(&known_costs, &menu.selling_price, &menu.currency);
    Ok(CostingResponse {
        menu_item: CostingMenuItem {
            id: menu.id,
            name: menu.name,
            selling_price: exact(&menu.selling_price),
            currency: menu.currency,
        },
        inventory_items,
        ingredients,
        summary,
    })
}

fn ingredient_calculation(
    row: &ConfiguredRow,
    menu_currency: &str,
) -> (IngredientCalculation, Option<BigDecimal>) {
    let Some(source) = source_facts(row) else {
        return unavailable(
            UnavailableReason::NoRecordedReceipt,
            "Use Connect purchases on an approved invoice for this inventory item.",
            None,
        );
    };
    let source_currency = row
        .source_currency
        .as_deref()
        .expect("a receipt source has a currency");
    if source_currency != menu_currency {
        return unavailable(
            UnavailableReason::CurrencyMismatch,
            "The latest recorded purchase uses a different currency. Use Connect purchases on a newer invoice in the menu item's currency.",
            Some(source),
        );
    }
    let Some(count_unit) = row.count_unit.as_deref().and_then(Unit::parse) else {
        return unavailable(
            UnavailableReason::UnsupportedReceiptUnit,
            "The latest recorded purchase uses a package or unsupported count unit. Use Connect purchases on a newer invoice with a controlled serving-compatible unit.",
            Some(source),
        );
    };
    let Some(ingredient_unit) = Unit::parse(&row.unit) else {
        return unavailable(
            UnavailableReason::IncompatibleUnits,
            "Choose a controlled serving unit for this ingredient.",
            Some(source),
        );
    };
    let Some(converted_quantity) = convert(&row.quantity, ingredient_unit, count_unit) else {
        return unavailable(
            UnavailableReason::IncompatibleUnits,
            "The serving unit is not compatible with the latest receipt count unit. Change the serving unit or use Connect purchases on a compatible newer invoice.",
            Some(source),
        );
    };
    let Some(conversion) = row
        .count_units_per_purchase_unit
        .as_ref()
        .filter(|value| *value > &BigDecimal::from(0))
    else {
        return unavailable(
            UnavailableReason::InvalidConversion,
            "Use Connect purchases on a newer invoice with a positive purchase conversion.",
            Some(source),
        );
    };

    let positive_line_total = row
        .line_total
        .as_ref()
        .filter(|value| *value > &BigDecimal::from(0));
    let positive_purchase_quantity = row
        .purchase_quantity
        .as_ref()
        .filter(|value| *value > &BigDecimal::from(0));
    let (purchase_unit_cost, price_basis, price_formula) = if let (
        Some(line_total),
        Some(purchase_quantity),
    ) =
        (positive_line_total, positive_purchase_quantity)
    {
        (
            line_total / purchase_quantity,
            PriceBasis::LineTotalDividedByPurchaseQuantity,
            format!("{} ÷ {}", exact(line_total), exact(purchase_quantity)),
        )
    } else if let Some(unit_price) = row
        .unit_price
        .as_ref()
        .filter(|value| *value > &BigDecimal::from(0))
    {
        (unit_price.clone(), PriceBasis::UnitPrice, exact(unit_price))
    } else {
        return unavailable(
            UnavailableReason::UnusablePrice,
            "Use Connect purchases on a newer invoice with a positive line total and quantity, or a positive unit price.",
            Some(source),
        );
    };
    let cost = &purchase_unit_cost * &converted_quantity / conversion;
    let displayed_cost = display(&cost, 4);
    let formula = format!(
        "{price_formula} × {} ÷ {} = {displayed_cost} {menu_currency}",
        exact(&converted_quantity),
        exact(conversion)
    );
    (
        IngredientCalculation::Available {
            cost_per_serving: displayed_cost,
            currency: menu_currency.to_owned(),
            source,
            arithmetic: Arithmetic {
                price_basis,
                purchase_unit_cost: display(&purchase_unit_cost, 6),
                ingredient_quantity_in_count_unit: exact(&converted_quantity),
                formula,
            },
        },
        Some(cost),
    )
}

fn unavailable(
    reason: UnavailableReason,
    recovery: &'static str,
    source: Option<SourceFacts>,
) -> (IngredientCalculation, Option<BigDecimal>) {
    (
        IngredientCalculation::Unavailable {
            reason,
            recovery,
            source,
        },
        None,
    )
}

fn source_facts(row: &ConfiguredRow) -> Option<SourceFacts> {
    Some(SourceFacts {
        invoice_id: row.source_invoice_id?,
        source_line_id: row.source_line_id?,
        supplier_name: row.supplier_name.clone()?,
        invoice_date: row.invoice_date?,
        recorded_at: row.recorded_at?,
        currency: row.source_currency.clone()?,
        description: row.source_description.clone()?,
        purchase_quantity: row.purchase_quantity.as_ref().map(exact),
        purchase_unit: row.purchase_unit.clone(),
        line_total: row.line_total.as_ref().map(exact),
        unit_price: row.unit_price.as_ref().map(exact),
        count_unit: row.count_unit.clone(),
        count_units_per_purchase_unit: row.count_units_per_purchase_unit.as_ref().map(exact),
    })
}

fn summarize(costs: &[Option<BigDecimal>], selling_price: &BigDecimal, currency: &str) -> Summary {
    let known = costs.iter().flatten().count();
    if costs.is_empty() || known == 0 {
        return Summary::Unavailable {
            currency: currency.to_owned(),
            configured_ingredient_count: costs.len(),
            known_ingredient_count: known,
        };
    }
    let subtotal = costs
        .iter()
        .flatten()
        .fold(BigDecimal::from(0), |sum, cost| sum + cost);
    if known != costs.len() {
        return Summary::Partial {
            known_subtotal: display(&subtotal, 4),
            currency: currency.to_owned(),
            configured_ingredient_count: costs.len(),
            known_ingredient_count: known,
        };
    }
    let percentage = &subtotal / selling_price * BigDecimal::from(100);
    Summary::Complete {
        known_subtotal: display(&subtotal, 4),
        currency: currency.to_owned(),
        approximate_ingredient_cost_percentage: display(&percentage, 2),
        configured_ingredient_count: costs.len(),
        known_ingredient_count: known,
    }
}

fn convert(quantity: &BigDecimal, from: Unit, to: Unit) -> Option<BigDecimal> {
    if from == to {
        return Some(quantity.clone());
    }
    let (multiply, divisor) = match (from, to) {
        (Unit::Kilogram, Unit::Gram) => (1000, 1),
        (Unit::Gram, Unit::Kilogram) => (1, 1000),
        (Unit::Pound, Unit::Ounce) => (16, 1),
        (Unit::Ounce, Unit::Pound) => (1, 16),
        (Unit::Liter, Unit::Milliliter) => (1000, 1),
        (Unit::Milliliter, Unit::Liter) => (1, 1000),
        (Unit::UsGallon, Unit::UsFluidOunce) => (128, 1),
        (Unit::UsFluidOunce, Unit::UsGallon) => (1, 128),
        _ => return None,
    };
    Some(quantity * BigDecimal::from(multiply) / BigDecimal::from(divisor))
}

impl Unit {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "g" => Some(Self::Gram),
            "kg" => Some(Self::Kilogram),
            "oz" => Some(Self::Ounce),
            "lb" => Some(Self::Pound),
            "mL" => Some(Self::Milliliter),
            "L" => Some(Self::Liter),
            "fl_oz_us" => Some(Self::UsFluidOunce),
            "gal_us" => Some(Self::UsGallon),
            "each" => Some(Self::Each),
            _ => None,
        }
    }
}

impl ReplaceIngredients {
    fn validated(self) -> Result<Vec<ValidatedIngredient>, ApiError> {
        if self.ingredients.len() > MAX_INGREDIENTS {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose no more than 30 ingredients for one menu item.",
            ));
        }
        let mut seen = HashSet::with_capacity(self.ingredients.len());
        let mut ingredients = Vec::with_capacity(self.ingredients.len());
        for ingredient in self.ingredients {
            if !seen.insert(ingredient.inventory_item_id) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Choose each inventory item only once.",
                ));
            }
            let quantity = positive_quantity(&ingredient.quantity)?;
            if Unit::parse(&ingredient.unit).is_none() {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Choose a controlled mass, volume, or each serving unit.",
                ));
            }
            ingredients.push(ValidatedIngredient {
                inventory_item_id: ingredient.inventory_item_id,
                quantity,
                unit: ingredient.unit,
            });
        }
        ingredients.sort_by_key(|ingredient| ingredient.inventory_item_id);
        Ok(ingredients)
    }
}

fn positive_quantity(value: &str) -> Result<BigDecimal, ApiError> {
    let (integer, fraction) = value.split_once('.').unwrap_or((value, ""));
    if value.is_empty()
        || value.starts_with(['+', '-'])
        || integer.is_empty()
        || !integer.bytes().all(|byte| byte.is_ascii_digit())
        || value.contains('.') && fraction.is_empty()
        || !fraction.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(quantity_error());
    }
    let quantity = strict_decimal(value, 6).map_err(|_| quantity_error())?;
    if quantity <= 0 {
        return Err(quantity_error());
    }
    Ok(quantity.normalized())
}

fn quantity_error() -> ApiError {
    ApiError(
        StatusCode::UNPROCESSABLE_ENTITY,
        "Ingredient quantity must be a positive plain decimal with at most 6 decimal places.",
    )
}

fn display(value: &BigDecimal, scale: i64) -> String {
    value
        .with_scale_round(scale, RoundingMode::HalfUp)
        .to_string()
}

fn exact(value: &BigDecimal) -> String {
    value.normalized().to_plain_string()
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
            "Owner or manager access is required for ingredient costing.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decimal(value: &str) -> BigDecimal {
        value.parse().unwrap()
    }

    fn row(
        ingredient: (&str, &str),
        receipt_conversion: (&str, &str),
        prices: (Option<&str>, Option<&str>, Option<&str>),
        currency: &str,
    ) -> ConfiguredRow {
        let (ingredient_quantity, ingredient_unit) = ingredient;
        let (count_unit, conversion) = receipt_conversion;
        let (purchase_quantity, unit_price, line_total) = prices;
        ConfiguredRow {
            id: Uuid::now_v7(),
            inventory_item_id: Uuid::now_v7(),
            inventory_item_name: "Chicken".into(),
            inventory_item_category: None,
            inventory_item_active: true,
            quantity: decimal(ingredient_quantity),
            unit: ingredient_unit.into(),
            source_line_id: Some(Uuid::now_v7()),
            source_invoice_id: Some(Uuid::now_v7()),
            supplier_name: Some("Supplier".into()),
            invoice_date: Some(NaiveDate::from_ymd_opt(2026, 7, 1).unwrap()),
            recorded_at: Some(Utc::now()),
            source_currency: Some(currency.into()),
            source_description: Some("Chicken cases".into()),
            purchase_quantity: purchase_quantity.map(decimal),
            purchase_unit: Some("case".into()),
            unit_price: unit_price.map(decimal),
            line_total: line_total.map(decimal),
            count_unit: Some(count_unit.into()),
            count_units_per_purchase_unit: Some(decimal(conversion)),
        }
    }

    #[test]
    fn converts_only_exact_compatible_unit_pairs() {
        assert_eq!(
            convert(&decimal("2"), Unit::Kilogram, Unit::Gram),
            Some(decimal("2000"))
        );
        assert_eq!(
            convert(&decimal("8"), Unit::Ounce, Unit::Pound),
            Some(decimal("0.5"))
        );
        assert_eq!(
            convert(&decimal("1"), Unit::Liter, Unit::Milliliter),
            Some(decimal("1000"))
        );
        assert_eq!(
            convert(&decimal("64"), Unit::UsFluidOunce, Unit::UsGallon),
            Some(decimal("0.5"))
        );
        assert_eq!(convert(&decimal("1"), Unit::Gram, Unit::Ounce), None);
        assert_eq!(convert(&decimal("1"), Unit::Each, Unit::Gram), None);
        assert!(Unit::parse("case").is_none());
        let small = convert(&decimal("0.0001"), Unit::Gram, Unit::Kilogram).unwrap();
        assert_eq!(exact(&small), "0.0000001");
    }

    #[test]
    fn exact_case_formula_costs_one_dollar_per_serving() {
        let source = row(
            ("8", "oz"),
            ("lb", "20"),
            (Some("2"), Some("40"), Some("80")),
            "USD",
        );
        let (calculation, amount) = ingredient_calculation(&source, "USD");
        assert_eq!(amount, Some(decimal("1")));
        match calculation {
            IngredientCalculation::Available {
                cost_per_serving,
                arithmetic,
                ..
            } => {
                assert_eq!(cost_per_serving, "1.0000");
                assert!(matches!(
                    arithmetic.price_basis,
                    PriceBasis::LineTotalDividedByPurchaseQuantity
                ));
                assert_eq!(arithmetic.ingredient_quantity_in_count_unit, "0.5");
            }
            _ => panic!("expected an available cost"),
        }
    }

    #[test]
    fn line_total_takes_precedence_and_unit_price_is_the_fallback() {
        let preferred = row(
            ("1", "lb"),
            ("lb", "1"),
            (Some("2"), Some("99"), Some("20")),
            "USD",
        );
        assert_eq!(
            ingredient_calculation(&preferred, "USD").1,
            Some(decimal("10"))
        );

        let fallback = row(
            ("1", "lb"),
            ("lb", "1"),
            (Some("2"), Some("7.5"), Some("0")),
            "USD",
        );
        let (calculation, amount) = ingredient_calculation(&fallback, "USD");
        assert_eq!(amount, Some(decimal("7.5")));
        assert!(matches!(
            calculation,
            IngredientCalculation::Available {
                arithmetic: Arithmetic {
                    price_basis: PriceBasis::UnitPrice,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn aggregates_complete_partial_and_unavailable_without_partial_percentage() {
        let complete = summarize(
            &[Some(decimal("1.25")), Some(decimal("0.75"))],
            &decimal("10"),
            "USD",
        );
        assert!(matches!(
            complete,
            Summary::Complete {
                known_subtotal,
                approximate_ingredient_cost_percentage,
                ..
            } if known_subtotal == "2.0000" && approximate_ingredient_cost_percentage == "20.00"
        ));
        let partial = summarize(&[Some(decimal("1.25")), None], &decimal("10"), "USD");
        assert!(matches!(
            partial,
            Summary::Partial { known_subtotal, .. } if known_subtotal == "1.2500"
        ));
        assert!(matches!(
            summarize(&[None, None], &decimal("10"), "USD"),
            Summary::Unavailable {
                configured_ingredient_count: 2,
                known_ingredient_count: 0,
                ..
            }
        ));
        assert!(matches!(
            summarize(&[], &decimal("10"), "USD"),
            Summary::Unavailable {
                configured_ingredient_count: 0,
                ..
            }
        ));
    }

    #[test]
    fn validates_positive_exact_decimal_quantities() {
        assert_eq!(positive_quantity("1.123456").unwrap(), decimal("1.123456"));
        for invalid in [
            "",
            "0",
            "-1",
            "+1",
            " 1",
            "1 ",
            "1.",
            ".5",
            "1e2",
            "1.1234567",
        ] {
            assert!(positive_quantity(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn currency_mismatch_is_explicitly_unavailable() {
        let source = row(
            ("1", "lb"),
            ("lb", "1"),
            (Some("1"), Some("4"), None),
            "CAD",
        );
        let (calculation, amount) = ingredient_calculation(&source, "USD");
        assert!(amount.is_none());
        assert!(matches!(
            calculation,
            IngredientCalculation::Unavailable {
                reason: UnavailableReason::CurrencyMismatch,
                source: Some(SourceFacts { currency, .. }),
                ..
            } if currency == "CAD"
        ));
    }
}
