# Bank8:Risk - Detailed Design Document

## 1. Scope & Context

Bank8:Risk owns: the `risk` schema (schema-per-service - domain tables +
`risk.outbox` / `risk.audit_journal` producer tables), the risk
measurement REST API, the event-driven measurement worker, the limit
monitor, the IFRS 9 ECL engine, and the `bank8:risk:*` event family.

It does NOT own: money movement or balances (Bank8:Ledger), payment
state (Bank8:Payments), filing (Bank8:Reporting), tenant-authored hook
execution (Sentin8:Func - Risk consumes the *results*), or metric
aggregation (Sentin8:Stat).

```mermaid
graph TD
    subgraph platform["Sentin8 platform"]
        AUTH["Sentin8:Auth"]
        BUS["Sentin8:Bus"]
        FUNC["Sentin8:Func"]
        STAT["Sentin8:Stat"]
    end
    subgraph vertical["bank8 vertical"]
        RSK["Bank8:Risk<br/>(API + embedded measurement worker)"]
        LEDGER["Bank8:Ledger"]
        PAY["Bank8:Payments"]
        RPT["Bank8:Reporting"]
        PG[("bank8 database")]
    end
    LEDGER -->|"posting:created"| BUS
    PAY -->|"instruction:completed"| BUS
    BUS -->|"SSE consume"| RSK
    RSK -->|"limit:breached"| BUS
    RSK -->|"public API exposures"| RPT
    FUNC -->|"hook results"| RSK
    RSK -->|"telemetry"| STAT
    RSK <-->|"schema risk"| PG
```

## 2. Runtime Architecture

Shell-owned boot plus one service-spawned task: the embedded
measurement worker (a Bus SSE consumer on the `ledger`/`payments`
families that updates exposures, ECL staging, and limit state
incrementally, then emits breaches on the `risk` family).

Access-decision order (principles §4.7 - **membus → bus → owner
API**): (1) MemBus state refresh first - measured limit/exposure state
publishes newest-wins ephemeral notifications (`sentin8::membus::ServiceBus`,
fail-soft) so risk dashboards refresh surgically; (2) durable, audited
`bank8:risk:limit:breached` / `stress:completed` events via the local
outbox → relay → Bus `POST /v1/produce` - trigger delivery to
Compliance/Reporting rides this tier; (3) owner HTTP APIs only for
on-demand full recomputes (filing inputs, stress runs) that need a
synchronous aggregate. The measurement inputs themselves are Bus
events (`posting:created`, `instruction:completed`) carrying the full
domain snapshot - never cross-service table reads.

```mermaid
graph TD
    subgraph L["API layer"]
        RT["server.rs router"]
        MW["auth.rs verify + policy.rs require"]
    end
    subgraph D["Domain"]
        EX["exposures"]
        CAP["capital / liquidity"]
        ECL["IFRS 9 ECL"]
        LIM["limits / breach"]
        STR["stress scenarios"]
    end
    subgraph W["Worker"]
        WKR["worker/mod.rs<br/>(SSE consume + incremental update)"]
    end
    RT --> MW --> EX & CAP & ECL & LIM & STR
    WKR --> EX & ECL & LIM
    EX & CAP & ECL & LIM & STR --> PG[("schema risk")]
```

## 3. Data Model

```mermaid
erDiagram
    RISK_LIMIT ||--o{ LIMIT_BREACH : "crossed by"
    EXPOSURE ||--o{ LIMIT_BREACH : "triggers"
    CAPITAL_COMPONENT ||--o{ CAPITAL_RATIO : "feeds"
    LIQUIDITY_BUFFER ||--o{ LCR_NSFR : "feeds"
    CREDIT_STAGE ||--o{ ECL_MODEL : "drives"
    STRESS_SCENARIO ||--o{ STRESS_RESULT : "produces"

    RISK_LIMIT {
        uuid id PK
        text env_id
        text limit_type
        numeric threshold
        text status
    }
    EXPOSURE {
        uuid id PK
        text env_id
        text dimension
        numeric amount
        text currency
    }
    LIMIT_BREACH {
        uuid id PK
        uuid limit_id FK
        uuid exposure_id FK
        numeric actual
        timestamp breached_at
    }
    CAPITAL_COMPONENT {
        uuid id PK
        text env_id
        text tier
        numeric amount
    }
    LIQUIDITY_BUFFER {
        uuid id PK
        text env_id
        text asset_class
        numeric amount
    }
    ECL_MODEL {
        uuid id PK
        text env_id
        text stage
        numeric ecl
    }
    STRESS_SCENARIO {
        uuid id PK
        text env_id
        text name
        jsonb assumptions
    }
```

All tables env-scoped + RLS-enforced (`FORCE ROW LEVEL SECURITY`,
`env_id` match); explicit `WHERE env_id = $N` in every handler.

## 4. API Surface

| Audience | Routes | Auth gate | Events emitted |
|---|---|---|---|
| Public | `GET /v1/exposures`, `GET /v1/limits`, `GET /v1/limits/{id}`, `POST /v1/limits` (upsert - jurisdiction floors land here as registry data), `POST /v1/stress/scenarios` (declarative authoring), `POST /v1/stress/run` | `bank8:risk:{exposures,limits,stress}:{read,write}` via Shield | `bank8:risk:limit:{breached,upserted}`, `bank8:risk:stress:{scenario:upserted,completed}` |
| Admin/RFW | `GET /v1/admin/ui/{page}`, `GET /v1/admin/overview` | `bank8:risk:admin` | - |

Scenario authoring is declarative data (assumption JSONB, idempotent per
(env, name)) - a Func-authored hook cannot be audited as a filing input
(DESIGN.md §10 resolved). A stress run is an on-demand full recompute,
idempotent per (scenario, snapshot ref).

```mermaid
sequenceDiagram
    participant L as Bank8:Ledger
    participant B as Sentin8:Bus
    participant R as Bank8:Risk
    participant C as Bank8:Compliance
    participant RP as Bank8:Reporting
    L->>B: posting:created (full snapshot)
    B-->>R: SSE consume
    R->>R: update exposure + limit state (incremental)
    R->>B: limit:breached (risk.outbox)
    B-->>C: alert on breach
    B-->>RP: filing input
```

## 5. Cross-Cutting Concerns

- **Auth**: Sentin8:Auth-issued tokens verified; Shield against live
  roles; `scope` is a pre-filter, never the decision.
- **Identity - no token envs.** The worker's certificate-presenting Bus
  client is the single outbound identity (CERTFREE-4V - single identity
  plane, principles §4.1); config carries URLs only. A failed certificate
  mint fails the consume closed - there is no static-token fallback
  anywhere.
- **Event emission (membus → bus → API)**: every measurement change
  publishes the MemBus state refresh first (fail-soft), then the durable
  outbox row; the breach/stress events trigger downstream work via Bus.
  No cross-service table access ever - inputs are event payloads.
- **Tenant + RLS**: `set_tenant_context` awaited on the same connection
  as the query; explicit `WHERE env_id = $N`.
- **Idempotency**: incremental updates are idempotent per
  (event idempotency_key); re-delivery never double-counts exposure.
- **Audit**: `risk.audit_journal` in the same transaction + Sys audit
  push.
- **TLS**: PQ-TLS via `sentin8-tls`.

## 6. Failure Modes & Recovery

```mermaid
flowchart TD
    A[posting event] --> B{deduped?}
    B -->|already seen| C[skip]
    B -->|new| D[update exposure]
    D --> E{breach?}
    E -->|no| F[done]
    E -->|yes| G[emit limit:breached + journal]
    H[worker crash] --> I[replay from Valkey stream + idempotency key]
```

## 7. Concurrency & Performance Model

Pool capped at 3. Measurement is incremental (single-row exposure
updates keyed by dimension), so the hot path is a bounded UPSERT, not a
full recompute; full recomputes run only on demand (stress tests,
filing inputs). Backpressure via Valkey consumer group ACK.

## 8. Invariants

- Exposure updates are idempotent per event idempotency key - no
  double-counting.
- A limit breach is emitted at most once per (limit, direction)
  transition.
- Risk never writes ledger or payments tables; all inputs are events or
  public API reads.

## 9. Testing Strategy

- **Contracts crate** - shared jurisdiction/limit shapes.
- **Policy suite** (`tests/policy.rs`) - the real seeded scope-mirror
  text with the handler-action parity matrix (11 tests).
- **DB invariants** (`tests/db_invariants.rs`, all 5 pass against a live
  bank8 DB): FORCE-RLS rejection; exposure UPSERT convergence (one row
  per (env, dimension, currency)); at-most-once breach transition per
  (limit); stress-run idempotency per (scenario, snapshot);
  no-cross-schema-dependence (no risk view references
  ledger/payments).
- **E2E** - UI-only through the BFF (E2E-REG-1). No live-regulator
  stress certification.

## 10. Open Questions

- Stress-scenario definition language (declarative vs. Func-authored).
- Cross-currency netting conventions for exposure aggregation.

---

**Architecture & Design:** bank8:risk and its underlying concepts were designed by [Hamid Jahanzad](https://www.linkedin.com/in/hamidjahanzad/).
