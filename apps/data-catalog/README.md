# Data Catalog

A data-governance catalog demo: eight synthetic Nebraska datasets with
field-level definitions, staleness tracking, and on-demand source-link
checks.

## What it does

- **Catalog** — each dataset has an owner, an update cadence, a
  last-updated date, a source URL, and a table of its fields with a plain
  definition, a type, and a provenance (where the field's values come
  from) for every field.
- **Staleness** — a dataset is `STALE` when `today − last_updated_day`
  exceeds its declared cadence, `FRESH` otherwise. Badges are computed as
  of the day you look, never stored.
- **Link checks, on demand only** — `check_url` opens a plain TCP
  connection to an `http://` host and reads the HTTP status line (5-second
  timeouts). `https://` URLs are honestly reported as "not checked in
  demo" rather than silently skipped or faked. Checks run only when the
  link-check endpoint is called — never on page load, so viewing the
  catalog does no network work.

See `docs/data-catalog-statute.md` in the workspace for the framing sheet,
including the honest caveat that the need for this catalog is an
unvalidated hypothesis.

## Run

```sh
cargo run -p data-catalog            # serves on port 8804
PORT=9000 cargo run -p data-catalog  # override the port
```

With the SSO environment set (`EDIT_SSO_URL`, `EXO_SSO_SECRET`,
`EXO_SESSION_SECRET`, each secret ≥ 32 chars) every route sits behind the
shared edit.phext.io gate. Without it, the app runs open and HTML pages
carry an "AUTH DISABLED — local demo mode" banner.

## Routes

| Route | Description |
| --- | --- |
| `GET /` | HTML dataset cards with per-field tables and FRESH/STALE badges |
| `GET /api/staleness` | JSON staleness report for every dataset |
| `GET /api/links` | Run the link checker over all source URLs now; JSON results |
| `POST /api/check-links` | Same as `GET /api/links` |
| `GET /health` | `ok` (also the live target for one dataset's source URL) |

## Data note

All data is **synthetic**. Eight Nebraska-flavoured datasets are declared
in code and stored as phext scrolls: dataset scrolls at
`1.1.1/1.1.1/6.1.<i>` and one field scroll per field nearby at
`1.1.1/1.1.1/6.<10+i>.<f>`. Seven source URLs deliberately point at
`http://127.0.0.1:9/...` (nothing answers) and one points at this app's
own `/health` endpoint, so a link check shows both the dead and the live
state. No real dataset, agency system, or personal data is involved.
