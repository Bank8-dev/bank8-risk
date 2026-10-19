// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! Axum server setup for Risk - shell boot composition.
//! See `services/risk/DESIGN.md` §2.

use crate::handlers::{self, AppState};
use axum::routing::{get, post};
use std::sync::Arc;

pub async fn build_state(core: sentin8::shell::ServiceCore) -> anyhow::Result<AppState> {
    let config = Arc::new(crate::config::RiskConfig::from_env());

    // ── Sentin8:Policy (§4.2b - shell-owned boot): consume the engine the
    // shell booted in step 8 - no second engine, consumer, or credential
    // registration here. Step 5 is the ONLY gate.
    let policy_engine = core.admin.policy.clone();

    // ── MemBus (principles §4.7 - membus → bus → API): the newest-wins
    // state-refresh tier. Fail-soft by contract.
    let membus = sentin8::membus::ServiceBus::connect("bank8:risk", &core.config.valkey_url).await;

    let state = AppState {
        config,
        pool: core.pool.clone(),
        policy: policy_engine.clone(),
        admin: sentin8::shield::admin::SharedAdminState {
            service: "bank8:risk".to_string(),
            pool: core.pool.clone(),
            policy: policy_engine,
            auth_client: Arc::new(core.config.sentin8_auth_client.clone()),
        },
        membus,
    };

    // Spawn the embedded measurement worker: consumes the `ledger` +
    // `payments` families and updates exposures/ECL/limits incrementally
    // (the WKR-MERGE embedded-worker precedent - no separate binary).
    // The worker's Bus client is the single outbound identity - the
    // `risk-worker` workload certificate (CERTFREE-4V - fail-closed, no
    // token fallback).
    crate::worker::spawn(&state).await;

    Ok(state)
}

/// Add this service's routes onto the shell-provided router. The shell
/// owns the boot sequence, state binding, OpenAPI merge and the standard
/// middleware stack; this adds ONLY routes (including the service's own
/// `/v1/health`).
pub fn routes(router: axum::Router<AppState>, state: AppState) -> axum::Router<AppState> {
    router
        .route("/v1/health", get(handlers::health))
        // Measured state (DESIGN.md §4)
        .route("/v1/exposures", get(handlers::list_exposures))
        .route(
            "/v1/limits",
            get(handlers::list_limits).post(handlers::upsert_limit),
        )
        .route("/v1/limits/{id}", get(handlers::get_limit))
        .route("/v1/stress/scenarios", post(handlers::create_scenario))
        .route("/v1/stress/run", post(handlers::run_stress))
        // ── Admin UI (RFW) + policy admin (§3.6 - shared implementation) ──
        .route("/v1/admin/ui/{*page}", get(crate::admin_ui::get_admin_ui))
        .route("/v1/admin/overview", get(crate::admin_ui::admin_overview))
        .route(
            "/v1/admin/policy-sets",
            get(sentin8::shield::admin::list_policy_sets::<AppState>),
        )
        .route(
            "/v1/admin/policy-sets/{name}",
            get(sentin8::shield::admin::get_policy_set::<AppState>)
                .put(sentin8::shield::admin::put_policy_set::<AppState>)
                .delete(sentin8::shield::admin::delete_policy_set::<AppState>),
        )
        .route(
            "/v1/admin/policy-sets/{name}/test",
            post(sentin8::shield::admin::test_policy_set::<AppState>),
        )
        .route(
            "/v1/admin/policy-sets/enforce",
            post(sentin8::shield::admin::admin_enforce_policy_set::<AppState>),
        )
        .route(
            "/v1/admin/policy-sets/shadow",
            post(sentin8::shield::admin::admin_shadow_policy_set::<AppState>),
        )
        .route(
            "/v1/admin/policy-sets/disable",
            post(sentin8::shield::admin::admin_disable_policy_set::<AppState>),
        )
        .route(
            "/v1/admin/relations",
            get(sentin8::shield::admin::list_relations::<AppState>)
                .put(sentin8::shield::admin::put_relation::<AppState>)
                .delete(sentin8::shield::admin::delete_relation::<AppState>),
        )
        .route(
            "/v1/admin/relations/check",
            get(sentin8::shield::admin::check_relation::<AppState>),
        )
        .route(
            "/v1/admin/attributes",
            get(sentin8::shield::admin::list_attributes::<AppState>)
                .put(sentin8::shield::admin::put_attribute::<AppState>)
                .delete(sentin8::shield::admin::delete_attribute::<AppState>),
        )
        .with_state(state)
}

/// Shared policy-admin surface accessor (§3.6) - the shared handlers
/// extract this service's admin state via [`HasPolicyAdmin`].
impl sentin8::shield::admin::HasPolicyAdmin for AppState {
    fn policy_admin(&self) -> &sentin8::shield::admin::SharedAdminState {
        &self.admin
    }
}
