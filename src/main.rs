// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! Sentin8 Risk - prudential risk measurement: capital, liquidity,
//! credit (IFRS 9 ECL), limits, stress. See `services/risk/DESIGN.md`.
//! Boot runs on the platform `sentin8-shell` crate: registration flag, PQ
//! provider, config, capped pool, PG18 check, state binding, OpenAPI
//! merge, middleware stack and PQ-TLS listener are owned by the shell.
//! This binary supplies only its registration data, state and routes.

#[allow(dead_code)]
mod admin_ui;
mod auth;
#[allow(dead_code)]
mod config;
#[allow(dead_code)]
mod errors;
mod handlers;
mod openapi;

// The utoipa dump as a fn pointer for registration.rs (which cannot
// name crate::openapi - embedded test harnesses have no openapi module;
// see registration.rs and the mirrored static in lib.rs).
static OPENAPI_DUMP: fn() -> serde_json::Value = || {
    use utoipa::OpenApi as _;
    serde_json::to_value(openapi::RiskOpenApi::openapi()).unwrap_or(serde_json::Value::Null)
};

pub mod policy;
mod registration;
mod server;
mod worker;

use utoipa::OpenApi;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    sentin8::shell::serve(
        sentin8::shell::Sentin8Shell::new("bank8:risk", "BANK8_RISK")
            .with_registration(registration::print)
            .with_state(server::build_state)
            .with_own_health()
            .with_router(server::routes)
            .with_openapi(openapi::RiskOpenApi::openapi()),
    )
    .await
}
