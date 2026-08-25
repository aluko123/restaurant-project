use axum::{
    Json,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::{Deserialize, Serialize};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;

use crate::{ApiError, AppState, authenticated_subject, database_error};

const STREAMS: [&str; 5] = [
    "menu",
    "sales",
    "inventory",
    "purchases",
    "bookkeeping_export",
];

#[derive(sqlx::FromRow)]
struct Actor {
    restaurant_id: Uuid,
    user_id: Uuid,
    role: String,
}

#[derive(sqlx::FromRow)]
struct SetupMeta {
    setup_approach: Option<String>,
    setup_assistance_requested_at: Option<DateTime<Utc>>,
    completed_at: Option<DateTime<Utc>>,
}

#[derive(Clone, sqlx::FromRow)]
struct SelectionRow {
    stream: String,
    method: String,
    owner: String,
    connector_provider: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(sqlx::FromRow)]
struct EvidenceRow {
    inventory_count: i64,
    inventory_latest_at: Option<DateTime<Utc>>,
    focused_item_count: i64,
    unresolved_import_unit_count: i64,
    completed_count_count: i64,
    verified_count_count: i64,
    last_completed_count_at: Option<DateTime<Utc>>,
    menu_count: i64,
    menu_latest_at: Option<DateTime<Utc>>,
    sales_count: i64,
    last_sales_date: Option<NaiveDate>,
    purchase_count: i64,
    purchase_latest_at: Option<DateTime<Utc>>,
    menu_pending_count: i64,
    menu_failed_count: i64,
    menu_review_count: i64,
    inventory_review_count: i64,
    inventory_draft_count: i64,
    purchase_pending_count: i64,
    purchase_failed_count: i64,
    purchase_review_count: i64,
    source_sync_pending_count: i64,
}

#[derive(Clone, sqlx::FromRow)]
struct ConnectorStatus {
    status: String,
    last_success_at: Option<DateTime<Utc>>,
    last_error: Option<String>,
    menu_last_success_at: Option<DateTime<Utc>>,
    sales_last_success_at: Option<DateTime<Utc>>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SetupResponse {
    setup_approach: Option<String>,
    assistance_requested_at: Option<DateTime<Utc>>,
    setup_exited_at: Option<DateTime<Utc>>,
    activation_state: &'static str,
    first_count_handoff: FirstCountHandoff,
    streams: Vec<SetupStream>,
    connectors: Vec<ConnectorView>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FirstCountHandoff {
    focused_item_count: i64,
    unresolved_critical_import_unit_count: i64,
    state: &'static str,
    next_action: Option<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SetupStream {
    stream: &'static str,
    selection: Option<SelectionView>,
    lifecycle: &'static str,
    next_action: Option<&'static str>,
    evidence: EvidenceView,
    issue: Option<IssueView>,
    supported_methods: Vec<&'static str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SelectionView {
    method: String,
    owner: String,
    connector_provider: Option<String>,
    updated_at: DateTime<Utc>,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct EvidenceView {
    record_count: i64,
    latest_at: Option<DateTime<Utc>>,
    latest_business_date: Option<NaiveDate>,
    completed_count: i64,
    last_successful_sync_at: Option<DateTime<Utc>>,
    backlog: BacklogView,
}

#[derive(Serialize, Default)]
#[serde(rename_all = "camelCase")]
struct BacklogView {
    pending_count: i64,
    failed_count: i64,
    review_count: i64,
}

#[derive(Serialize)]
struct IssueView {
    code: &'static str,
    message: String,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ConnectorView {
    provider: &'static str,
    supported: bool,
    configured: bool,
    selected: bool,
    capabilities: [&'static str; 2],
    status: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub(crate) struct UpdateStream {
    method: String,
    owner: String,
    connector_provider: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct SelectConnector {
    selected: bool,
}

pub(crate) async fn get(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<SetupResponse>, ApiError> {
    let actor = actor(&state, &headers).await?;
    manager(&actor)?;
    Ok(Json(load(&state, actor.restaurant_id).await?))
}

pub(crate) async fn put_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(stream): Path<String>,
    Json(input): Json<UpdateStream>,
) -> Result<Json<SetupResponse>, ApiError> {
    let actor = actor(&state, &headers).await?;
    manager(&actor)?;
    let input = validate_stream(&stream, input)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    lock_restaurant(&mut tx, actor.restaurant_id).await?;
    if matches!(stream.as_str(), "menu" | "sales")
        && let Some(provider) = active_connector(&mut tx, actor.restaurant_id).await?
        && !(input.method == "connector"
            && input.connector_provider.as_deref() == Some(provider.as_str()))
    {
        return Err(deselect_conflict(&provider));
    }
    sqlx::query(
        "INSERT INTO restaurant_setup_streams
         (restaurant_id,stream,method,owner,connector_provider,created_by,updated_by)
         VALUES($1,$2,$3,$4,$5,$6,$6)
         ON CONFLICT (restaurant_id,stream) DO UPDATE SET
           method=EXCLUDED.method,owner=EXCLUDED.owner,
           connector_provider=EXCLUDED.connector_provider,
           updated_by=EXCLUDED.updated_by,updated_at=NOW()",
    )
    .bind(actor.restaurant_id)
    .bind(&stream)
    .bind(input.method)
    .bind(input.owner)
    .bind(input.connector_provider)
    .bind(actor.user_id)
    .execute(&mut *tx)
    .await
    .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(&state, actor.restaurant_id).await?))
}

pub(crate) async fn delete_stream(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(stream): Path<String>,
) -> Result<Json<SetupResponse>, ApiError> {
    let actor = actor(&state, &headers).await?;
    manager(&actor)?;
    valid_stream(&stream)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    lock_restaurant(&mut tx, actor.restaurant_id).await?;
    if matches!(stream.as_str(), "menu" | "sales")
        && let Some(provider) = active_connector(&mut tx, actor.restaurant_id).await?
    {
        return Err(deselect_conflict(&provider));
    }
    sqlx::query("DELETE FROM restaurant_setup_streams WHERE restaurant_id=$1 AND stream=$2")
        .bind(actor.restaurant_id)
        .bind(stream)
        .execute(&mut *tx)
        .await
        .map_err(database_error)?;
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(&state, actor.restaurant_id).await?))
}

pub(crate) async fn put_connector(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(provider): Path<String>,
    Json(input): Json<SelectConnector>,
) -> Result<Json<SetupResponse>, ApiError> {
    if !CONNECTOR_PROVIDERS.contains(&provider.as_str()) {
        return Err(ApiError(StatusCode::NOT_FOUND, "Unknown connector."));
    }
    let actor = actor(&state, &headers).await?;
    manager(&actor)?;
    let mut tx = state.pool.begin().await.map_err(database_error)?;
    lock_restaurant(&mut tx, actor.restaurant_id).await?;
    if input.selected {
        select_connector(&mut tx, actor.restaurant_id, actor.user_id, &provider).await?;
    } else {
        deselect_connector(&mut tx, actor.restaurant_id, &provider).await?;
    }
    tx.commit().await.map_err(database_error)?;
    Ok(Json(load(&state, actor.restaurant_id).await?))
}

/// Providers that can own the menu+sales streams. Grows as connectors land.
pub(crate) const CONNECTOR_PROVIDERS: [&str; 2] = ["square", "clover"];

fn provider_label(provider: &str) -> &'static str {
    match provider {
        "clover" => "Clover",
        "square" => "Square",
        _ => "the connector",
    }
}

fn deselect_conflict(provider: &str) -> ApiError {
    ApiError(
        StatusCode::CONFLICT,
        match provider_label(provider) {
            "Clover" => "Deselect Clover first.",
            "Square" => "Deselect Square first.",
            _ => "Deselect the current connector first.",
        },
    )
}

/// The provider that currently owns both menu+sales streams, if any.
async fn active_connector(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
) -> Result<Option<String>, ApiError> {
    sqlx::query_scalar::<_, Option<String>>(
        "SELECT MIN(connector_provider) FROM restaurant_setup_streams
         WHERE restaurant_id=$1 AND stream IN ('menu','sales') AND method='connector'",
    )
    .bind(restaurant_id)
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)
}

pub(crate) async fn select_connector(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    user_id: Uuid,
    provider: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "DELETE FROM restaurant_setup_streams
         WHERE restaurant_id=$1 AND stream IN ('menu','sales')
           AND method='connector' AND connector_provider<>$2",
    )
    .bind(restaurant_id)
    .bind(provider)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    for stream in ["menu", "sales"] {
        sqlx::query(
            "INSERT INTO restaurant_setup_streams
             (restaurant_id,stream,method,owner,connector_provider,created_by,updated_by)
             VALUES($1,$2,'connector','restaurant',$3,$4,$4)
             ON CONFLICT (restaurant_id,stream) DO UPDATE SET
               method='connector',owner='restaurant',connector_provider=EXCLUDED.connector_provider,
               updated_by=EXCLUDED.updated_by,updated_at=NOW()",
        )
        .bind(restaurant_id)
        .bind(stream)
        .bind(provider)
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

pub(crate) async fn sync_legacy_sources(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    user_id: Uuid,
    pos_system: Option<&str>,
    setup_approach: Option<&str>,
) -> Result<(), ApiError> {
    lock_restaurant(tx, restaurant_id).await?;
    match pos_system {
        Some("Square") => return select_connector(tx, restaurant_id, user_id, "square").await,
        Some("Clover") => return select_connector(tx, restaurant_id, user_id, "clover").await,
        _ => {}
    }

    for provider in CONNECTOR_PROVIDERS {
        deselect_connector(tx, restaurant_id, provider).await?;
    }
    let Some(_) = pos_system else {
        sqlx::query(
            "DELETE FROM restaurant_setup_streams
             WHERE restaurant_id=$1 AND stream IN ('menu','sales')",
        )
        .bind(restaurant_id)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
        return Ok(());
    };

    let effective_approach = match setup_approach {
        Some(value) => Some(value.to_owned()),
        None => sqlx::query_scalar("SELECT setup_approach FROM restaurants WHERE id=$1")
            .bind(restaurant_id)
            .fetch_one(&mut **tx)
            .await
            .map_err(database_error)?,
    };
    let (method, owner) = if effective_approach.as_deref() == Some("assisted") {
        ("assisted", "parline")
    } else {
        ("import", "restaurant")
    };
    for stream in ["menu", "sales"] {
        sqlx::query(
            "INSERT INTO restaurant_setup_streams
             (restaurant_id,stream,method,owner,connector_provider,created_by,updated_by)
             VALUES($1,$2,$3,$4,NULL,$5,$5)
             ON CONFLICT (restaurant_id,stream) DO UPDATE SET
               method=EXCLUDED.method,owner=EXCLUDED.owner,connector_provider=NULL,
               updated_by=EXCLUDED.updated_by,updated_at=NOW()",
        )
        .bind(restaurant_id)
        .bind(stream)
        .bind(method)
        .bind(owner)
        .bind(user_id)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    }
    Ok(())
}

async fn deselect_connector(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    provider: &str,
) -> Result<(), ApiError> {
    let running = sqlx::query_scalar::<_, bool>(
        "SELECT EXISTS(SELECT 1 FROM source_sync_runs run
         JOIN source_connections connection
           ON connection.id=run.connection_id AND connection.restaurant_id=run.restaurant_id
         WHERE connection.restaurant_id=$1 AND connection.provider=$2
           AND run.status='running')",
    )
    .bind(restaurant_id)
    .bind(provider)
    .fetch_one(&mut **tx)
    .await
    .map_err(database_error)?;
    if running {
        return Err(ApiError(
            StatusCode::CONFLICT,
            "Wait for the sync to finish.",
        ));
    }
    sqlx::query(
        "DELETE FROM restaurant_setup_streams
         WHERE restaurant_id=$1 AND stream IN ('menu','sales')
           AND method='connector' AND connector_provider=$2",
    )
    .bind(restaurant_id)
    .bind(provider)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    cancel_queued(tx, restaurant_id, provider).await
}

async fn lock_restaurant(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
) -> Result<(), ApiError> {
    sqlx::query("SELECT id FROM restaurants WHERE id=$1 FOR UPDATE")
        .bind(restaurant_id)
        .execute(&mut **tx)
        .await
        .map_err(database_error)?;
    Ok(())
}

async fn cancel_queued(
    tx: &mut Transaction<'_, Postgres>,
    restaurant_id: Uuid,
    provider: &str,
) -> Result<(), ApiError> {
    sqlx::query(
        "UPDATE source_sync_runs run SET status='failed',
           error='The connector was removed from setup.',finished_at=NOW()
         FROM source_connections connection
         WHERE run.connection_id=connection.id
           AND run.restaurant_id=connection.restaurant_id
           AND connection.restaurant_id=$1 AND connection.provider=$2
           AND run.status='queued'",
    )
    .bind(restaurant_id)
    .bind(provider)
    .execute(&mut **tx)
    .await
    .map_err(database_error)?;
    Ok(())
}

async fn load(state: &AppState, restaurant_id: Uuid) -> Result<SetupResponse, ApiError> {
    let meta = sqlx::query_as::<_, SetupMeta>(
        "SELECT setup_approach,setup_assistance_requested_at,
                migration_setup_completed_at completed_at
         FROM restaurants WHERE id=$1",
    )
    .bind(restaurant_id)
    .fetch_one(&state.pool)
    .await
    .map_err(database_error)?;
    let selections = sqlx::query_as::<_, SelectionRow>(
        "SELECT stream,method,owner,connector_provider,updated_at
         FROM restaurant_setup_streams WHERE restaurant_id=$1",
    )
    .bind(restaurant_id)
    .fetch_all(&state.pool)
    .await
    .map_err(database_error)?;
    let evidence = sqlx::query_as::<_, EvidenceRow>(
        "SELECT
          (SELECT COUNT(*) FROM inventory_items WHERE restaurant_id=$1 AND active) inventory_count,
          (SELECT MAX(updated_at) FROM inventory_items WHERE restaurant_id=$1 AND active) inventory_latest_at,
          (SELECT COUNT(*) FROM inventory_import_rows row
           JOIN inventory_items item ON item.id=row.created_inventory_item_id
           WHERE row.restaurant_id=$1 AND row.selected=true AND item.active) focused_item_count,
          (SELECT COUNT(*) FROM inventory_import_rows row
           JOIN inventory_imports inventory_import ON inventory_import.id=row.import_id
           WHERE row.restaurant_id=$1 AND inventory_import.restaurant_id=$1 AND inventory_import.status='needs_review'
             AND row.selected IS DISTINCT FROM false
             AND (BTRIM(row.count_unit)='' OR CHAR_LENGTH(BTRIM(row.count_unit))>20)) unresolved_import_unit_count,
          (SELECT COUNT(*) FROM inventory_count_sessions WHERE restaurant_id=$1 AND status='completed') completed_count_count,
          (SELECT COUNT(*) FROM inventory_count_sessions session
           WHERE session.restaurant_id=$1 AND session.status='completed'
             AND EXISTS (SELECT 1 FROM inventory_count_entries entry
                         WHERE entry.session_id=session.id AND entry.quantity IS NOT NULL)) verified_count_count,
          (SELECT MAX(completed_at) FROM inventory_count_sessions WHERE restaurant_id=$1 AND status='completed') last_completed_count_at,
          (SELECT COUNT(*) FROM menu_items WHERE restaurant_id=$1 AND active) menu_count,
          (SELECT MAX(updated_at) FROM menu_items WHERE restaurant_id=$1 AND active) menu_latest_at,
          (SELECT COUNT(*) FROM sales_days WHERE restaurant_id=$1) sales_count,
          (SELECT MAX(business_date) FROM sales_days WHERE restaurant_id=$1) last_sales_date,
          (SELECT COUNT(*) FROM purchase_receipts WHERE restaurant_id=$1) purchase_count,
          (SELECT MAX(recorded_at) FROM purchase_receipts WHERE restaurant_id=$1) purchase_latest_at,
          (SELECT COUNT(*) FROM menu_imports WHERE restaurant_id=$1 AND status='processing') menu_pending_count,
          (SELECT COUNT(*) FROM menu_imports WHERE restaurant_id=$1 AND status='failed') menu_failed_count,
          (SELECT COUNT(*) FROM menu_imports WHERE restaurant_id=$1 AND status='needs_review') menu_review_count,
          (SELECT COUNT(*) FROM inventory_imports WHERE restaurant_id=$1 AND status='needs_review') inventory_review_count,
          (SELECT COUNT(*) FROM inventory_count_sessions WHERE restaurant_id=$1 AND status='draft') inventory_draft_count,
          (SELECT COUNT(*) FROM invoices WHERE restaurant_id=$1 AND status IN ('uploaded','processing')) purchase_pending_count,
          (SELECT COUNT(*) FROM invoices WHERE restaurant_id=$1 AND status='failed') purchase_failed_count,
          (SELECT COUNT(*) FROM source_sync_runs WHERE restaurant_id=$1 AND status IN ('queued','running')) source_sync_pending_count,
          (SELECT COUNT(*) FROM invoices invoice WHERE invoice.restaurant_id=$1
             AND (invoice.status='needs_review' OR (invoice.status='ready' AND NOT EXISTS (
               SELECT 1 FROM purchase_receipts receipt
               WHERE receipt.restaurant_id=invoice.restaurant_id AND receipt.invoice_id=invoice.id)))) purchase_review_count",
    )
    .bind(restaurant_id)
    .fetch_one(&state.pool)
    .await
    .map_err(database_error)?;
    let mut connections: std::collections::HashMap<&'static str, Option<ConnectorStatus>> =
        std::collections::HashMap::new();
    for provider in CONNECTOR_PROVIDERS {
        let row = sqlx::query_as::<_, ConnectorStatus>(
            "SELECT status,last_success_at,last_error,
                    menu_last_success_at,sales_last_success_at
             FROM source_connections
             WHERE restaurant_id=$1 AND provider=$2",
        )
        .bind(restaurant_id)
        .bind(provider)
        .fetch_optional(&state.pool)
        .await
        .map_err(database_error)?;
        connections.insert(provider, row);
    }
    let streams = STREAMS
        .into_iter()
        .map(|stream| build_stream(stream, &selections, &evidence, &connections, state))
        .collect();
    let activation_state = if evidence.verified_count_count > 0 {
        "active"
    } else if evidence.inventory_count > 0 {
        "ready_for_first_count"
    } else {
        "not_ready"
    };
    let first_count_handoff = if evidence.verified_count_count > 0 {
        FirstCountHandoff {
            focused_item_count: evidence.focused_item_count,
            unresolved_critical_import_unit_count: evidence.unresolved_import_unit_count,
            state: "completed",
            next_action: None,
        }
    } else if evidence.unresolved_import_unit_count > 0 {
        FirstCountHandoff {
            focused_item_count: evidence.focused_item_count,
            unresolved_critical_import_unit_count: evidence.unresolved_import_unit_count,
            state: "needs_unit_review",
            next_action: Some("review_import_units"),
        }
    } else if evidence.focused_item_count > 0 || evidence.inventory_count > 0 {
        FirstCountHandoff {
            focused_item_count: evidence.focused_item_count,
            unresolved_critical_import_unit_count: 0,
            state: "ready",
            next_action: Some("start_first_count"),
        }
    } else {
        FirstCountHandoff {
            focused_item_count: 0,
            unresolved_critical_import_unit_count: 0,
            state: "not_ready",
            next_action: Some("import_inventory"),
        }
    };
    Ok(SetupResponse {
        setup_approach: meta.setup_approach,
        assistance_requested_at: meta.setup_assistance_requested_at,
        setup_exited_at: meta.completed_at,
        activation_state,
        first_count_handoff,
        streams,
        connectors: CONNECTOR_PROVIDERS
            .iter()
            .map(|provider| ConnectorView {
                provider,
                supported: true,
                configured: server_configured(state, provider),
                selected: connector_selected(&selections, provider),
                capabilities: ["menu", "sales"],
                status: connections
                    .get(provider)
                    .and_then(|row| row.as_ref())
                    .map(|row| row.status.clone()),
            })
            .collect(),
    })
}

/// Whether this server has credentials for the given connector provider.
fn server_configured(state: &AppState, provider: &str) -> bool {
    match provider {
        "square" => state.square.is_some(),
        "clover" => state.clover.is_some(),
        _ => false,
    }
}

/// Both menu and sales streams are owned by this provider's connector.
fn connector_selected(selections: &[SelectionRow], provider: &str) -> bool {
    selections
        .iter()
        .filter(|row| {
            matches!(row.stream.as_str(), "menu" | "sales")
                && row.method == "connector"
                && row.connector_provider.as_deref() == Some(provider)
        })
        .count()
        == 2
}

fn build_stream(
    stream: &'static str,
    selections: &[SelectionRow],
    evidence: &EvidenceRow,
    connections: &std::collections::HashMap<&'static str, Option<ConnectorStatus>>,
    state: &AppState,
) -> SetupStream {
    let row = selections.iter().find(|row| row.stream == stream);
    // The connector owning this stream (connector method only) determines
    // whose connection status and sync outcomes the stream reports.
    let provider = row
        .filter(|row| row.method == "connector")
        .and_then(|row| row.connector_provider.as_deref());
    let connection: Option<&ConnectorStatus> = provider
        .and_then(|provider| connections.get(provider))
        .and_then(|row| row.as_ref());
    let data = stream_evidence(stream, evidence, connection);
    let (lifecycle, issue) = lifecycle(
        row,
        &data,
        connection,
        provider.is_some_and(|provider| server_configured(state, provider)),
    );
    let next_action = match lifecycle {
        "not_started" => Some("select_method"),
        "needs_attention" => Some("resolve_issue"),
        "in_progress" if row.is_some_and(|row| row.owner == "parline") => Some("wait_for_parline"),
        "in_progress" if row.is_some_and(|row| row.method == "connector") => {
            Some("connect_connector")
        }
        "in_progress" => Some("add_records"),
        _ => None,
    };
    SetupStream {
        stream,
        selection: row.map(|row| SelectionView {
            method: row.method.clone(),
            owner: row.owner.clone(),
            connector_provider: row.connector_provider.clone(),
            updated_at: row.updated_at,
        }),
        lifecycle,
        next_action,
        evidence: data,
        issue,
        supported_methods: supported_methods(stream),
    }
}

fn stream_evidence(
    stream: &str,
    evidence: &EvidenceRow,
    connection: Option<&ConnectorStatus>,
) -> EvidenceView {
    match stream {
        "menu" => EvidenceView {
            record_count: evidence.menu_count,
            latest_at: evidence.menu_latest_at,
            last_successful_sync_at: connection
                .and_then(|row| row.menu_last_success_at.or(row.last_success_at)),
            backlog: BacklogView {
                pending_count: evidence.menu_pending_count + evidence.source_sync_pending_count,
                failed_count: evidence.menu_failed_count,
                review_count: evidence.menu_review_count,
            },
            ..Default::default()
        },
        "sales" => EvidenceView {
            record_count: evidence.sales_count,
            latest_business_date: evidence.last_sales_date,
            last_successful_sync_at: connection
                .and_then(|row| row.sales_last_success_at.or(row.last_success_at)),
            backlog: BacklogView {
                pending_count: evidence.source_sync_pending_count,
                ..Default::default()
            },
            ..Default::default()
        },
        "inventory" => EvidenceView {
            record_count: evidence.inventory_count,
            latest_at: evidence
                .inventory_latest_at
                .max(evidence.last_completed_count_at),
            completed_count: evidence.completed_count_count,
            backlog: BacklogView {
                pending_count: evidence.inventory_draft_count,
                review_count: evidence.inventory_review_count,
                ..Default::default()
            },
            ..Default::default()
        },
        "purchases" => EvidenceView {
            record_count: evidence.purchase_count,
            latest_at: evidence.purchase_latest_at,
            backlog: BacklogView {
                pending_count: evidence.purchase_pending_count,
                failed_count: evidence.purchase_failed_count,
                review_count: evidence.purchase_review_count,
            },
            ..Default::default()
        },
        _ => EvidenceView::default(),
    }
}

fn lifecycle(
    selection: Option<&SelectionRow>,
    evidence: &EvidenceView,
    connection: Option<&ConnectorStatus>,
    configured: bool,
) -> (&'static str, Option<IssueView>) {
    let Some(selection) = selection else {
        return ("not_started", None);
    };
    if selection.method == "deferred" {
        return ("deferred", None);
    }
    if selection.method != "connector" {
        return (
            if evidence.record_count > 0 {
                "ready"
            } else {
                "in_progress"
            },
            None,
        );
    }
    let label = provider_label(selection.connector_provider.as_deref().unwrap_or(""));
    if !configured {
        return (
            "needs_attention",
            Some(IssueView {
                code: "connector_unavailable",
                message: format!("{label} is unavailable."),
            }),
        );
    }
    if evidence.backlog.pending_count > 0 {
        return ("in_progress", None);
    }
    match connection {
        Some(row) if row.status == "connected" && row.last_success_at.is_some() => ("ready", None),
        Some(row)
            if matches!(
                row.status.as_str(),
                "error" | "needs_reauth" | "disconnected"
            ) =>
        {
            (
                "needs_attention",
                Some(IssueView {
                    code: if row.status == "needs_reauth" {
                        "reauthorize"
                    } else {
                        "connection_error"
                    },
                    message: row
                        .last_error
                        .clone()
                        .unwrap_or_else(|| format!("{label} needs attention.")),
                }),
            )
        }
        // pending/importing/syncing and no-connection all mean work is owed.
        _ => ("in_progress", None),
    }
}

fn supported_methods(stream: &str) -> Vec<&'static str> {
    match stream {
        "menu" | "sales" => vec!["connector", "import", "manual", "assisted", "deferred"],
        "inventory" => vec!["import", "manual", "assisted", "deferred"],
        "purchases" => vec!["import", "assisted", "deferred"],
        _ => vec!["deferred"],
    }
}

fn validate_stream(stream: &str, mut input: UpdateStream) -> Result<UpdateStream, ApiError> {
    valid_stream(stream)?;
    input.method = input.method.trim().to_owned();
    input.owner = input.owner.trim().to_owned();
    input.connector_provider = input
        .connector_provider
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty());
    if input.method == "connector" {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Use connector selection.",
        ));
    }
    if input.connector_provider.is_some()
        || !supported_methods(stream).contains(&input.method.as_str())
        || (input.method == "assisted" && input.owner != "parline")
        || (input.method != "assisted" && input.owner != "restaurant")
    {
        return Err(ApiError(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid setup selection.",
        ));
    }
    Ok(input)
}

fn valid_stream(stream: &str) -> Result<(), ApiError> {
    if STREAMS.contains(&stream) {
        Ok(())
    } else {
        Err(ApiError(StatusCode::NOT_FOUND, "Setup stream not found."))
    }
}

async fn actor(state: &AppState, headers: &HeaderMap) -> Result<Actor, ApiError> {
    let subject = authenticated_subject(state, headers).await?;
    sqlx::query_as(
        "SELECT m.restaurant_id,u.id user_id,m.role
         FROM users u JOIN restaurant_memberships m ON m.user_id=u.id
         WHERE u.auth_subject=$1",
    )
    .bind(subject)
    .fetch_optional(&state.pool)
    .await
    .map_err(database_error)?
    .ok_or(ApiError(
        StatusCode::FORBIDDEN,
        "Restaurant access required.",
    ))
}

fn manager(actor: &Actor) -> Result<(), ApiError> {
    if matches!(actor.role.as_str(), "owner" | "manager") {
        Ok(())
    } else {
        Err(ApiError(
            StatusCode::FORBIDDEN,
            "Owner or manager access required.",
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validates_stream_combinations() {
        let selection = |method: &str, owner: &str| UpdateStream {
            method: method.into(),
            owner: owner.into(),
            connector_provider: None,
        };
        assert!(validate_stream("inventory", selection("manual", "restaurant")).is_ok());
        assert!(validate_stream("purchases", selection("manual", "restaurant")).is_err());
        assert!(validate_stream("menu", selection("assisted", "parline")).is_ok());
        assert!(validate_stream("menu", selection("assisted", "restaurant")).is_err());
        assert!(validate_stream("bookkeeping_export", selection("deferred", "restaurant")).is_ok());
        assert!(validate_stream("bookkeeping_export", selection("manual", "restaurant")).is_err());
        assert!(validate_stream("unknown", selection("manual", "restaurant")).is_err());
    }
}
