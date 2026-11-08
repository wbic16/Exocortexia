# Verification — Exocortexia

Task coordinate: 31.5.9/7.5.9/6.5.7 (parent funnel 29.5.13/6.5.9/5.4.8).
All results below are from an actual run on 2026-11-07 (America/Chicago
evening). Reproduce with `scripts/verify.sh` (build + tests + seed +
store benchmarks) plus the HTTP/SSO steps listed inline.

## Environment

- Host: ranch node **aletheia-core** (x86_64, 16 cores, 29 GB RAM).
- Toolchain: cargo 1.94.0 / rustc 1.94.0.
- Phext library: **libphext 0.4.0 from crates.io** (the in-tree 0.4.2 on
  libphext-rs `exo` branches is not published to crates.io; 0.4.0 is what
  compiles cleanly and is what this suite uses — recorded per instructions).
- Thermal: node idled at Tctl ≈ 50 °C; peak observed 71.6 °C during the
  first full compile; the ≤ 75 °C sustained target held throughout.
- Other deps: sha2 0.10, hmac 0.12, base64 0.22, socket2 0.5 (see the
  backlog fix under Known failures — it was found and fixed by testing).

## Build

`cargo build --workspace` (dev) and `cargo build --release --workspace`
both clean. Two warnings, both benign and left visible on purpose:

- `exo-sso`: `value assigned to pos is never read` (flat-JSON parser).
- `minutes-publisher`: field `audit_store` never read after seeding (the
  `AuditLog` handle is what the routes use; the backing store struct
  field is retained for symmetry with records-desk).

## Unit tests

`cargo test --workspace`: **69 passed, 0 failed.**

| Target | Passed |
|---|---|
| exo-core | 14 |
| exo-sso | 10 |
| exo-seed | 8 |
| records-desk | 4 |
| minutes-publisher | 7 |
| contract-ledger | 8 |
| data-catalog | 7 |
| forms-router | 11 |
| loadsim / seedgen / edit-minter | 0 (exercised live, below) |

Coverage highlights: phext serialize/parse round-trip identity
(exo-core); redaction never mutates original scroll bytes (exo-core +
records-desk); audit hash-chain verifies on an honest chain and fails
after tampering — in memory and after a store round-trip (exo-core);
SSO mint→verify, wrong secret, expired, replayed jti, wrong aud, and
exp−iat > 120 all rejected (exo-sso); forms-router enumerates all 16
state pairs — exactly 3 legal transitions, every illegal one rejected
without mutating state; minutes working-day/Open-Meetings clocks;
contract amendment lineage totals; catalog staleness math; seed
determinism (byte-identical `to_phext()` across calls).

## Seed generation

`./target/release/seedgen` (≈ 0.23 s):

| File | Docs | Bytes | SHA-256 |
|---|---|---|---|
| seed/out/records-100k.phext | 100,000 | 32,417,503 | e859f7f966570050899bcbdac46770b519fed2ce52047888dd285c50e8fb70b3 |
| seed/out/minutes.phext | 20,000 (500 boards × 10 yrs × 4 mtgs) | 2,378,480 | a449d7e592a702bf7a9d291867182941c32175adb6b075f8b86986f504627ddc |
| seed/out/contracts-10k.phext | 11,916 (10,000 + 1,916 amendments) | 1,835,276 | 1016bb1e9529856981a05b0c708ea01a555873e231eb59c058e459b0899fc61e |

(seed/out is gitignored; regenerate deterministically — corpora are
seeded, so bytes are reproducible.)

## Store benchmarks (loadsim store, 100k documents)

`./target/release/loadsim store --docs 100000` → exit 0:

```
build_ms=69.43 stored_docs=100000
serialize_ms=82.60 bytes=32417503
parse_ms=94.29 parsed_docs=100000
search_runs=200 search_hits=3215800 search_ms=2407.35   (~12 ms per full-corpus scan)
audit_appends=1000 audit_build_ms=3.32 audit_verify_ms=1.12
redaction_runs=1000 redaction_ms=0.21
negative_control=detected
```

The negative control flips one character of a stored audit hash in a
cloned store; verification must fail, and it did. If it ever doesn't,
loadsim exits 1.

## Functional smoke (all five apps running, release binaries)

- All five homepages: HTTP 200. Records search returns hits with
  snippets; doc page shows the redacted view and the custodian
  (original) view side by side.
- records-desk: POST /api/redact with an invalid category → 400 with a
  category error; with a valid 84-712.05 category → `{"ok":true}` and
  the doc page then shows `[REDACTED 84-712.05(3) personnel records]`
  in place of the redacted span; the stored original is unchanged.
  GET /api/export prints the packet (responsive set + redaction log +
  worksheet with assumptions stated inline).
- minutes-publisher: /api/compliance reports per-board violations from
  the seeded data; /static carries the proof-of-posting log — 72
  post_agenda and 65 post_minutes audit entries (72 meetings; 7 sets of
  minutes deliberately unposted by the seed to exercise the checks).
- contract-ledger: /api/contracts returns 60 contracts; /api/alerts
  returns EXPIRED/30/60/90 buckets and missing-from-public-database
  flags as seeded.
- data-catalog: /health → ok; /api/staleness flags stale datasets;
  /api/links → 1 live link (its own /health, HTTP 200) and 7 dead
  (127.0.0.1:9, connection refused) — both states demonstrated.
- forms-router: no-PII banner present; submit → tracking code issued;
  legal transition received→routed → 200; illegal routed→closed → 400;
  /track shows the state; /api/export.csv contains the submission.

## SSO smoke (consumer side, mock edit-minter)

records-desk started with EDIT_SSO_URL / EXO_SSO_SECRET /
EXO_SESSION_SECRET set (throwaway local test secrets, ≥ 32 chars) and
edit-minter running with the matching SSO secret:

- Unauthenticated GET / → 302 → /auth/login?return=%2F.
- Full curl cookie-jar chain (app → /auth/login → minter → /auth/callback
  with assertion) → final 200; page carries the "SSO: edit.phext.io"
  banner; `exo_session` cookie set (host-only, HttpOnly).
- First use of a minted assertion → 302 (session minted).
- **Replay of the same assertion → 401** (single-use jti enforced live).
- **Tampered assertion (one character changed) → 401.**
- With SSO env unset (pass-through): apps serve normally and every page
  carries the "AUTH DISABLED — local demo mode" banner.

Production gap, stated plainly: the edit.phext.io *minting* side for
aud="exocortexia" does not exist yet — it is Will's phext-lattice deploy
step (see docs/SSO.md). The consumer is verified against the mock
minter, which implements the same assertion format.

## HTTP load (loadsim http, budget: p95 ≤ 500 ms, zero errors required)

Concurrency 50, 1,000 requests each (single node, loopback):

| App / path | errors | p50 ms | p95 ms | p99 ms |
|---|---|---|---|---|
| records-desk /api/search?q=InterRAI | 0 | 10.08 | 11.68 | 20.20 |
| minutes-publisher /api/compliance | 0 | 8.80 | 15.18 | 18.27 |
| contract-ledger /api/contracts | 0 | 1.17 | 1.85 | 2.03 |
| data-catalog /api/staleness | 0 | 0.76 | 1.08 | 1.27 |
| forms-router / | 0 | 0.81 | 1.55 | 2.16 |

Concurrency scaling on forms-router / (2,000 requests each):

| Concurrency | errors | p50 ms | p95 ms | p99 ms | verdict |
|---|---|---|---|---|---|
| 100 | 0 | 1.15 | 2.72 | 15.36 | within budget |
| 200 | 0 | 1.62 | 11.36 | 1,028.11 | p95 within budget; p99 tail (see below) |
| 500 (pre-fix) | 0 | 2.04 | 1,047.97 | 1,055.55 | **FAILED the 500 ms p95 budget** |
| 200 (post-fix) | 0 | 2.75 | 11.51 | 21.65 | within budget |
| 500 (post-fix) | 0 | 3.49 | 26.22 | 38.40 | within budget |

records-desk search at concurrency 200 post-fix: 0 errors, p50 41.72 ms,
p95 58.77 ms, p99 83.29 ms — within budget.

## Known failures, and the fix

**500-concurrent latency failure (found by loadsim, fixed, re-verified).**
At 500 simultaneous clients the pre-fix server kept 0 errors but p95 hit
≈ 1,048 ms — a bimodal tail at almost exactly 1 s, the TCP SYN
retransmit interval. Root cause: std `TcpListener::bind` uses a
128-deep listen backlog; a burst overflows it, the kernel drops SYNs,
and clients retry after ~1 s. Fix: bind through `socket2` with a
1024-deep backlog (crates/core/src/http.rs). Post-fix at 500 concurrent:
p95 26.22 ms, p99 38.40 ms, 0 errors. The pre-fix row is kept in the
table above — receipts, not cosmetics.

**Open / honest limitations**

- The records-desk app demo runs its seeded 400-document corpus; the
  100k corpus is verified at store level (above) and via seedgen, not
  loaded into the demo app process.
- data-catalog link checking is http-only by design; https URLs are
  reported as "https not checked in demo", never silently passed.
- Load numbers are single-node loopback on a 16-core ranch box; they
  show headroom for a <5M-resident state's small-board traffic shape,
  not internet-path latency.
