# Exocortexia

A proof of concept automated system for states with fewer than 5M residents.

Exocortexia is a suite of five small public-records and transparency web
apps that share one storage core: every document lives as a scroll in a
[phext](https://phext.io) (a plain-text, multi-dimensional document store),
every mutation is written to a hash-chained audit log, and redactions are
first-class, reviewable data rather than edits. The whole suite is a single
Rust workspace using only the standard library for HTTP — no web framework,
no async runtime — so it builds fast, runs on modest hardware, and can be
read end-to-end by one person.

Sign-in is shared with [edit.phext.io](https://edit.phext.io) through a
small HMAC assertion protocol (see [docs/SSO.md](docs/SSO.md)); when SSO is
not configured the apps run in an open local demo mode with a visible
banner.

## The suite

| Component | Purpose | Port |
|---|---|---|
| records-desk (`apps/records-desk`) | Public-records search with audited, reversible redactions (Neb. Rev. Stat. § 84-712) | 8801 |
| minutes-publisher (`apps/minutes-publisher`) | Public-meeting agenda/minutes posting and timeliness checks (Open Meetings Act, § 84-1407 et seq.) | 8802 |
| contract-ledger (`apps/contract-ledger`) | Contract and grant ledger cross-checked against the public contracts database | 8803 |
| data-catalog (`apps/data-catalog`) | Open-data catalog: datasets, fields, definitions, provenance, refresh cadence | 8804 |
| forms-router (`apps/forms-router`) | Forms intake router: classifies incoming forms and routes them to the owning office | 8805 |
| exo-core (`crates/core`) | Shared core: phext `Store`, audit log, redactions, mini HTTP server, deterministic RNG | — |
| exo-sso (`crates/sso`) | Consumer-side edit.phext.io SSO gate plus the `edit-minter` local test minter | — |
| exo-seed (`seed/`) | Deterministic synthetic corpora (records, minutes, contracts, catalog) and the `seedgen` generator | — |
| loadsim (`tools/loadsim`) | Load/store simulator: HTTP latency percentiles and store benchmarks with a tamper negative control | — |

## Quickstart

Build everything:

```sh
cargo build --release
```

Run an app (each also respects a `PORT` environment variable):

```sh
cargo run -p records-desk --release        # http://127.0.0.1:8801
cargo run -p minutes-publisher --release   # http://127.0.0.1:8802
cargo run -p contract-ledger --release     # http://127.0.0.1:8803
cargo run -p data-catalog --release        # http://127.0.0.1:8804
cargo run -p forms-router --release        # http://127.0.0.1:8805
```

Generate the standard seed phexts into `seed/out/`:

```sh
cargo run -p exo-seed --bin seedgen --release
```

### SSO (optional locally, required for a public deployment)

With no SSO variables set, the apps run open in demo mode and show an
"AUTH DISABLED — local demo mode" banner. To enable edit.phext.io SSO:

```sh
export EDIT_SSO_URL="https://edit.phext.io/sso"   # edit-side SSO endpoint
export EXO_SSO_SECRET="<shared assertion secret, >= 32 chars>"
export EXO_SESSION_SECRET="<this app's own session secret, >= 32 chars>"
export EXO_SSO_AUD="exocortexia"                  # optional; this is the default
```

For local SSO testing without a deployed edit side, use the bundled
minter — see the step-by-step sequence in [docs/SSO.md](docs/SSO.md).

### Load simulation

```sh
# HTTP mode: latency percentiles against a running app
cargo run -p loadsim --release -- http \
  --base http://127.0.0.1:8801 --path "/search?q=InterRAI" \
  --concurrency 8 --requests 400 --p95-budget-ms 250

# Store mode: build/serialize/parse/search/audit/redaction benchmarks
# plus the audit tamper negative control
cargo run -p loadsim --release -- store --docs 100000
```

## Data

All data in this repository is **synthetic or public**: the seed corpora are
generated deterministically from fixed-seed RNGs with fictional people,
vendors, and boards, and the demo datasets describe public record types.
There is no real personal information (no PII) anywhere in the suite, and
the apps are designed so that real deployments keep it that way — records
enter the store only through the audited, redactable paths in exo-core.

## Verification

Build, test, SSO smoke, seed, benchmark, and load results — including the
negative controls — are recorded in
[docs/VERIFICATION.md](docs/VERIFICATION.md).

## Repository layout

```text
crates/core/          exo-core: Store, AuditLog, redactions, HTTP, RNG
crates/sso/           exo-sso: SSO gate, session sealing, edit-minter bin
apps/records-desk/        port 8801
apps/minutes-publisher/   port 8802
apps/contract-ledger/     port 8803
apps/data-catalog/        port 8804
apps/forms-router/        port 8805
seed/                 exo-seed corpora + seedgen bin (output: seed/out/)
tools/loadsim/        loadsim binary (modes: http, store)
docs/                 SSO.md, VERIFICATION.md
API-CONTRACT.md       the binding interface contract every crate builds to
```

## License

MIT. See the `LICENSE` file at the repository root.
