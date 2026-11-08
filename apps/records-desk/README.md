# records-desk — Phext Records Desk

A public-records search, redaction, and export demo for the Exocortexia
suite (Nebraska state software funnel, demo 1 of 5).

**What it shows.** A custodian can search ~400 public-records documents,
apply statutory redactions as a *separate layer* (the original scroll is
never modified — the redacted view is computed at read time, and a
"custodian view (original)" shows the untouched bytes side by side), and
produce an export packet whose time/cost worksheet is explicit arithmetic:
pages from bytes, minutes from pages, cost from a stated hourly rate, and a
delivery estimate in working days. The worksheet is the point: search,
review, and arithmetic do not take years.

**Statute.** Neb. Rev. Stat. § 84-712 et seq. (public records; § 84-712.05
lists the record categories that may be withheld — those are the redaction
categories offered by the redact form). Full mapping and template RFP
language: `../../docs/records-desk-statute.md`.

## Run

```sh
cargo run -p records-desk            # http://127.0.0.1:8801
PORT=9001 cargo run -p records-desk  # override the port
```

SSO is optional: with `EDIT_SSO_URL`, `EXO_SSO_SECRET`, and
`EXO_SESSION_SECRET` set (see the workspace API contract), all routes sit
behind the edit.phext.io SSO gate. Without them the app runs in local demo
mode and every page carries an "AUTH DISABLED" banner.

## Routes

| Route | What |
|---|---|
| `GET /` | Search page (query form; results link to documents) |
| `GET /api/search?q=...` | JSON hits: `coord`, `subject`, `snippet` (±60 chars around the first match). Empty `q` returns all documents, limit 50 |
| `GET /doc?coord=...` | Redacted public view + custodian view (original) + recorded redactions + redact form |
| `POST /api/redact` | Form fields `coord`, `start`, `end`, `category`, `reason`, `actor`. Validates `start < end <= doc length` and category ∈ `REDACTION_CATEGORIES`; appends a hash-chained audit entry; JSON `{"ok":true,...}` or 400 |
| `GET /api/export?q=...` | Plain-text export packet (default `q=InterRAI`): responsive set, redaction log, time/cost worksheet |

## Data note

The corpus is generated in code from `exo_core::Rng(7)` — memos, emails,
and meeting notes about bridge maintenance, InterRAI public meetings, park
permits, budget hearings, and similar public topics. Senders use invented
names at `example.org`. **No real personal information exists anywhere in
this app.** Documents live at coordinates `1.1.1/1.1.1/1.1.<i>`; redactions
and audit entries live in their own stores.

## Tests

```sh
cargo test -p records-desk
```

Unit tests cover: search finding known documents (including
case-insensitivity and the empty-query limit), the redaction layer
(view differs, stored original byte-identical), and the worksheet math
(expected values derived from the stated assumptions inside the test).
