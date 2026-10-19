-- SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
-- SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
-- SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
-- SPDX-License-Identifier: Apache-2.0

-- Bank8:Risk - the service's own schema `risk` (schema-per-service):
-- risk_* domain tables + risk.outbox / risk.audit_journal producer
-- tables, all FORCE-RLS env-scoped, per services/risk/DESIGN.md §3.
-- No FKs or joins into ledger.* / payments.* - inputs arrive as events
-- or public-API reads (DESIGN.md §8 invariant).

CREATE SCHEMA IF NOT EXISTS risk;

-- Configured limits with breach state. A breach is emitted at most once
-- per (limit, direction) transition (DESIGN.md §8).
CREATE TABLE IF NOT EXISTS risk.risk_limits (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    limit_type  TEXT NOT NULL CHECK (limit_type IN ('capital_ratio', 'lcr', 'nsfr', 'concentration', 'var')),
    threshold   NUMERIC NOT NULL,
    status      TEXT NOT NULL DEFAULT 'ok' CHECK (status IN ('ok', 'breached')),
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, limit_type)
);

-- Measured positions - the incremental-UPSERT hot path keyed by
-- dimension (DESIGN.md §7): a posting/instruction event updates the
-- single matching row, never a full recompute.
CREATE TABLE IF NOT EXISTS risk.exposures (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    dimension   TEXT NOT NULL,
    currency    TEXT NOT NULL,
    amount      NUMERIC NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, dimension, currency)
);

CREATE INDEX IF NOT EXISTS idx_risk_exposures_env
    ON risk.exposures (env_id);

-- Breach history - at most one row per (limit, direction) transition;
-- supports the dashboard's breach-history view and the filing inputs.
CREATE TABLE IF NOT EXISTS risk.limit_breaches (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    limit_id    UUID NOT NULL REFERENCES risk.risk_limits(id),
    exposure_id UUID REFERENCES risk.exposures(id),
    actual      NUMERIC NOT NULL,
    breached_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX IF NOT EXISTS idx_risk_breaches_limit
    ON risk.limit_breaches (limit_id, breached_at);

-- Basel capital components (tier, amount) - aggregated per recompute.
CREATE TABLE IF NOT EXISTS risk.capital_components (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    tier        TEXT NOT NULL CHECK (tier IN ('cet1', 'tier1', 'tier2', 'total')),
    amount      NUMERIC NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, tier)
);

-- Liquidity buffers (asset class, amount) feeding LCR/NSFR.
CREATE TABLE IF NOT EXISTS risk.liquidity_buffers (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    asset_class TEXT NOT NULL,
    amount      NUMERIC NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, asset_class)
);

-- IFRS 9 ECL staging (12-month vs lifetime) per customer exposure.
CREATE TABLE IF NOT EXISTS risk.ecl_models (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    customer_id UUID NOT NULL,
    stage       TEXT NOT NULL CHECK (stage IN ('stage1', 'stage2', 'stage3')),
    -- 12-month ECL (stage1) or lifetime ECL (stages 2-3).
    ecl         NUMERIC NOT NULL DEFAULT 0,
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, customer_id)
);

-- Declarative stress scenarios - assumption sets as data, never
-- Func-authored hooks (DESIGN.md §10 resolved: a hook cannot be audited
-- as a filing input).
CREATE TABLE IF NOT EXISTS risk.stress_scenarios (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id      UUID NOT NULL,
    name        TEXT NOT NULL,
    -- Shock parameters applied to the measured dimensions.
    assumptions JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (env_id, name)
);

-- Scenario run outcomes for a point-in-time snapshot, idempotent per
-- (scenario, snapshot ref).
CREATE TABLE IF NOT EXISTS risk.stress_results (
    id           UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id       UUID NOT NULL,
    scenario_id  UUID NOT NULL REFERENCES risk.stress_scenarios(id),
    snapshot_ref TEXT NOT NULL,
    result       JSONB NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (scenario_id, snapshot_ref)
);

-- ── Env-scoped RLS (every table) ─────────────────────────────────────────
ALTER TABLE risk.risk_limits ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.risk_limits FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS limits_dml ON risk.risk_limits;
CREATE POLICY limits_dml ON risk.risk_limits
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.exposures ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.exposures FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS exposures_dml ON risk.exposures;
CREATE POLICY exposures_dml ON risk.exposures
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.limit_breaches ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.limit_breaches FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS breaches_dml ON risk.limit_breaches;
CREATE POLICY breaches_dml ON risk.limit_breaches
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.capital_components ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.capital_components FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS capital_dml ON risk.capital_components;
CREATE POLICY capital_dml ON risk.capital_components
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.liquidity_buffers ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.liquidity_buffers FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS liquidity_dml ON risk.liquidity_buffers;
CREATE POLICY liquidity_dml ON risk.liquidity_buffers
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.ecl_models ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.ecl_models FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS ecl_dml ON risk.ecl_models;
CREATE POLICY ecl_dml ON risk.ecl_models
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.stress_scenarios ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.stress_scenarios FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS scenarios_dml ON risk.stress_scenarios;
CREATE POLICY scenarios_dml ON risk.stress_scenarios
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

ALTER TABLE risk.stress_results ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.stress_results FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS stress_results_dml ON risk.stress_results;
CREATE POLICY stress_results_dml ON risk.stress_results
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

-- ── Local outbox (DBX, architecture-principles §6.7) ─────────────────────
-- Producer path: risk rows are appended to the outbox in the SAME
-- transaction as the domain change; the shell's post-commit relay pushes
-- them to Bus POST /v1/produce (idempotency_key = row id). Generated
-- from sentin8-outbox::outbox_table_sql("risk").
CREATE TABLE IF NOT EXISTS risk.outbox (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    env_id          UUID NOT NULL,
    event_type      TEXT NOT NULL,
    stream_family   TEXT NOT NULL,
    payload         JSONB NOT NULL,
    publisher_id_token  TEXT,
    publisher_subject   TEXT,
    relayed         BOOLEAN NOT NULL DEFAULT false,
    relayed_at      TIMESTAMPTZ,
    relay_attempts  INT NOT NULL DEFAULT 0,
    next_retry_at   TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_risk_outbox_unrelayed
    ON risk.outbox (created_at) WHERE relayed = false;
ALTER TABLE risk.outbox ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.outbox FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS outbox_select ON risk.outbox;
CREATE POLICY outbox_select ON risk.outbox FOR SELECT
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS outbox_insert ON risk.outbox;
CREATE POLICY outbox_insert ON risk.outbox FOR INSERT
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
DROP POLICY IF EXISTS outbox_update ON risk.outbox;
CREATE POLICY outbox_update ON risk.outbox FOR UPDATE
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id());
CREATE OR REPLACE FUNCTION risk.risk_notify_outbox() RETURNS TRIGGER
LANGUAGE plpgsql AS $fn$
BEGIN
    PERFORM pg_notify('risk_outbox_changed', '');
    RETURN NEW;
END;
$fn$;
DROP TRIGGER IF EXISTS risk_outbox_notify_trigger ON risk.outbox;
CREATE TRIGGER risk_outbox_notify_trigger
    AFTER INSERT ON risk.outbox
    FOR EACH ROW
    EXECUTE FUNCTION risk.risk_notify_outbox();

-- ── Local audit journal (DBX §6.8) ────────────────────────────────────────
-- Spill journal for the canonical audit push: rows written same-tx as the
-- domain change, pushed to Sys POST /v1/audit (sentin8:sys:audit:write),
-- then marked pushed. A Sys outage never drops audit.
-- Generated from sentin8-audit::audit_journal_sql("risk").
CREATE TABLE IF NOT EXISTS risk.audit_journal (
    id              UUID PRIMARY KEY,
    env_id          UUID NOT NULL,
    action          TEXT NOT NULL,
    target_resource_type TEXT,
    target_resource_id    TEXT,
    actor_user_id   TEXT,
    actor_subject_type TEXT,
    severity        TEXT NOT NULL DEFAULT 'info',
    correlation_id  TEXT,
    before_state    JSONB,
    after_state     JSONB,
    details         JSONB,
    pushed          BOOLEAN NOT NULL DEFAULT false,
    pushed_at       TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX IF NOT EXISTS idx_risk_audit_journal_unpushed
    ON risk.audit_journal (created_at) WHERE pushed = false;
ALTER TABLE risk.audit_journal ENABLE ROW LEVEL SECURITY;
ALTER TABLE risk.audit_journal FORCE ROW LEVEL SECURITY;
DROP POLICY IF EXISTS audit_journal_dml ON risk.audit_journal;
CREATE POLICY audit_journal_dml ON risk.audit_journal
    USING (sentin8_context_initialized() AND env_id = sentin8_current_env_id())
    WITH CHECK (sentin8_context_initialized() AND env_id = sentin8_current_env_id());

-- ── Per-service role grants + search_path (F1 - §4.3e) ──────────────────
-- The risk service connects via its own PostgreSQL role
-- (bank8_risk_service, created + granted by `sentin8-cli vertical
-- init/enroll` from this service's --print-registration
-- --section=pg_roles). Grants scoped to the OWN schema only - no
-- blanket grants, no cross-schema grants (DESIGN.md §8: inputs are
-- events or public-API reads, never ledger/payments tables).
DO $$
BEGIN
    IF EXISTS (SELECT 1 FROM pg_roles WHERE rolname = 'bank8_risk_service') THEN
        ALTER ROLE bank8_risk_service SET search_path = risk, public;
        GRANT USAGE ON SCHEMA risk TO bank8_risk_service;
        GRANT SELECT, INSERT, UPDATE, DELETE ON ALL TABLES IN SCHEMA risk TO bank8_risk_service;
        GRANT USAGE, SELECT ON ALL SEQUENCES IN SCHEMA risk TO bank8_risk_service;
    END IF;
END $$;