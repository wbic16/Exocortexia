# Records Desk — statute sheet

Demo: **Phext Records Desk** (`apps/records-desk`, port 8801)
Statutes: **Neb. Rev. Stat. § 84-712 et seq.** and **§ 84-712.05**

> This sheet is a demonstration aid, not legal advice. Statute summaries
> below are paraphrases for mapping software behaviour to legal duties;
> bidders and agencies should work from the current official text of the
> Nebraska Revised Statutes.

## What the demo does, in statutory terms

Nebraska's public records law, § 84-712 et seq., gives every citizen the
right to examine public records during regular business hours, requires
the custodian to respond to a request promptly (the statute contemplates
a response within four business days, with a written explanation and an
estimated availability date when the request cannot be filled in that
window), and permits charging only the actual, lawful costs the statute
allows. § 84-712.05 lists the categories of records that *may* be withheld
from the public — the exceptions.

The Records Desk maps one-to-one onto that structure:

| Demo behaviour | Statutory anchor |
|---|---|
| Full-text search across the whole records corpus, results in seconds | The custodian's duty to locate and produce responsive records under § 84-712 |
| Responsive set listed explicitly in the export packet, with coordinates | The response should identify what was found and produced |
| Redactions stored as a **separate layer** over untouched originals; the exported view shows `[REDACTED <category>]` markers | § 84-712.05 — withholding is by category/exception, applied to copies produced for the requester; the underlying public record is not altered |
| Redaction form requires a category chosen from the § 84-712.05 exception list, a byte range, and a written reason | Each withholding should be traceable to a specific statutory exception |
| Every redaction appends a hash-chained audit entry (actor, action, coordinate, payload hash) | Accountability for what was withheld, by whom, and why — the record a requester or a court would ask for |
| Export packet's time/cost worksheet: pages = bytes ÷ 2,000 (rounded up), review = 2 min/page, search/assembly = 5 min/document, cost = hours × a stated rate, delivery = hours ÷ 6 productive hours per day | Fees and timelines under § 84-712 must be actual and explainable; a worksheet whose assumptions are printed next to its arithmetic is the difference between an estimate and a number |

The worksheet is deliberately the centrepiece. A public-records response
is search, review, and arithmetic. Software that does the search in
milliseconds and shows its cost math line by line removes the factual
basis for multi-year delivery estimates and six-figure search fees: the
remaining time is human review, and the worksheet prices exactly that.

## Suggested RFP language (template)

> **Template language — for adaptation by the issuing agency; not a legal
> opinion.** The Respondent's public-records system shall: (1) maintain
> public records in a searchable store in which each record carries a
> stable, citable address; (2) apply redactions as a separate,
> reversible layer over unmodified original records, with each redaction
> bound to a specific exception under Neb. Rev. Stat. § 84-712.05, a
> stated reason, and the identity of the person applying it; (3) record
> every redaction and release in an append-only, tamper-evident audit log
> whose integrity can be verified independently; (4) generate, for any
> request, an export packet listing the responsive set and a time/cost
> worksheet in which every figure is derived from stated assumptions
> (bytes-per-page, minutes-per-page, hourly rate, productive hours per
> day) printed in the packet itself; and (5) demonstrate, on a reference
> corpus of at least 100,000 records, end-to-end production of an export
> packet — search, redaction, worksheet, and audit verification —
> within one business day of the request.

## Demo notes

- The demo corpus is synthetic (400 documents, deterministic seed) and
  contains no real personal information; a production deployment would
  ingest the agency's own records store.
- The $28.00/hour rate in the worksheet is a stated demo assumption, not
  a statutory figure; the worksheet prints whatever rate is configured
  alongside the math.
- Redaction categories in the demo use the short labels from
  `REDACTION_CATEGORIES` in exo-core (e.g. "84-712.05(3) personnel").
