# Forms Router

A forms-intake demo with an explicit state machine, built for
public-records request intake. The form is rendered from a data spec, not
hard-coded in the page.

## What it does

- **Spec-driven form** — the `public-records-request` form is a phext
  scroll (`form=` plus one `field=` line per field). The HTML form is
  rendered by parsing that scroll, so changing the form means changing
  the spec.
- **No-PII by design** — the fields are `agency`, `subject`,
  `description`, and `date_range`. There are **no name, email, or contact
  fields**: routing a request needs none of them, so the form never asks.
  The page carries a large banner: **SYNTHETIC DATA ONLY — DO NOT ENTER
  PERSONAL INFORMATION**.
- **Tracking code** — each submission gets a code derived as the first 10
  hex characters of `sha256(form + seq + values)`. The requester follows
  status with the code alone — no account, no email round-trip.
- **Strict state machine** — `received → routed → in_review → closed` are
  the only legal transitions (3 of 16 pairs). Accepted transitions append
  a history line to the submission scroll and an entry to the hash-chained
  audit log; rejected transitions write nothing and return HTTP 400.
- **CSV export** — all submissions as RFC 4180 CSV
  (`code, form, state, agency, subject`).

See `docs/forms-router-statute.md` in the workspace for the statute sheet
and template RFP language.

## Run

```sh
cargo run -p forms-router            # serves on port 8805
PORT=9000 cargo run -p forms-router  # override the port
```

With the SSO environment set (`EDIT_SSO_URL`, `EXO_SSO_SECRET`,
`EXO_SESSION_SECRET`, each secret ≥ 32 chars) every route sits behind the
shared edit.phext.io gate. Without it, the app runs open and HTML pages
carry an "AUTH DISABLED — local demo mode" banner.

## Routes

| Route | Description |
| --- | --- |
| `GET /` | Synthetic-data banner, the spec-rendered submission form, and a tracking lookup form |
| `POST /api/submit` | Create a submission in state `received`; returns an HTML page with the tracking code |
| `POST /api/transition` | Form fields `code`, `to`. `200` JSON with the new state on success; `400` JSON error for an illegal transition (nothing is written) or an unknown target state; `404` JSON for an unknown code |
| `GET /track?code=<code>` | HTML status page with the submission's fields and full history timeline |
| `GET /api/export.csv` | CSV export of all submissions |
| `GET /health` | `ok` |

Example transition call:

```sh
curl -X POST localhost:8805/api/transition -d 'code=ab12cd34ef&to=routed'
```

## Data note

All data is **synthetic**. The form spec lives at phext coordinate
`1.1.1/1.1.1/7.1.1`; each submission is a scroll at
`1.1.1/1.1.1/7.2.<seq>` holding `code=`, `form=`, `state=`, one line per
field value, and `history=<seq>|<from>-><to>|<ts>` lines. Submissions and
the audit log live in memory behind mutexes for the life of the process.
Do not enter real personal information — the form is designed so that you
never need to.
