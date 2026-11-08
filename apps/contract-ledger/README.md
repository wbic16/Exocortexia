# Contract Ledger

A feeder / reconciler for the Nebraska State Contracts Database
(Neb. Rev. Stat. § 84-602.04, enacted by LB 429).

## What it does

State contracts must be uploaded to the public State Contracts Database.
In practice, agency contract records and the public database drift apart:
uploads are missed, and amendments change authorized amounts without the
public record showing the overlay. This demo keeps a local ledger and
reconciles it:

- **Amendment overlay** — a contract's effective authorized amount is its
  base authorization plus every amendment delta. Base contract records are
  never modified; amendments are separate records that point at the base.
- **Expiry alerts** — each contract is bucketed as EXPIRED, or expiring
  within 30 / 60 / 90 days, as of today.
- **Missing-upload flag** — contracts present in agency records but absent
  from the public database are badged `MISSING FROM PUBLIC DATABASE`, which
  is the reconciler's work queue: these are the uploads to chase.

See `docs/contract-ledger-statute.md` in the workspace for the statute
sheet and template RFP language.

## Run

```sh
cargo run -p contract-ledger            # serves on port 8803
PORT=9000 cargo run -p contract-ledger  # override the port
```

With the SSO environment set (`EDIT_SSO_URL`, `EXO_SSO_SECRET`,
`EXO_SESSION_SECRET`, each secret ≥ 32 chars) every route sits behind the
shared edit.phext.io gate. Without it, the app runs open and HTML pages
carry an "AUTH DISABLED — local demo mode" banner.

## Routes

| Route | Description |
| --- | --- |
| `GET /` | HTML table of all contracts: vendor, agency, effective authorized, spent, spend bar, end date, alert / missing badges |
| `GET /contract?id=CTR-0001` | Detail page with the full amendment lineage (base → each amendment → running total) |
| `GET /api/contracts` | JSON for every contract, including effective authorized and alert bucket |
| `GET /api/alerts` | JSON for alerted and missing-from-public-database contracts only |
| `GET /health` | `ok` |

## Data note

All data is **synthetic and deterministic**: 60 contracts plus amendments
are generated in code with `exo_core::Rng(11)` and stored as phext scrolls
(base contracts at `1.1.1/1.1.1/5.1.<i>`, amendments at
`1.1.1/1.1.1/5.2.<j>` with an `amends=` line naming the base coordinate).
End days are generated relative to the runtime "today"
(`exo_core::today_epoch_day()`), spread so every alert bucket and the
missing-upload flag appear on every run. No real contract, vendor, or
agency transaction data is used.
