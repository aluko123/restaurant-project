use std::collections::{HashMap, HashSet};

use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use bigdecimal::BigDecimal;
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
    ApiError, AppState, database_error,
    inventory::{ItemInput, ValidItem, item_write_error},
    invoices::{membership, strict_decimal_with_precision},
};

#[derive(sqlx::FromRow)]
struct InvoiceHeader {
    id: Uuid,
    restaurant_id: Uuid,
    supplier_name: String,
    supplier_key: String,
    invoice_number: Option<String>,
    invoice_date: NaiveDate,
    currency: String,
}

#[derive(sqlx::FromRow)]
struct ReceiptHeader {
    invoice_id: Uuid,
    supplier_name: String,
    invoice_number: Option<String>,
    invoice_date: NaiveDate,
    currency: String,
    recorded_at: DateTime<Utc>,
}

#[derive(Clone, sqlx::FromRow)]
struct SourceLine {
    id: Uuid,
    position: i32,
    sku: Option<String>,
    description: String,
    quantity: Option<BigDecimal>,
    unit: Option<String>,
    unit_price: Option<BigDecimal>,
    line_total: Option<BigDecimal>,
    comparison_key: Option<String>,
    comparison_unit: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PurchaseInvoice {
    invoice_id: Uuid,
    supplier_name: String,
    invoice_number: Option<String>,
    invoice_date: NaiveDate,
    currency: String,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
pub(crate) struct InventoryChoice {
    id: Uuid,
    name: String,
    category: Option<String>,
    count_unit: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct PendingLine {
    id: Uuid,
    position: i32,
    sku: Option<String>,
    description: String,
    quantity: Option<String>,
    unit: Option<String>,
    unit_price: Option<String>,
    line_total: Option<String>,
    can_track: bool,
    suggested_inventory_item_id: Option<Uuid>,
    suggested_conversion: Option<String>,
}

#[derive(Serialize, sqlx::FromRow)]
#[serde(rename_all = "camelCase")]
struct ReceiptLine {
    id: Uuid,
    position: i32,
    resolution: String,
    sku: Option<String>,
    description: String,
    quantity: Option<String>,
    unit: Option<String>,
    unit_price: Option<String>,
    line_total: Option<String>,
    inventory_item_id: Option<Uuid>,
    inventory_item_name: Option<String>,
    count_unit: Option<String>,
    conversion: Option<String>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct Receipt {
    invoice: PurchaseInvoice,
    recorded_at: DateTime<Utc>,
    lines: Vec<ReceiptLine>,
}

#[derive(Serialize)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub(crate) enum PurchaseResponse {
    Pending {
        invoice: PurchaseInvoice,
        inventory_items: Vec<InventoryChoice>,
        lines: Vec<PendingLine>,
    },
    Recorded {
        receipt: Receipt,
        already_recorded: bool,
    },
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RecordInput {
    resolutions: Vec<ResolutionInput>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ResolutionInput {
    line_id: Uuid,
    #[serde(flatten)]
    decision: DecisionInput,
}

#[derive(Deserialize)]
#[serde(
    tag = "action",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
enum DecisionInput {
    Ignore,
    Match {
        inventory_item_id: Uuid,
        expected_count_unit: String,
        conversion: String,
    },
    Create {
        name: String,
        category: Option<String>,
        count_unit: String,
        conversion: String,
    },
}

struct ResolvedLine {
    source: SourceLine,
    resolution: &'static str,
    inventory_item_id: Option<Uuid>,
    inventory_item_name: Option<String>,
    count_unit: Option<String>,
    conversion: Option<BigDecimal>,
}

pub(crate) async fn review(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<PurchaseResponse>, ApiError> {
    let member = membership(&state, &headers).await?;
    if let Some(receipt) = load_receipt(&state.pool, id, member.restaurant_id).await? {
        return Ok(Json(PurchaseResponse::Recorded {
            receipt,
            already_recorded: true,
        }));
    }
    let invoice = load_invoice(&state.pool, id, member.restaurant_id, false).await?;

    let source_lines = load_source_lines(&state.pool, id).await?;
    let inventory_items = sqlx::query_as::<_, InventoryChoice>(
        "SELECT id,name,category,count_unit FROM inventory_items
         WHERE restaurant_id=$1 AND active ORDER BY category NULLS LAST,LOWER(name),id",
    )
    .bind(member.restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    let suggestions = load_suggestions(&state.pool, &invoice, &source_lines).await?;
    let lines = source_lines
        .into_iter()
        .map(|line| {
            let suggestion = suggestions.get(&line.id);
            let can_track = trackable(&line);
            PendingLine {
                id: line.id,
                position: line.position,
                sku: line.sku,
                description: line.description,
                quantity: line.quantity.as_ref().map(ToString::to_string),
                unit: line.unit,
                unit_price: line.unit_price.as_ref().map(ToString::to_string),
                line_total: line.line_total.as_ref().map(ToString::to_string),
                can_track,
                suggested_inventory_item_id: suggestion.map(|value| value.0),
                suggested_conversion: suggestion.map(|value| value.1.to_string()),
            }
        })
        .collect();
    Ok(Json(PurchaseResponse::Pending {
        invoice: public_invoice(&invoice),
        inventory_items,
        lines,
    }))
}

pub(crate) async fn record(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
    Json(input): Json<RecordInput>,
) -> Result<Json<PurchaseResponse>, ApiError> {
    let member = membership(&state, &headers).await?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    let invoice = load_invoice(&mut *tx, id, member.restaurant_id, true).await?;
    let receipt_exists = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM purchase_receipts WHERE invoice_id=$1 AND restaurant_id=$2)",
    )
    .bind(invoice.id)
    .bind(invoice.restaurant_id)
    .fetch_one(&mut *tx)
    .await
    .map_err(database_error)?;
    if receipt_exists {
        tx.commit().await.map_err(database_error)?;
        let receipt = load_receipt(&state.pool, invoice.id, invoice.restaurant_id)
            .await?
            .expect("existing receipt disappeared");
        return Ok(Json(PurchaseResponse::Recorded {
            receipt,
            already_recorded: true,
        }));
    }

    let source_lines = load_source_lines(&mut *tx, id).await?;
    let mut decisions = HashMap::new();
    for resolution in input.resolutions {
        if decisions
            .insert(resolution.line_id, resolution.decision)
            .is_some()
        {
            return Err(ApiError(
                StatusCode::UNPROCESSABLE_ENTITY,
                "Choose one action for every invoice line.",
            ));
        }
    }
    if decisions.len() != source_lines.len()
        || source_lines
            .iter()
            .any(|line| !decisions.contains_key(&line.id))
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose one action for every invoice line.",
        ));
    }

    let mut matched_ids = decisions
        .values()
        .filter_map(|decision| match decision {
            DecisionInput::Match {
                inventory_item_id, ..
            } => Some(*inventory_item_id),
            _ => None,
        })
        .collect::<Vec<_>>();
    matched_ids.sort_unstable();
    matched_ids.dedup();
    let mut matched_items = HashMap::new();
    for inventory_item_id in matched_ids {
        let item = sqlx::query_as::<_, (Uuid, String, String)>(
            "SELECT id,name,count_unit FROM inventory_items
             WHERE id=$1 AND restaurant_id=$2 AND active FOR SHARE",
        )
        .bind(inventory_item_id)
        .bind(member.restaurant_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(database_error)?
        .ok_or(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Choose an active inventory item from this restaurant.",
        ))?;
        matched_items.insert(item.0, (item.1, item.2));
    }
    for decision in decisions.values() {
        if let DecisionInput::Match {
            inventory_item_id,
            expected_count_unit,
            ..
        } = decision
        {
            let (_, current_count_unit) = matched_items
                .get(inventory_item_id)
                .expect("matched items were loaded");
            if expected_count_unit != current_count_unit {
                return Err(ApiError(
                    StatusCode::CONFLICT,
                    "This inventory item's count unit changed. Review the conversion and try again.",
                ));
            }
        }
    }

    let mut create_names = HashSet::new();
    let mut prepared_creates = HashMap::new();
    for line in &source_lines {
        if let Some(DecisionInput::Create {
            name,
            category,
            count_unit,
            ..
        }) = decisions.get(&line.id)
        {
            let item = ItemInput {
                name: name.clone(),
                category: category.clone(),
                count_unit: count_unit.clone(),
                par_level: None,
                active: true,
            }
            .validated()?;
            if !create_names.insert(item.name.to_lowercase()) {
                return Err(ApiError(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    "Create each new inventory item only once.",
                ));
            }
            prepared_creates.insert(line.id, item);
        }
    }

    sqlx::query(
        "INSERT INTO purchase_receipts
         (invoice_id,restaurant_id,supplier_name,invoice_number,invoice_date,currency,recorded_by)
         VALUES($1,$2,$3,$4,$5,$6,$7)",
    )
    .bind(invoice.id)
    .bind(invoice.restaurant_id)
    .bind(&invoice.supplier_name)
    .bind(&invoice.invoice_number)
    .bind(invoice.invoice_date)
    .bind(&invoice.currency)
    .bind(member.user_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;

    let mut resolved = Vec::with_capacity(source_lines.len());
    for source in source_lines {
        let decision = decisions
            .remove(&source.id)
            .expect("line set was validated");
        let line = match decision {
            DecisionInput::Ignore => ResolvedLine {
                source,
                resolution: "ignored",
                inventory_item_id: None,
                inventory_item_name: None,
                count_unit: None,
                conversion: None,
            },
            DecisionInput::Match {
                inventory_item_id,
                conversion,
                ..
            } => {
                ensure_trackable(&source)?;
                let conversion = positive_conversion(&conversion)?;
                let (name, count_unit) = matched_items
                    .get(&inventory_item_id)
                    .expect("matched items were loaded");
                ResolvedLine {
                    source,
                    resolution: "matched",
                    inventory_item_id: Some(inventory_item_id),
                    inventory_item_name: Some(name.clone()),
                    count_unit: Some(count_unit.clone()),
                    conversion: Some(conversion),
                }
            }
            DecisionInput::Create { conversion, .. } => {
                ensure_trackable(&source)?;
                let conversion = positive_conversion(&conversion)?;
                let item = prepared_creates
                    .remove(&source.id)
                    .expect("created item was validated");
                create_inventory_item(&mut tx, member.restaurant_id, &item)
                    .await
                    .map(|(inventory_item_id, name, count_unit)| ResolvedLine {
                        source,
                        resolution: "created",
                        inventory_item_id: Some(inventory_item_id),
                        inventory_item_name: Some(name),
                        count_unit: Some(count_unit),
                        conversion: Some(conversion),
                    })?
            }
        };
        insert_receipt_line(&mut tx, &invoice, &line).await?;
        resolved.push(line);
    }
    save_mappings(&mut tx, &invoice, member.user_id, &resolved).await?;
    tx.commit().await.map_err(database_error)?;

    let receipt = load_receipt(&state.pool, invoice.id, invoice.restaurant_id)
        .await?
        .expect("receipt was committed");
    Ok(Json(PurchaseResponse::Recorded {
        receipt,
        already_recorded: false,
    }))
}

async fn load_invoice<'e, E>(
    executor: E,
    id: Uuid,
    restaurant_id: Uuid,
    lock: bool,
) -> Result<InvoiceHeader, ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let lock = if lock { " FOR UPDATE OF invoice" } else { "" };
    let query = format!(
        "SELECT invoice.id,invoice.restaurant_id,invoice.supplier_name,
                LOWER(BTRIM(invoice.supplier_name)) supplier_key,
                extraction.invoice_number,invoice.invoice_date,extraction.currency
         FROM invoices invoice JOIN invoice_extractions extraction ON extraction.invoice_id=invoice.id
         WHERE invoice.id=$1 AND invoice.restaurant_id=$2 AND invoice.status='ready'{lock}"
    );
    sqlx::query_as::<_, InvoiceHeader>(&query)
        .bind(id)
        .bind(restaurant_id)
        .fetch_optional(executor)
        .await
        .map_err(database_error)?
        .ok_or(ApiError(
            StatusCode::NOT_FOUND,
            "Connect purchases after this invoice is approved.",
        ))
}

async fn load_source_lines<'e, E>(
    executor: E,
    invoice_id: Uuid,
) -> Result<Vec<SourceLine>, ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    sqlx::query_as::<_, SourceLine>(
        "SELECT id,position,sku,description,quantity,unit,unit_price,line_total,
                comparison_key,comparison_unit
         FROM invoice_line_items WHERE invoice_id=$1 ORDER BY position,id",
    )
    .bind(invoice_id)
    .fetch_all(executor)
    .await
    .map_err(database_error)
}

async fn load_suggestions<'e, E>(
    executor: E,
    invoice: &InvoiceHeader,
    lines: &[SourceLine],
) -> Result<HashMap<Uuid, (Uuid, BigDecimal)>, ApiError>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let mut counts = HashMap::new();
    for line in lines {
        if let (Some(key), Some(unit)) = (&line.comparison_key, &line.comparison_unit) {
            *counts.entry((key.clone(), unit.clone())).or_insert(0usize) += 1;
        }
    }
    let rows = sqlx::query_as::<_, (String, String, Uuid, BigDecimal)>(
        "SELECT mapping.comparison_key,mapping.comparison_unit,mapping.inventory_item_id,
                mapping.count_units_per_purchase_unit
         FROM supplier_product_mappings mapping
         JOIN inventory_items item ON item.id=mapping.inventory_item_id
           AND item.restaurant_id=mapping.restaurant_id AND item.active
         WHERE mapping.restaurant_id=$1 AND mapping.supplier_key=$2",
    )
    .bind(invoice.restaurant_id)
    .bind(&invoice.supplier_key)
    .fetch_all(executor)
    .await
    .map_err(database_error)?;
    let mappings = rows
        .into_iter()
        .map(|(key, unit, item, conversion)| ((key, unit), (item, conversion)))
        .collect::<HashMap<_, _>>();
    Ok(lines
        .iter()
        .filter_map(|line| {
            let identity = (line.comparison_key.clone()?, line.comparison_unit.clone()?);
            (counts.get(&identity) == Some(&1)).then(|| {
                mappings
                    .get(&identity)
                    .cloned()
                    .map(|value| (line.id, value))
            })?
        })
        .collect())
}

async fn create_inventory_item(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    restaurant_id: Uuid,
    item: &ValidItem,
) -> Result<(Uuid, String, String), ApiError> {
    let id = Uuid::now_v7();
    sqlx::query(
        "INSERT INTO inventory_items(id,restaurant_id,name,category,count_unit,par_level,active)
         VALUES($1,$2,$3,$4,$5,$6,TRUE)",
    )
    .bind(id)
    .bind(restaurant_id)
    .bind(&item.name)
    .bind(&item.category)
    .bind(&item.count_unit)
    .bind(&item.par_level)
    .execute(&mut **tx)
    .await
    .map_err(item_write_error)?;
    Ok((id, item.name.clone(), item.count_unit.clone()))
}

async fn insert_receipt_line(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    invoice: &InvoiceHeader,
    line: &ResolvedLine,
) -> Result<(), ApiError> {
    sqlx::query(
        "INSERT INTO purchase_receipt_lines
         (invoice_id,restaurant_id,source_line_id,position,resolution,supplier_sku,description,
          purchase_quantity,purchase_unit,unit_price,line_total,inventory_item_id,
          inventory_item_name,count_unit,count_units_per_purchase_unit)
         VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
    )
    .bind(invoice.id)
    .bind(invoice.restaurant_id)
    .bind(line.source.id)
    .bind(line.source.position)
    .bind(line.resolution)
    .bind(&line.source.sku)
    .bind(&line.source.description)
    .bind(&line.source.quantity)
    .bind(&line.source.unit)
    .bind(&line.source.unit_price)
    .bind(&line.source.line_total)
    .bind(line.inventory_item_id)
    .bind(&line.inventory_item_name)
    .bind(&line.count_unit)
    .bind(&line.conversion)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn save_mappings(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    invoice: &InvoiceHeader,
    user_id: Uuid,
    lines: &[ResolvedLine],
) -> Result<(), ApiError> {
    let mut counts = HashMap::new();
    for line in lines {
        if let (Some(key), Some(unit)) = (&line.source.comparison_key, &line.source.comparison_unit)
        {
            *counts.entry((key.clone(), unit.clone())).or_insert(0usize) += 1;
        }
    }
    let mut eligible = lines
        .iter()
        .filter(|line| line.resolution != "ignored")
        .filter_map(|line| {
            let key = line.source.comparison_key.as_ref()?;
            let unit = line.source.comparison_unit.as_ref()?;
            (counts.get(&(key.clone(), unit.clone())) == Some(&1)).then_some((key, unit, line))
        })
        .collect::<Vec<_>>();
    eligible.sort_by(|left, right| (left.0, left.1).cmp(&(right.0, right.1)));
    for (key, unit, line) in eligible {
        sqlx::query(
            "INSERT INTO supplier_product_mappings
             (id,restaurant_id,supplier_name,supplier_key,comparison_key,comparison_unit,
              product_description,supplier_sku,purchase_unit,inventory_item_id,
              count_units_per_purchase_unit,created_by)
             VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12)
             ON CONFLICT (restaurant_id,supplier_key,comparison_key,comparison_unit)
             DO UPDATE SET supplier_name=EXCLUDED.supplier_name,
               product_description=EXCLUDED.product_description,supplier_sku=EXCLUDED.supplier_sku,
               purchase_unit=EXCLUDED.purchase_unit,inventory_item_id=EXCLUDED.inventory_item_id,
               count_units_per_purchase_unit=EXCLUDED.count_units_per_purchase_unit,updated_at=NOW()",
        )
        .bind(Uuid::now_v7())
        .bind(invoice.restaurant_id)
        .bind(&invoice.supplier_name)
        .bind(&invoice.supplier_key)
        .bind(key)
        .bind(unit)
        .bind(&line.source.description)
        .bind(&line.source.sku)
        .bind(line.source.unit.as_deref().expect("trackable line has a unit"))
        .bind(line.inventory_item_id.expect("linked line has an item"))
        .bind(line.conversion.as_ref().expect("linked line has a conversion"))
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

async fn load_receipt(
    pool: &sqlx::PgPool,
    invoice_id: Uuid,
    restaurant_id: Uuid,
) -> Result<Option<Receipt>, ApiError> {
    let header = sqlx::query_as::<_, ReceiptHeader>(
        "SELECT invoice_id,supplier_name,invoice_number,invoice_date,currency,recorded_at
         FROM purchase_receipts WHERE invoice_id=$1 AND restaurant_id=$2",
    )
    .bind(invoice_id)
    .bind(restaurant_id)
    .fetch_optional(pool)
    .await
    .map_err(database_error)?;
    let Some(header) = header else {
        return Ok(None);
    };
    let lines = sqlx::query_as::<_, ReceiptLine>(
        "SELECT source_line_id id,position,resolution,supplier_sku sku,description,
                purchase_quantity::text quantity,purchase_unit unit,unit_price::text unit_price,
                line_total::text line_total,inventory_item_id,inventory_item_name,count_unit,
                count_units_per_purchase_unit::text conversion
         FROM purchase_receipt_lines
         WHERE invoice_id=$1 AND restaurant_id=$2 ORDER BY position,source_line_id",
    )
    .bind(invoice_id)
    .bind(restaurant_id)
    .fetch_all(pool)
    .await
    .map_err(database_error)?;
    Ok(Some(Receipt {
        invoice: PurchaseInvoice {
            invoice_id: header.invoice_id,
            supplier_name: header.supplier_name,
            invoice_number: header.invoice_number,
            invoice_date: header.invoice_date,
            currency: header.currency,
        },
        recorded_at: header.recorded_at,
        lines,
    }))
}

fn public_invoice(invoice: &InvoiceHeader) -> PurchaseInvoice {
    PurchaseInvoice {
        invoice_id: invoice.id,
        supplier_name: invoice.supplier_name.clone(),
        invoice_number: invoice.invoice_number.clone(),
        invoice_date: invoice.invoice_date,
        currency: invoice.currency.clone(),
    }
}

fn trackable(line: &SourceLine) -> bool {
    line.quantity
        .as_ref()
        .is_some_and(|quantity| quantity > &BigDecimal::from(0))
        && line
            .unit
            .as_ref()
            .is_some_and(|unit| !unit.trim().is_empty())
}

fn ensure_trackable(line: &SourceLine) -> Result<(), ApiError> {
    if trackable(line) {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Only lines with a positive quantity and purchase unit can connect to inventory.",
        ))
    }
}

fn positive_conversion(value: &str) -> Result<BigDecimal, ApiError> {
    let value = value.trim();
    let conversion = strict_decimal_with_precision(value, 24, 12).map_err(|_| {
        ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Conversion must be a positive plain decimal with at most 12 decimal places.",
        )
    })?;
    if conversion <= 0 {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Conversion must be greater than zero.",
        ));
    }
    Ok(conversion)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn conversion_is_positive_and_exact() {
        assert_eq!(positive_conversion(" 24 ").unwrap().to_string(), "24");
        assert!(positive_conversion("0").is_err());
        assert!(positive_conversion("-1").is_err());
        assert!(positive_conversion("1.1234567890123").is_err());
        assert!(positive_conversion("1e2").is_err());
    }

    #[test]
    fn purchase_contract_uses_camel_case_fields() {
        let item_id = Uuid::now_v7();
        let line_id = Uuid::now_v7();
        let input: RecordInput = serde_json::from_value(json!({
            "resolutions": [{
                "lineId": line_id,
                "action": "match",
                "inventoryItemId": item_id,
                "expectedCountUnit": "lb",
                "conversion": "24"
            }]
        }))
        .unwrap();
        assert_eq!(input.resolutions.len(), 1);
        assert!(matches!(
            &input.resolutions[0].decision,
            DecisionInput::Match { inventory_item_id, .. } if *inventory_item_id == item_id
        ));

        let response = PurchaseResponse::Pending {
            invoice: PurchaseInvoice {
                invoice_id: Uuid::now_v7(),
                supplier_name: "Supplier".into(),
                invoice_number: None,
                invoice_date: NaiveDate::from_ymd_opt(2026, 7, 22).unwrap(),
                currency: "USD".into(),
            },
            inventory_items: vec![InventoryChoice {
                id: item_id,
                name: "Chicken".into(),
                category: None,
                count_unit: "lb".into(),
            }],
            lines: Vec::new(),
        };
        let json = serde_json::to_value(response).unwrap();
        assert!(json.get("inventoryItems").is_some());
        assert!(json.get("inventory_items").is_none());
    }
}
