# minutes-publisher — Public Meetings Minutes Publisher

An Open Meetings Act compliance publisher for the Exocortexia suite
(Nebraska state software funnel, demo 2 of 5).

**What it shows.** Twelve synthetic Nebraska-flavoured public bodies
(village board, county board, school board, natural resources district,
city council, fire district, and peers), six meetings each, every meeting
checked against the Act's timing rules. Each posting is recorded in a
hash-chained proof-of-posting audit log, and `GET /static` emits a single
self-contained HTML snapshot — the published artifact a body can post —
closing with the audit entries (sequence, action, hash prefix) and the
result of verifying the chain.

**Statute.** Neb. Rev. Stat. § 84-1413 and the Open Meetings Act
(§§ 84-1408–84-1414): agenda available at least 24 hours before the
meeting, minutes available within 10 working days, minutes retained at
least 6 months for the bodies the statute names. Full mapping and template
RFP language: `../../docs/minutes-publisher-statute.md`.

## Run

```sh
cargo run -p minutes-publisher            # http://127.0.0.1:8802
PORT=9002 cargo run -p minutes-publisher  # override the port
```

SSO is optional: with `EDIT_SSO_URL`, `EXO_SSO_SECRET`, and
`EXO_SESSION_SECRET` set (see the workspace API contract), all routes sit
behind the edit.phext.io SSO gate. Without them the app runs in local demo
mode and every page carries an "AUTH DISABLED" banner.

## Routes

| Route | What |
|---|---|
| `GET /` | Index of boards with per-board meeting and violation counts |
| `GET /board?name=...` | Meetings table with per-meeting badges (`OK` / `LATE AGENDA` / `LATE MINUTES` / `PENDING`) and each meeting's votes as a table |
| `GET /api/compliance` | JSON: per-board counts plus the full violations list |
| `GET /static` | Self-contained static snapshot of all boards and meetings, with the proof-of-posting section |

## Compliance model

Dates are epoch-day integers. Weekday uses the 0 = Sunday convention:
epoch day 0 (1970-01-01) was a Thursday, so `weekday = (day + 4) mod 7`;
working days are Monday–Friday. `working_days_between(a, b)` counts
working days in `(a, b]`.

- **Agenda:** `agenda_posted_day < meeting_day`. Day granularity
  approximates the statutory 24 hours — a same-day posting is a violation.
- **Minutes:** posted within 10 working days after the meeting → OK;
  unposted while the window is open → PENDING (not a violation); unposted
  after the window or posted late → LATE MINUTES.
- **Retention:** for meetings more than 180 days old, the minutes must
  still be present.

## Data note

Everything is synthetic and deterministic (`exo_core::Rng(23)`): board
names all contain "Example", members are invented, and votes are
generated. Meetings are stored as scroll text (`board=`, `meeting_day=`,
`agenda_posted_day=`, `minutes_posted_day=` — `none` while unposted — then
`vote=<member>|<motion>|<yes/no/abstain>` lines). The seed deliberately
includes a few same-day agendas, late minutes, one pending set, and one
old meeting whose minutes were never posted, so every badge and the
retention rule have something real to flag.

## Tests

```sh
cargo test -p minutes-publisher
```

Unit tests cover: weekday/working-day math on known dates (epoch day 0 =
Thursday; 2024-01-01 = Monday), agenda same-day = violation, minutes at
working-day 10 = OK and day 11 = late, pending → missing, the 180-day
retention rule, and the scroll round-trip.
