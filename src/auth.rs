// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! Auth token verification for risk.
//! Step 5 is the ONLY gate (policy design §4.3): handlers verify the
//! token (steps 1-4) with [`verify_token`], then evaluate a
//! `bank8:risk:<resource>:<action>` Shield action via
//! `crate::policy::require`. No per-service scope check remains.

use axum::http::{HeaderMap, StatusCode};

use crate::config::RiskConfig;

pub use sentin8::auth::client::AuthContext;

/// Verify the Bearer token and return the auth context (steps 1-4:
/// identity + signature + expiry + tenant). NO scope check - step 5
/// (`crate::policy::require`) is the authorization decision.
pub async fn verify_token(
    config: &RiskConfig,
    headers: &HeaderMap,
) -> Result<AuthContext, (StatusCode, String)> {
    sentin8::auth::client::verify_bearer_token(&config.sentin8_auth_client, headers).await
}
