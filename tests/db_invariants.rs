// SPDX-FileCopyrightText: Copyright Logically Magic Inc. <hello@logicallymagic.com>
// SPDX-FileCopyrightText: Copyright Hamid Jahanzad <Hamid@Jahanzad.com>
// SPDX-FileCopyrightText: Copyright Contributors to the Bank8:Risk
// SPDX-License-Identifier: Apache-2.0

//! DB-backed schema-invariant tests (runtime-skip) — risk DESIGN.md §9.
//! Exercises: FORCE-RLS rejects context-less inserts; the exposure UPSERT
//! hot path converges (bounded single-row update); the (limit, direction)
//! breach at-most-once invariant via guarded transitions; stress runs are
//! idempotent per (scenario, snapshot ref); no cross-schema tables exist
//! (§8 — inputs are events, never ledger/payments tables). Skipped when
//! `DATABASE_URL` is unreachable.

use sqlx::{PgPool, Row};
use uuid::Uuid;

async fn test_pool() -> Option<PgPool> {
    let url = std::env::var("BANK8_RISK_DATABASE_URL")
        .or_else(|_| std::env::var("DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://bank8_risk_service@localhost:5432/bank8".into());
    match sqlx::PgPool::connect(&url).await {
        Ok(p) => Some(p),
        Err(e) => {
            eprintln!("skipping DB-backed risk tests — no database: {e}");
            None
        }
    }
}

async fn with_context<'c>(pool: &'c PgPool, env_id: Uuid) -> sqlx::Transaction<'c, sqlx::Postgres> {
    let mut tx = pool.begin().await.expect("tx");
    // The context-set and every dependent insert share ONE transaction
    // (the ledger db_handlers discipline — set_config true is
    // transaction-local).
    let _ = sqlx::query("SELECT sentin8_set_tenant_context($1, '', 'm2m', 'sys:admin')")
        .bind(env_id)
        .execute(&mut *tx)
        .await;
    tx
}

/// FORCE-RLS: a context-less exposure insert is rejected.
#[tokio::test]
async fn exposure_insert_without_context_rejected() {
    let Some(pool) = test_pool().await else { return; };
    let res = sqlx::query(
        r#"INSERT INTO risk.exposures (env_id, dimension, currency, amount)
           VALUES ($1, 'test-dim', 'USD', 100)"#,
    )
    .bind(Uuid::new_v4())
    .execute(&pool).await;
    assert!(res.is_err(), "context-less insert must violate FORCE RLS");
}

/// The exposure UPSERT (env, dimension, currency) converges: two updates
/// with the same key are one row (the bounded hot path §7).
#[tokio::test]
async fn exposure_upsert_converges() {
    let Some(pool) = test_pool().await else { return; };
    let env_id = Uuid::new_v4();
    let mut tx = with_context(&pool, env_id).await;
    let sql = r#"INSERT INTO exposures (env_id, dimension, currency, amount)
                 VALUES ($1, 'counterparty', 'USD', $2)
                 ON CONFLICT (env_id, dimension, currency)
                 DO UPDATE SET amount = exposures.amount + EXCLUDED.amount"#;
    sqlx::query(sql).bind(env_id).bind(100i64).execute(&mut *tx).await.expect("first");
    sqlx::query(sql).bind(env_id).bind(50i64).execute(&mut *tx).await.expect("second");
    let rows = sqlx::query("SELECT amount FROM exposures WHERE env_id = $1 AND dimension = 'counterparty'")
        .bind(env_id).fetch_all(&mut *tx).await.expect("read");
    tx.commit().await.expect("commit");
    assert_eq!(rows.len(), 1, "one row per (env, dimension, currency)");
    assert_eq!(rows[0].get::<rust_decimal::Decimal, _>("amount"), rust_decimal::Decimal::from(150));
}

/// The at-most-once breach invariant: the guarded
/// `ok → breached` transition fires once; the replay is a no-op.
#[tokio::test]
async fn breach_transition_is_at_most_once() {
    let Some(pool) = test_pool().await else { return; };
    let env_id = Uuid::new_v4();
    let mut tx = with_context(&pool, env_id).await;
    let limit_id = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO risk_limits (env_id, limit_type, threshold, status)
           VALUES ($1, 'lcr', 100, 'ok') RETURNING id"#,
    ).bind(env_id).fetch_one(&mut *tx).await.expect("limit");

    let sql = r#"UPDATE risk_limits SET status = 'breached'
                 WHERE env_id = $1 AND id = $2 AND status = 'ok'
                 RETURNING id"#;
    let first = sqlx::query(sql).bind(env_id).bind(limit_id).fetch_optional(&mut *tx).await.expect("first");
    let replay = sqlx::query(sql).bind(env_id).bind(limit_id).fetch_optional(&mut *tx).await.expect("replay");
    tx.commit().await.expect("commit");
    assert!(first.is_some(), "the ok→breached transition fires once");
    assert!(replay.is_none(), "the replay cannot double-emit the breach");
}

/// Stress runs are idempotent per (scenario, snapshot ref).
#[tokio::test]
async fn stress_run_is_idempotent_per_snapshot() {
    let Some(pool) = test_pool().await else { return; };
    let env_id = Uuid::new_v4();
    let mut tx = with_context(&pool, env_id).await;
    let sid = sqlx::query_scalar::<_, Uuid>(
        r#"INSERT INTO stress_scenarios (env_id, name, assumptions)
           VALUES ($1, 'test-scenario', '{"lcr": "-10"}') RETURNING id"#,
    ).bind(env_id).fetch_one(&mut *tx).await.expect("scenario");

    let sql = r#"INSERT INTO stress_results (env_id, scenario_id, snapshot_ref, result)
                 VALUES ($1, $2, 'snap-1', '{}')
                 ON CONFLICT (scenario_id, snapshot_ref) DO UPDATE SET result = EXCLUDED.result
                 RETURNING id"#;
    let first = sqlx::query_scalar::<_, Uuid>(sql).bind(env_id).bind(sid).fetch_one(&mut *tx).await.expect("first");
    let second = sqlx::query_scalar::<_, Uuid>(sql).bind(env_id).bind(sid).fetch_one(&mut *tx).await.expect("second");
    tx.commit().await.expect("commit");
    assert_eq!(first, second, "same (scenario, snapshot) converges to one row");
}

/// Cross-schema isolation: the risk role/schema must contain NO tables
/// derived from ledger/payments — inputs are events, never joins (§8).
#[tokio::test]
async fn no_cross_schema_dependence() {
    let Some(pool) = test_pool().await else { return; };
    let bad = sqlx::query_scalar::<_, i64>(
        r#"SELECT count(*) FROM information_schema.views
            WHERE table_schema = 'risk'
              AND (view_definition ILIKE '%ledger.%' OR view_definition ILIKE '%payments.%')"#,
    )
    .fetch_one(&pool).await.expect("views check");
    assert_eq!(bad, 0, "no risk view may reference ledger/payments schemas");
}