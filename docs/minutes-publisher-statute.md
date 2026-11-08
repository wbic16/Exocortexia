# Minutes Publisher — statute sheet

Demo: **Public Meetings Minutes Publisher** (`apps/minutes-publisher`, port 8802)
Statute: **Neb. Rev. Stat. § 84-1413** (Open Meetings Act, §§ 84-1408–84-1414)

> This sheet is a demonstration aid, not legal advice. Statute summaries
> below are paraphrases for mapping software behaviour to legal duties;
> bidders and agencies should work from the current official text of the
> Nebraska Revised Statutes.

## The three rules the demo enforces

Nebraska's Open Meetings Act requires, in substance:

1. **Agenda — 24 hours.** A public body must make its agenda available to
   the public at least 24 hours before the meeting (§ 84-1412 covers the
   agenda duty; the publisher checks it alongside § 84-1413's minutes
   duties). The demo works at day granularity: an agenda posted on an
   earlier day than the meeting passes; a same-day posting is flagged
   **LATE AGENDA**.
2. **Minutes — 10 working days.** Under § 84-1413, the minutes of a
   meeting must be written and available for public inspection within
   **10 working days** or before the next meeting, whichever comes first.
   The demo flags minutes posted after the 10th working day — or not
   posted once the window has closed — as **LATE MINUTES**, and minutes
   still inside the window as **PENDING** (not a violation).
3. **Retention — 6 months.** § 84-1413 requires minutes to be retained
   for at least **6 months** by the bodies it names: **natural resources
   districts, cities, counties with a population above the statutory
   threshold (25,000), and school boards** — political subdivisions whose
   records the Legislature singled out for a minimum retention floor.
   (Other public bodies remain subject to their applicable records
   retention schedules.) The demo flags a meeting more than 180 days old
   whose minutes are no longer present.

## What the demo does, in statutory terms

| Demo behaviour | Statutory anchor |
|---|---|
| Every meeting stored as an addressable record: board, meeting day, agenda posting day, minutes posting day, and each vote (member, motion, yes/no/abstain) | § 84-1413's requirement that minutes record the substance of matters discussed and votes taken |
| Compliance computed from the posting days, not asserted: `agenda_posted < meeting_day`; minutes within 10 working days (Mon–Fri) counted in `(meeting_day, posted_day]` | The Act's deadlines are arithmetic; the demo shows the arithmetic |
| Proof-of-posting audit log: each agenda and minutes posting appends a hash-chained entry at publication time | A body that must prove *when* something was posted needs a tamper-evident posting record, not a file timestamp |
| `GET /static` emits one self-contained HTML snapshot of all boards and meetings, closing with the audit entries and a chain-verification result | The published artifact itself carries its own evidence of timely posting |
| Per-board compliance summary and a JSON violations list (`GET /api/compliance`) | Oversight is per-body; a clerk or a citizen can see exactly which meetings of which body are out of compliance |

Working-day counting is the subtle part, and the demo does it explicitly:
epoch day 0 (1970-01-01) was a Thursday; with 0 = Sunday, `weekday =
(day + 4) mod 7`, working days are Monday–Friday, and the 10-day window is
counted in working days only — a meeting on a Friday has until the Monday
of the second following week, not the next calendar Monday-plus-ten.

## Suggested RFP language (template)

> **Template language — for adaptation by the issuing agency; not a legal
> opinion.** The Respondent's meeting-records system shall: (1) store each
> meeting as a discrete, addressable record carrying the body, the meeting
> date, the agenda posting timestamp, the minutes posting timestamp, and
> each recorded vote by member and motion; (2) compute Open Meetings Act
> compliance arithmetically from those timestamps — agenda available at
> least 24 hours before the meeting, minutes available within 10 working
> days (Neb. Rev. Stat. § 84-1413), and minutes retained for at least 6
> months for the bodies § 84-1413 names, including natural resources
> districts, cities, larger counties, and school boards — and report
> violations per body without manual review; (3) maintain an append-only,
> tamper-evident proof-of-posting log in which every agenda and minutes
> posting is recorded at publication time and whose integrity can be
> verified independently; and (4) publish a self-contained static snapshot
> of all meeting records and compliance status that a member of the public
> can save and verify offline.

## Demo notes

- All twelve boards in the demo are synthetic (every name contains
  "Example"); members and votes are generated deterministically. The seed
  deliberately includes same-day agendas, late minutes, one pending set,
  and one old meeting with missing minutes so every rule visibly fires.
- Day granularity is a demo simplification of the 24-hour agenda rule; a
  production deployment would store timestamps, not days, for that check.
  The 10-working-day minutes rule and the 6-month retention rule are
  day-based in substance and are computed exactly.
