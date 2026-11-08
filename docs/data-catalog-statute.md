# Statute Sheet — Data Catalog

> Template material for a demonstration and a possible solicitation
> response. Verify current policy documents and solicitation terms before
> relying on them. Nothing here is legal advice.

## 1. The demo

**Data Catalog** (port 8804) is a working demonstration with synthetic
data: eight Nebraska-flavoured datasets, each with an owner, a declared
update cadence, a last-updated date, a source URL, and a field-level table
(field, plain-language definition, type, provenance). It computes
FRESH/STALE badges as of today and can check, on demand, whether each
dataset's source URL still answers.

## 2. The framing — a pilot, honestly labelled

Unlike the other apps in this suite, the Data Catalog is **not** tied to a
single statute that mandates a catalog. It is framed as **data-governance
pilot support**:

- **Hypothesis (unvalidated):** the catalog need itself is unproven. No
  agency has yet confirmed, in a requirements process, that a shared
  dataset catalog with field definitions, provenance, and staleness
  tracking is a gap they need filled. This demo exists to make that
  conversation concrete — a steward can react to a working catalog in a
  way they cannot react to a slide — not to presume the answer.
- **Context:** state IT leadership through the Office of the Chief
  Information Officer (OCIO) has publicly prioritised data quality and
  "clean data" as foundations for later modernisation and AI work. A
  catalog is one plausible instrument for that priority: you cannot clean
  data you have not inventoried, and you cannot keep data clean without
  knowing its owner, cadence, and provenance. That is context for the
  hypothesis, not evidence for it.

The correct next step this sheet supports is a **time-boxed pilot with a
validation gate**, not a platform purchase.

## 3. The pitch

- Inventory first: datasets, owners, cadences, and field-level definitions
  and provenance in one inspectable place.
- Freshness as a rule, not a vibe: staleness is one stated formula
  (age > cadence), recomputed on every view.
- Link rot surfaced on demand: source URLs are tested when a steward asks,
  and the checker says plainly what it does not check (HTTPS in this demo).
- Pilot terms: a small fixed scope, a named set of real datasets from one
  or two volunteer agencies, and a written go/no-go based on whether
  stewards actually use the staleness and provenance views.

## 4. Paste-able RFP language (template)

> **TEMPLATE — adapt to the specific solicitation before use. The pilot
> and validation-gate structure below is deliberate; do not strip it.**
>
> The Contractor shall provide a data-catalog pilot that: (a) inventories
> an initial set of datasets designated by the State, recording for each
> dataset its owning agency, declared update cadence, last-updated date,
> and source location; (b) records, for each field of each dataset, a
> plain-language definition, a data type, and a provenance statement
> identifying where the field's values originate; (c) computes dataset
> freshness by a single stated rule — a dataset is stale when the time
> since its last update exceeds its declared cadence — and reports
> freshness as of the date of viewing; (d) tests source locations on
> demand and reports, for each, whether it answered, and discloses any
> class of location the tool does not test; and (e) concludes with a
> written pilot evaluation, prepared with the participating agencies,
> stating whether the catalog met a demonstrated governance need and
> recommending whether to expand, revise, or discontinue it. Expansion
> beyond the pilot scope shall occur only on the State's written
> direction following that evaluation.
