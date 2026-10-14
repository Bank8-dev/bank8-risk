// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! Service self-registration data for Bank8:Risk.
//! Output via `--print-registration` flag. See `sentin8/design/sentin8-service-self-registration-design.md`.

use sentin8::registration::{
    AdminScope, AdminUi, AdminUiPage, AuditFinding, ErrorCode, M2mGrant, PgRole, PolicySetDecl,
    Registration, RelationDecl, ServiceActionEntry, SqlMigration,
};
use utoipa::OpenApi;

const PG_ROLES: &[PgRole] = &[PgRole {
    scope: "bank8:risk:read,bank8:risk:write",
    role: "bank8_risk_service",
    bypass_rls: false,
    // Grants live in the bank8 vertical database only (DBX §6.6
    // database-per-vertical, schema-per-service).
    database: "bank8",
    // Risk owns all its reads — inputs arrive as events, never
    // cross-schema grants (risk DESIGN.md §8).
    cross_schema_grants: &[],
}];

/// Workload scope grants (self-registration design §3.11). One entry per
/// workload this service presents a certificate for.
/// - `bank8:risk` / `risk` — the service's own workload (server.rs): bus
///   produce/consume (breach events + worker consume).
/// - `risk-audit` — shell audit-journal pusher workload (audit crate
///   §6.8; boot step 8c).
/// - `risk-worker` — the embedded measurement worker (worker.rs): SSE
///   consume on the ledger/payments families.
const M2M_GRANTS: &[M2mGrant] = &[
    M2mGrant {
        workload: "bank8:risk",
        scopes: &[
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "sentin8:stat:measure",
        ],
    },
    // Shield hot-reload consumer (BUSPOLICY-1 — see bank8-reporting).
    M2mGrant {
        workload: "bank8:risk-policy",
        scopes: &["sentin8:bus:consume", "sentin8:stat:measure"],
    },
    M2mGrant {
        workload: "risk",
        scopes: &[
            "sentin8:bus:produce",
            "sentin8:bus:consume",
            "sentin8:stat:measure",
        ],
    },
    M2mGrant {
        workload: "risk-audit",
        scopes: &["sentin8:sys:audit:write"],
    },
    M2mGrant {
        workload: "risk-worker",
        scopes: &["sentin8:bus:consume"],
    },
];

const ADMIN_SCOPES: &[AdminScope] = &[
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:exposures:read",
    },
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:limits:read",
    },
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:limits:write",
    },
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:stress:read",
    },
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:stress:write",
    },
    // Policy-admin surface (policy design §3.6; principles §6.0b per-service scope).
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:policy:read",
    },
    AdminScope::Typed {
        role: "bank8:risk:admin",
        scope: "bank8:risk:policy:write",
    },
];

const ERROR_CODES: &[ErrorCode] = &[ErrorCode {
    enum_name: "RiskValidationFailed",
    error_code: "risk_validation_failed",
    status_code: 422,
}];

const ADMIN_UI_PAGES: &[AdminUiPage] = &[
    AdminUiPage {
        route: "overview",
        label: "Overview",
        ui_endpoint: "/v1/admin/ui/overview",
        data_endpoint: "/v1/admin/overview",
        icon: "0xe1b1",
    },
    AdminUiPage {
        route: "limits",
        label: "Limits",
        ui_endpoint: "/v1/admin/ui/limits",
        data_endpoint: "/v1/admin/overview",
        icon: "0xf04e6",
    },
    AdminUiPage {
        route: "stress",
        label: "Stress",
        ui_endpoint: "/v1/admin/ui/stress",
        data_endpoint: "/v1/admin/overview",
        icon: "0xe7f4",
    },
    // Sentin8:Policy admin surface (§3.6, post pattern).
    AdminUiPage {
        route: "policy-sets",
        label: "Policy Sets",
        ui_endpoint: "/v1/admin/ui/policy-sets",
        data_endpoint: "/v1/admin/policy-sets",
        icon: "0xe8b8",
    },
    AdminUiPage {
        route: "policy-decisions",
        label: "Policy Decisions",
        ui_endpoint: "/v1/admin/ui/policy-decisions",
        data_endpoint: "/v1/admin/policy-decisions",
        icon: "0xf070",
    },
    AdminUiPage {
        route: "relations",
        label: "Relations",
        ui_endpoint: "/v1/admin/ui/relations",
        data_endpoint: "/v1/admin/relations",
        icon: "0xe572",
    },
    AdminUiPage {
        route: "attributes",
        label: "Attributes",
        ui_endpoint: "/v1/admin/ui/attributes",
        data_endpoint: "/v1/admin/attributes",
        icon: "0xe8d3",
    },
];

/// Default declarative policy set (Sentin8:Policy design §3.2/§3.4,
/// seeded by `vertical enroll` as `origin='platform'`, `mode='enforced'`):
/// action-granular scope permits over the LIVE effective scope set — the
/// exact mirror of the gates this service's handlers evaluate in step 5
/// (policy design §4.3). Step 5 is the only gate.
pub const SCOPE_MIRROR_POLICIES: &str = concat!(
    "permit(principal, action == Action::\"bank8:risk:exposures:read\", resource) when { principal.scopes.contains(\"bank8:risk:exposures:read\") };\n",
    "permit(principal, action == Action::\"bank8:risk:exposures:read\", resource) when { principal.scopes.contains(\"bank8:risk:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:exposures:read\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:exposures:read\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:exposures:read\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:read\", resource) when { principal.scopes.contains(\"bank8:risk:limits:read\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:read\", resource) when { principal.scopes.contains(\"bank8:risk:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:read\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:read\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:read\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:write\", resource) when { principal.scopes.contains(\"bank8:risk:limits:write\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:write\", resource) when { principal.scopes.contains(\"bank8:risk:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:write\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:write\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:limits:write\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:read\", resource) when { principal.scopes.contains(\"bank8:risk:stress:read\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:read\", resource) when { principal.scopes.contains(\"bank8:risk:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:read\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:read\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:read\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:write\", resource) when { principal.scopes.contains(\"bank8:risk:stress:write\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:write\", resource) when { principal.scopes.contains(\"bank8:risk:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:write\", resource) when { principal.scopes.contains(\"sentin8:sys:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:write\", resource) when { principal.scopes.contains(\"sentin8:sys:org:admin\") };\n",
    "permit(principal, action == Action::\"bank8:risk:stress:write\", resource) when { principal.scopes.contains(\"sentin8:sys:env:admin\") };\n",
);

const POLICIES: &[PolicySetDecl] = &[PolicySetDecl {
    policy_set_name: "scope-mirror",
    description: "Action-granular scope permits for bank8:risk over the LIVE effective scope set (design §3.1d). Step 5 is the only gate.",
    // Shield text lives in SCOPE_MIRROR_POLICIES (single source).
    policies: SCOPE_MIRROR_POLICIES,
    schema: None,
    mode: "enforced",
    default_enabled: true,
}];

/// Default ReBAC relation tuples (design §3.1c) — none declared by this
/// service today; env admins author their own graph via the relations
/// admin API.
const RELATIONS: &[RelationDecl] = &[];

const SQL_1_INIT: &str = include_str!("../sql/init.sql");

const AUDIT_FINDINGS: &[AuditFinding] = &[
    AuditFinding {
        finding: "Stress scenarios authored as executable hooks instead of declarative data",
        recommendation: "Stress scenarios must remain declarative assumption sets (risk DESIGN.md §10): a Func-authored hook cannot be audited as a filing input.",
    },
];

const SERVICE_ACTIONS: &[ServiceActionEntry] = &[ServiceActionEntry {
    action: "risk_stress_run",
    service_type: "bank8:risk",
    platform_domain_key: "bank8:risk",
    method: "POST",
    endpoint: "/v1/stress/run",
    scope: "bank8:risk:stress:write",
}];

fn openapi_spec() -> serde_json::Value {
    serde_json::to_value(crate::openapi::RiskOpenApi::openapi()).unwrap_or(serde_json::Value::Null)
}

/// Print the registration JSON to stdout.
pub fn print(section: Option<String>) -> anyhow::Result<()> {
    let reg = Registration {
        service_type: "bank8:risk",
        subdomain: "bank8-risk",
        health_path: "/v1/health",
        admin_scopes: ADMIN_SCOPES,
        error_codes: ERROR_CODES,
        consent_providers: &[],
        consent_flows: &[],
        policies: POLICIES,
        relations: RELATIONS,
        event_triggers: &[],
        schedules: &[],
        admin_ui: Some(AdminUi {
            label: "Risk",
            icon: "0xf04e6",
            service_flag: "risk",
            pages: ADMIN_UI_PAGES,
        }),
        sql_migrations: vec![SqlMigration {
            version: 1,
            name: "init",
            sql: SQL_1_INIT,
        }],
        audit_findings: Some(AUDIT_FINDINGS),
        service_actions: Some(SERVICE_ACTIONS),
        m2m_grants: Some(M2M_GRANTS),
        pg_roles: PG_ROLES,
        openapi_url: "/openapi.json",
        openapi: openapi_spec,
    };
    sentin8::registration::print_registration(section, &reg, openapi_spec)
}
