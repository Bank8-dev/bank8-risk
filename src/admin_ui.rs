// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! Bank8:Risk Admin UI handlers - RFW (Remote Flutter Widget)
//! definitions for the risk dashboard and limits admin pages. Icons are
//! Material Icons code points (hex strings), per principles §4.15.

use axum::{
    extract::{Path, State},
    http::HeaderMap,
    Json,
};
use sentin8::rfw::page::*;
use sentin8::rfw::response::AdminUiResponse;
use sentin8::rfw::widget::*;
use sqlx::Row;

use crate::auth;
use crate::handlers::{set_tenant_context, AppState};

/// GET /v1/admin/ui/{page} - return the RFW definition for the requested
/// admin page.
#[utoipa::path(
    get,
    path = "/v1/admin/ui/{page}",
    params(
        ("page" = String, Path, description = "RFW page name"),
        ("target_env_id" = String, Query, description = "Target env (RLS context)")
    ),
    responses(
        (status = 200, description = "RFW page definition", body = sentin8::rfw::AdminUiResponse),
        (status = 403, description = "Caller lacks the admin scope"),
    ),
    security(("bank8:admin" = [])),
    tag = "admin-ui",
)]
pub async fn get_admin_ui(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(page): Path<String>,
) -> Result<Json<AdminUiResponse>, (axum::http::StatusCode, String)> {
    let _a = auth::verify_token(&state.config, &headers).await?;
    crate::policy::require(
        &state.policy,
        &_a,
        &headers,
        sentin8::shield::EvalTarget::new("bank8:risk:limits:read"),
    )
    .await?;

    let page_def = match page.as_str() {
        "overview" => overview_page(),
        "limits" => limits_page(),
        "stress" => stress_page(),
        // ── Policy Studio (design §3.7) - shared page constructors; the
        // guard guarantees `Some` (§3.7 dispatch).
        p if sentin8::rfw::policy_page("bank8:risk", p).is_some() => {
            sentin8::rfw::policy_page("bank8:risk", p)
                .expect("policy_page checked in guard")
                .build()
        }
        _ => {
            return Err((
                axum::http::StatusCode::NOT_FOUND,
                format!("Unknown admin UI page: {page}"),
            ))
        }
    };

    Ok(Json(AdminUiResponse::page(page_def)))
}

fn overview_page() -> AdminUiPage {
    AdminPage::new("Risk Overview")
        .widget(RemoteWidget::Card(WidgetCard {
            title: Some("Risk Dashboard".into()),
            subtitle: None,
            children: vec![
                RemoteWidget::Text(WidgetText { text: "Measured exposures, capital, liquidity, and ECL stages updated incrementally from ledger and payment events - breaches emit the moment a limit is crossed.".into(), text_type: WidgetTextType::Body, align: TextAlign::Left, max_lines: None }),
            ],
            actions: vec![],
        }))
        .build()
}

fn limits_page() -> AdminUiPage {
    AdminPage::new("Limits")
        .widget(RemoteWidget::Card(WidgetCard {
            title: Some("Configured Limits".into()),
            subtitle: None,
            children: vec![
                RemoteWidget::Text(WidgetText { text: "Thresholds with live breach state; jurisdiction floors arrive as registry data, and a breach is emitted at most once per (limit, direction) transition.".into(), text_type: WidgetTextType::Body, align: TextAlign::Left, max_lines: None }),
            ],
            actions: vec![],
        }))
        .build()
}

fn stress_page() -> AdminUiPage {
    AdminPage::new("Stress Scenarios")
        .widget(RemoteWidget::Card(WidgetCard {
            title: Some("Scenario Runs".into()),
            subtitle: None,
            children: vec![
                RemoteWidget::Text(WidgetText { text: "Declarative assumption sets run on demand as a full recompute of the measured dimensions - never touching the incremental hot path; idempotent per (scenario, snapshot).".into(), text_type: WidgetTextType::Body, align: TextAlign::Left, max_lines: None }),
            ],
            actions: vec![],
        }))
        .build()
}

/// GET /v1/admin/overview - the data endpoint behind the overview page:
/// limit/breach counts for the tenant env.

/// Overview counters for the risk admin landing page (typed body).
#[derive(Debug, serde::Serialize, utoipa::ToSchema)]
pub struct AdminOverviewStats {
    /// Limits currently within their threshold.
    pub limits_ok: i64,
    /// Limits currently breached.
    pub limits_breached: i64,
    /// Open exposure records.
    pub exposures: i64,
    /// The service identifier.
    pub service: String,
}

#[utoipa::path(
    get,
    path = "/v1/admin/overview",
    params(("target_env_id" = String, Query, description = "Target env (RLS context)")),
    responses(
        (status = 200, description = "limit/breach counts", body = AdminOverviewStats),
        (status = 403, description = "Caller lacks the admin scope"),
    ),
    security(("bank8:admin" = [])),
    tag = "admin-ui",
)]
pub async fn admin_overview(
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Result<Json<AdminOverviewStats>, (axum::http::StatusCode, String)> {
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

    let limits_ok =
        sqlx::query("SELECT count(*) AS n FROM risk_limits WHERE env_id = $1 AND status = 'ok'")
            .bind(a.env_id())
            .fetch_one(&mut *conn)
            .await
            .map_err(crate::errors::err_db)?
            .get::<i64, _>("n");
    let limits_breached = sqlx::query(
        "SELECT count(*) AS n FROM risk_limits WHERE env_id = $1 AND status = 'breached'",
    )
    .bind(a.env_id())
    .fetch_one(&mut *conn)
    .await
    .map_err(crate::errors::err_db)?
    .get::<i64, _>("n");
    let exposures = sqlx::query("SELECT count(*) AS n FROM exposures WHERE env_id = $1")
        .bind(a.env_id())
        .fetch_one(&mut *conn)
        .await
        .map_err(crate::errors::err_db)?
        .get::<i64, _>("n");

    Ok(Json(AdminOverviewStats {
        limits_ok,
        limits_breached,
        exposures,
        service: "bank8:risk".into(),
    }))
}
