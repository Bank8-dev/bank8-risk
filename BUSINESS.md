# Bank8:Risk — Business Requirements & Functionality

> Audience: product owners, business analysts, and anyone evaluating what
> Bank8:Risk **does for the business**. For architecture read `README.md`;
> for wire-level detail read `DESIGN.md`.

## 1. Business Purpose

- **Risk known at any moment, not just at filing time** — capital
  adequacy, liquidity coverage, credit loss, and limit headroom are
  measured continuously from the bank's own posting and payment events.
- **Breaches surface instantly** — crossing a limit is a domain event
  that alerts Compliance and feeds Reporting, rather than a number
  found later in a spreadsheet.
- **Portable thresholds** — Basel/CRR3 output floors, OSFI CAR/LAR, US
  Basel III are jurisdiction registry data, not hardcoded.

## 2. Business Scope (What the Service Provides)

| # | Capability area | What the business gets |
|---|---|---|
| 1 | Exposure measurement | Counterparty/currency/asset-class positions updated incrementally |
| 2 | Capital measurement | Basel capital components and ratios (CET1, Tier 1, total) |
| 3 | Liquidity measurement | LCR/NSFR buffers and ratios |
| 4 | Credit risk | IFRS 9 expected-credit-loss staging (12-month vs lifetime) |
| 5 | Limit monitoring | Thresholds with breach events |
| 6 | Stress testing | Scenario runs with assumptions and outcomes |
| 7 | Domain events | `bank8:risk:*` family (breach, stress completion) |
| 8 | Health & ops | Health endpoint, metrics, audit journal |

**What the service does NOT own (business boundary):**

| Out of scope | Owned by |
|---|---|
| Money movement / balances | Bank8:Ledger |
| Payment state | Bank8:Payments |
| Filing | Bank8:Reporting |
| Tenant-authored hook execution | Sentin8:Func (Risk consumes results) |
| Metric aggregation | Sentin8:Stat |

## 3. Business Capabilities

| Capability | Business rule |
|---|---|
| Exposure | Every posting updates the relevant exposure; re-delivery never double-counts. |
| Capital | Capital ratios reflect current exposures; floors are jurisdiction data. |
| Liquidity | LCR/NSFR buffers are measured from current asset/liability positions. |
| Credit loss | ECL staging reflects credit events in near-real time. |
| Limits | A breach is emitted exactly once per threshold crossing. |

## 4. Business Events (Outbound)

| Event | Fires when |
|---|---|
| `bank8:risk:limit:breached` | A measured value crosses a configured limit |
| `bank8:risk:stress:completed` | A stress scenario run finishes |

## 5. Business Rules & Invariants

1. Exposure updates are idempotent per event.
2. A breach is emitted at most once per (limit, direction) transition.
3. Risk never writes another service's tables.

## 6. Target-User Roles & Use Cases

| Role | Use case |
|---|---|
| Risk officer | Reviews exposures, limits, ECL; runs stress scenarios |
| Compliance officer | Consumes breach alerts |
| Regulator | Receives capital/liquidity inputs via Reporting |

## 7. KPIs / Success Criteria

- Breach detection latency ≤ seconds from the triggering posting.
- Zero double-counted exposures.

## 8. Dependencies & Reliance on Other Services

- Bank8:Ledger, Bank8:Payments (events), Sentin8:Bus, Sentin8:Func,
  Bank8:Reporting (downstream consumer).

## 9. Acceptance Criteria

1. A posting event updates the matching exposure exactly once.
2. A limit breach emits an event consumed by Compliance and Reporting.
3. Jurisdiction thresholds are registry data.

## 10. Open Business Questions

- Cross-currency netting conventions.
- Stress-scenario definition language.

---

**Design attribution:** bank8:risk and its underlying concepts were designed by Hamid Jahanzad <Hamid@Jahanzad.com>.
