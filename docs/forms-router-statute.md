# Statute Sheet — Forms Router

> Template material for a demonstration and a possible solicitation
> response. Verify the current statutory text and any solicitation terms
> before relying on either. Nothing here is legal advice.

## 1. The demo

**Forms Router** (port 8805) is a working demonstration with synthetic
data: a public-records request form rendered from a data spec, submissions
tracked by a derived code, a strict four-state lifecycle
(received → routed → in review → closed) with a complete history timeline
and audit trail, and a CSV export. Illegal transitions are rejected with
nothing written — the demo's test suite enumerates all 16 state pairs and
proves exactly 3 are legal.

## 2. The statutes and channels

**Neb. Rev. Stat. § 84-712 — public records requests.** Nebraska's public
records law gives any person the right to examine public records and sets
agency duties for responding to requests. Every request an agency receives
has to be routed to the right custodian and answered inside that framework,
which makes intake — capturing the request completely, acknowledging it,
and tracking it to closure — a statutory workflow, not clerical overhead.
(Confirm current response timelines and fee provisions in the current
Revised Statutes before citing them.)

**Nebraska.gov small-application development.** The state's portal channel
has historically been the route for small agency web applications and
online forms. A forms-intake tool of this size fits that small-application
pattern: one form family, one workflow, hosted simply. (Confirm the
current network-manager arrangement and approval path — including OCIO
review of IT acquisitions — before proposing a channel.)

## 3. The pitch

- **Complete capture, minimal data.** The form asks for exactly what
  routing needs — agency, subject, description, date range — and nothing
  else.
- **No-PII design.** There is deliberately **no name, email, or contact
  field**. Status is followed with a tracking code issued at submission,
  so the system never has to store requester identities to keep requesters
  informed. This shrinks the privacy and records-retention surface of the
  intake system itself, and the request data can be handled as the public
  record it is.
- **A state machine, not a hope.** Requests move received → routed →
  in review → closed, one step at a time, with every move timestamped in
  the record and in a hash-chained audit log. Nothing skips, nothing goes
  backwards, nothing closes silently — which is what makes statutory
  response timelines measurable.
- **Cheap to run.** One small service per form family, an exportable CSV
  for whatever system of record the agency already keeps.

## 4. Paste-able RFP language (template)

> **TEMPLATE — adapt to the specific solicitation before use.**
>
> The Contractor shall provide a forms-intake service for public-records
> requests under Neb. Rev. Stat. § 84-712 that: (a) renders each intake
> form from a State-editable specification, so fields may be changed
> without modifying application code; (b) collects no personal information
> beyond what routing requires, and in particular shall not require a
> requester's name or contact information to submit or to track a request,
> instead issuing a tracking code at submission by which the requester may
> follow status; (c) manages each request through a defined state machine
> — received, routed, in review, closed — permitting only the transitions
> received→routed, routed→in-review, and in-review→closed, rejecting all
> other transitions without altering the record; (d) records every
> accepted transition, with timestamp, both in the request's own history
> and in a tamper-evident audit log; and (e) exports all requests, with
> current state, in a documented machine-readable format. The service
> shall display, on every intake form, a notice stating what information
> is collected and that personal information is not required.
