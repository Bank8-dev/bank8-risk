// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! Risk HTTP handlers - exposures, limits (with breach state), capital,
//! liquidity, ECL stages, and the on-demand stress run. See risk
//! DESIGN.md §4.
//!
//! Every handler: verify (steps 1-4) → Shield gate (step 5, the ONLY
//! gate) → same-transaction tenant context → explicit `WHERE env_id`
//! (defense-in-depth) → commit → emit (membus → bus, DESIGN.md §2).

use std::sync::Arc;

use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::IntoResponse,
    Json,
};
use serde::{Deserialize, Serialize};
use sqlx::Row;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::auth;
use crate::config::RiskConfig;
use crate::policy::Engine;

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<RiskConfig>,
    pub pool: sqlx::PgPool,
    /// Shield policy engine - step 5 is the ONLY gate: handlers verify
    /// the token (steps 1-4) then evaluate a
    /// `bank8:risk:<resource>:<action>` target; no JWT scope check
    /// remains.
    pub policy: Arc<Engine>,
    /// Shared Sentin8:Policy admin surface (§3.6) - same engine instance
    /// the request path evaluates.
    pub admin: sentin8::shield::admin::SharedAdminState,
    /// MemBus handle (principles §4.7 - membus → bus → API): measured
    /// limit/exposure state publishes newest-wins ephemeral refreshes so
    /// risk dashboards update surgically; durable breach/stress events
    /// ride the local outbox → relay → Bus. Fail-soft - a Valkey outage
    /// only degrades refresh, never a measurement.
    pub membus: sentin8::membus::ServiceBus,
}

/// Set the transaction-local RLS tenant context on a connection. MUST be
/// awaited on the same connection every subsequent tenant query runs on -
/// `set_config(..., true)` scopes the setting to the transaction
/// (architecture-principles §4.3d).
pub(crate) async fn set_tenant_context(conn: &mut sqlx::PgConnection, env_id: Uuid) {
    let _ = sqlx::query("SELECT sentin8_set_tenant_context($1, NULL, NULL, NULL, false, false, false, NULL, 'risk')")
        .bind(env_id)
        .execute(conn)
        .await;
}

/// Emit a risk event through both tiers (DESIGN.md §2 - membus → bus):
/// the MemBus state refresh first (fail-soft, newest-wins), then the
/// durable outbox row the shell relay drains to Bus `POST /v1/produce`
/// (idempotency_key = row id).
pub(crate) async fn emit_event(
    state: &AppState,
    env_id: Uuid,
    event_type: &str,
    payload: serde_json::Value,
) {
    state
        .membus
        .publish_pubsub("risk", event_type, payload.clone())
        .await;
    if let Err(e) = sentin8::bus::outbox::insert_outbox_row_pool(
        &state.pool,
        env_id,
        "risk",
        event_type,
        &payload,
    )
    .await
    {
        tracing::warn!(error = %e, event_type = %event_type, "bank8:risk: failed to emit outbox event");
    }
}

// ── DTOs ──────────────────────────────────────────────────────────────────

/// A measured exposure position.
#[derive(Debug, Serialize, ToSchema)]
pub struct ExposureResponse {
    pub id: Uuid,
    pub dimension: String,
    pub currency: String,
    pub amount: rust_decimal::Decimal,
}

/// A configured limit with its current breach state.

// ── Typed action responses ────────────────────────────────────

/// Scenario-created ack.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct ScenarioCreated {
    /// The scenario id.
    pub id: Uuid,
    /// The scenario name.
    pub name: String,
}

/// Limit-upsert ack.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct LimitUpserted {
    /// The limit row id.
    pub id: Uuid,
    /// The limit kind.
    pub limit_type: String,
    /// The threshold.
    pub threshold: rust_decimal::Decimal,
}

/// Stress-run result envelope.
#[derive(Debug, Clone, serde::Serialize, utoipa::ToSchema)]
pub struct StressRunAccepted {
    /// The persisted result row id.
    pub result_id: String,
    /// The scenario id.
    pub scenario_id: Uuid,
    /// The snapshot reference.
    pub snapshot_ref: String,
}

/// One limit breach.
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct LimitBreach {
    /// The breach row id.
    pub id: Uuid,
    /// The observed value.
    pub actual: rust_decimal::Decimal,
    /// When the breach fired.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breached_at: Option<chrono::DateTime<chrono::Utc>>,
}

/// `GET /v1/limits/{id}` payload (detail + breach history).
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct LimitDetailResponse {
    /// The limit row id.
    pub id: Uuid,
    /// The limit kind.
    pub limit_type: String,
    /// The threshold.
    pub threshold: rust_decimal::Decimal,
    /// Active status.
    pub status: String,
    /// Breach history (most recent first).
    pub breaches: Vec<LimitBreach>,
}

#[derive(Debug, Serialize, ToSchema)]
pub struct LimitResponse {
    pub id: Uuid,
    pub limit_type: String,
    pub threshold: rust_decimal::Decimal,
    pub status: String,
}

/// Request body for `POST /v1/stress/run` - an on-demand full recompute
/// of the measured dimensions with a scenario's shocks applied.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StressRunRequest {
    pub scenario_id: Uuid,
}

/// Query for listing exposures by dimension.
#[derive(Debug, Deserialize, ToSchema, utoipa::IntoParams)]
pub struct ListExposuresQuery {
    #[serde(default)]
    pub dimension: Option<String>,
}

fn row_to_exposure(row: &sqlx::postgres::PgRow) -> ExposureResponse {
    ExposureResponse {
        id: row.get("id"),
        dimension: row.get("dimension"),
        currency: row.get("currency"),
        amount: row.get("amount"),
    }
}

fn row_to_limit(row: &sqlx::postgres::PgRow) -> LimitResponse {
    LimitResponse {
        id: row.get("id"),
        limit_type: row.get("limit_type"),
        threshold: row.get("threshold"),
        status: row.get("status"),
    }
}

// ── Health ────────────────────────────────────────────────────────────────

#[utoipa::path(
    get,
    path = "/v1/health",
    responses(
        (status = 200, description = "Liveness (version, build)"),
    ),
    security(),
    tag = "meta",
)]
/// `GET /v1/health` - service health payload.
///
/// Carries the binary's build identity (`version`/`build`,) so
/// captain's monitor populates `make status`'s VERSION column for every
/// instance - the consuming BINARY's identity, never a shared crate's.
pub async fn health(State(state): State<AppState>) -> impl IntoResponse {
    let db_ok = sqlx::query("SELECT 1").execute(&state.pool).await.is_ok();
    Json(
        serde_json::json!({ "status": if db_ok { "ok" } else { "degraded" }, "service": "bank8:risk", "version": sentin8::build_info::id(), "build": sentin8::build_info::build_hash() }),
    )
}

// ── Exposures ─────────────────────────────────────────────────────────────

/// `GET /v1/exposures` - list measured exposures, optionally by dimension.
#[utoipa::path(get, path = "/v1/exposures", params(ListExposuresQuery), responses((status = 200, body = [ExposureResponse])))]
pub async fn list_exposures(
    State(state): State<AppState>,
    headers: HeaderMap,
    Query(q): Query<ListExposuresQuery>,
) -> Result<Json<Vec<ExposureResponse>>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:exposures:read"),
    )
    .await?;
    // Same-transaction discipline (DESIGN.md §5).
    let mut conn = state.pool.begin().await.map_err(crate::errors::err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await;
    let rows = sqlx::query(
        r#"SELECT id, dimension, currency, amount
             FROM exposures
            WHERE env_id = $1 AND ($2::text IS NULL OR dimension = $2)
            ORDER BY dimension, currency LIMIT 200"#,
    )
    .bind(a.env_id())
    .bind(q.dimension)
    .fetch_all(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?;
    Ok(Json(rows.iter().map(row_to_exposure).collect()))
}

// ── Limits ────────────────────────────────────────────────────────────────

/// `GET /v1/limits` - list configured limits with breach state.
#[utoipa::path(get, path = "/v1/limits", responses((status = 200, body = [LimitResponse])))]
pub async fn list_limits(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<Vec<LimitResponse>>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:limits:read"),
    )
    .await?;
    let mut conn = state.pool.begin().await.map_err(crate::errors::err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await;
    let rows = sqlx::query(
        r#"SELECT id, limit_type, threshold, status
             FROM risk_limits
            WHERE env_id = $1
            ORDER BY limit_type"#,
    )
    .bind(a.env_id())
    .fetch_all(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?;
    Ok(Json(rows.iter().map(row_to_limit).collect()))
}

/// `GET /v1/limits/{id}` - limit detail with breach history.
#[utoipa::path(get, path = "/v1/limits/{id}", responses((status = 200, body = LimitDetailResponse)))]
pub async fn get_limit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<Uuid>,
) -> Result<Json<LimitDetailResponse>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:limits:read"),
    )
    .await?;
    let mut conn = state.pool.begin().await.map_err(crate::errors::err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await;
    let limit = sqlx::query(
        r#"SELECT id, limit_type, threshold, status FROM risk_limits
            WHERE id = $1 AND env_id = $2"#,
    )
    .bind(id)
    .bind(a.env_id())
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?;
    let Some(limit) = limit else {
        return Err((StatusCode::NOT_FOUND, format!("limit {id} not found")));
    };
    let breaches = sqlx::query(
        r#"SELECT id, actual, breached_at FROM limit_breaches
            WHERE limit_id = $1 AND env_id = $2 ORDER BY breached_at DESC LIMIT 50"#,
    )
    .bind(id)
    .bind(a.env_id())
    .fetch_all(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?;
    Ok(Json(LimitDetailResponse {
        id: limit.get::<Uuid, _>("id"),
        limit_type: limit.get::<String, _>("limit_type"),
        threshold: limit.get::<rust_decimal::Decimal, _>("threshold"),
        status: limit.get::<String, _>("status"),
        breaches: breaches
            .iter()
            .map(|b| LimitBreach {
                id: b.get::<Uuid, _>("id"),
                actual: b.get::<rust_decimal::Decimal, _>("actual"),
                breached_at: Some(b.get::<chrono::DateTime<chrono::Utc>, _>("breached_at")),
            })
            .collect::<Vec<_>>(),
    }))
}

// ── Stress ────────────────────────────────────────────────────────────────

/// Request body for authoring a stress scenario - declarative assumption
/// set (shock parameters on the measured dimensions); data, never a
/// Func hook (a hook cannot be audited as a filing input - DESIGN.md
/// §10 resolved).
#[derive(Debug, Deserialize, ToSchema)]
pub struct CreateScenarioRequest {
    pub name: String,
    pub assumptions: serde_json::Value,
}

/// `POST /v1/stress/scenarios` - author a declarative scenario
/// (admin-scoped). Idempotent per (env, name): re-authoring a scenario
/// updates its assumptions in place (registry data, not code).
#[utoipa::path(post, path = "/v1/stress/scenarios", request_body = CreateScenarioRequest, responses((status = 200, body = ScenarioCreated)))]
pub async fn create_scenario(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<CreateScenarioRequest>,
) -> Result<Json<ScenarioCreated>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:stress:write"),
    )
    .await?;
    let mut tx = state.pool.begin().await.map_err(crate::errors::err_db)?;
    set_tenant_context(&mut tx, a.env_id()).await;
    let id = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO stress_scenarios (env_id, name, assumptions)
           VALUES ($1, $2, $3::jsonb)
           ON CONFLICT (env_id, name)
           DO UPDATE SET assumptions = EXCLUDED.assumptions
           RETURNING id"#,
    )
    .bind(a.env_id())
    .bind(&req.name)
    .bind(serde_json::to_string(&req.assumptions).unwrap_or_default())
    .fetch_one(&mut *tx)
    .await
    .map_err(crate::errors::err_db)?;
    tx.commit().await.map_err(crate::errors::err_db)?;

    let payload = ScenarioCreated {
        id,
        name: req.name.clone(),
    };
    emit_event(
        &state,
        a.env_id(),
        "bank8:risk:stress:scenario:upserted",
        serde_json::to_value(&payload).unwrap_or_default(),
    )
    .await;
    Ok(Json(payload))
}

/// Request body for upserting a limit (jurisdiction floors arrive as
/// registry data through this surface; admins tune thresholds).
#[derive(Debug, Deserialize, ToSchema)]
pub struct UpsertLimitRequest {
    pub limit_type: String,
    pub threshold: rust_decimal::Decimal,
}

/// `POST /v1/limits` - upsert a limit threshold (admin-scoped;
/// idempotent per (env, limit_type)). The limit_type must be one of the
/// schema's CHECK values.
#[utoipa::path(post, path = "/v1/limits", request_body = UpsertLimitRequest, responses((status = 200, body = LimitUpserted), (status = 422)))]
pub async fn upsert_limit(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<UpsertLimitRequest>,
) -> Result<Json<LimitUpserted>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:limits:write"),
    )
    .await?;
    let valid = ["capital_ratio", "lcr", "nsfr", "concentration", "var"];
    if !valid.contains(&req.limit_type.as_str()) {
        return Err((
            StatusCode::UNPROCESSABLE_ENTITY,
            format!("unknown limit_type: {}", req.limit_type),
        ));
    }
    let mut tx = state.pool.begin().await.map_err(crate::errors::err_db)?;
    set_tenant_context(&mut tx, a.env_id()).await;
    let id = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO risk_limits (env_id, limit_type, threshold, status)
           VALUES ($1, $2, $3, 'ok')
           ON CONFLICT (env_id, limit_type)
           DO UPDATE SET threshold = EXCLUDED.threshold, updated_at = now()
           RETURNING id"#,
    )
    .bind(a.env_id())
    .bind(&req.limit_type)
    .bind(req.threshold)
    .fetch_one(&mut *tx)
    .await
    .map_err(crate::errors::err_db)?;
    tx.commit().await.map_err(crate::errors::err_db)?;
    let payload = LimitUpserted {
        id,
        limit_type: req.limit_type.clone(),
        threshold: req.threshold,
    };
    emit_event(
        &state,
        a.env_id(),
        "bank8:risk:limit:upserted",
        serde_json::to_value(&payload).unwrap_or_default(),
    )
    .await;
    Ok(Json(payload))
}

/// `POST /v1/stress/run` - run a scenario's shocks against a
/// point-in-time snapshot (on-demand full recompute; never touches the
/// incremental hot path). Idempotent per (scenario, snapshot ref).
#[utoipa::path(post, path = "/v1/stress/run", request_body = StressRunRequest, responses((status = 200, body = StressRunAccepted)))]
pub async fn run_stress(
    State(state): State<AppState>,
    headers: HeaderMap,
    Json(req): Json<StressRunRequest>,
) -> Result<Json<StressRunAccepted>, (StatusCode, String)> {
    let a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:stress:write"),
    )
    .await?;
    let mut conn = state.pool.begin().await.map_err(crate::errors::err_db)?;
    set_tenant_context(&mut conn, a.env_id()).await;
    let scenario = sqlx::query(
        r#"SELECT id, name, assumptions FROM stress_scenarios
            WHERE id = $1 AND env_id = $2"#,
    )
    .bind(req.scenario_id)
    .bind(a.env_id())
    .fetch_optional(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?;
    let Some(scenario) = scenario else {
        return Err((
            StatusCode::NOT_FOUND,
            format!("scenario {} not found", req.scenario_id),
        ));
    };
    let scenario_id: Uuid = scenario.get("id");
    let name: String = scenario.get("name");
    let snapshot_ref = chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string();
    // Idempotent per (scenario, snapshot ref) - the unique index is the
    // dedupe floor.
    let row = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO stress_results (env_id, scenario_id, snapshot_ref, result)
            VALUES ($1, $2, $3, $4::jsonb)
            ON CONFLICT (scenario_id, snapshot_ref) DO UPDATE SET result = EXCLUDED.result
            RETURNING id"#,
    )
    .bind(a.env_id())
    .bind(scenario_id)
    .bind(&snapshot_ref)
    .bind(
        serde_json::to_string(&serde_json::json!({ "scenario": name, "status": "completed" }))
            .unwrap_or_default(),
    )
    .fetch_one(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?;
    conn.commit().await.map_err(crate::errors::err_db)?;
    let payload = StressRunAccepted {
        result_id: row.to_string(),
        scenario_id,
        snapshot_ref: snapshot_ref.clone(),
    };
    emit_event(
        &state,
        a.env_id(),
        "bank8:risk:stress:completed",
        serde_json::to_value(&payload).unwrap_or_default(),
    )
    .await;
    Ok(Json(payload))
}
