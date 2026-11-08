# Statute Sheet — Contract Ledger

> Template material for a demonstration and a possible solicitation
> response. Verify the current statutory text and any solicitation terms
> before relying on either. Nothing here is legal advice.

## 1. The demo

**Contract Ledger** (port 8803) is a working demonstration with synthetic
data: 60 generated state contracts with amendment overlays, expiry alert
buckets (expired / 30 / 60 / 90 days), spend percentages, and a
missing-from-public-database flag. It runs in seconds, needs no external
services, and every number on screen can be recomputed from the stored
records — amendments never rewrite the base contract, so the lineage from
base authorization to effective authorization is fully auditable.

## 2. The statute

**Neb. Rev. Stat. § 84-602.04 — State Contracts Database (LB 429).**
LB 429 (2011) established Nebraska's public State Contracts Database and
the duty for state agencies to make their contracts part of that public
record, administered under § 84-602.04. (Confirm the current codified text,
the administering office, and the contract-value thresholds in the current
Revised Statutes before citing them in a filing or response.)

The compliance problem § 84-602.04 creates is a *reconciliation* problem:
the statute is satisfied only when what agencies hold matches what the
public database shows. Missed uploads and un-reflected amendments are
exactly the gaps a feeder/reconciler closes.

## 3. The pitch

Contract Ledger is **not** a replacement database. It is a **feeder and
reconciler** that sits between agency contract records and the public
State Contracts Database:

- ingests agency contract records in a stable, append-only form;
- applies amendments as overlays, preserving the base record and the full
  lineage (base → amendment → running effective total);
- flags every contract **missing from the public database** so the upload
  gap is a work list, not an audit surprise;
- raises expiry alerts at 90 / 60 / 30 days so renewals are planned, not
  discovered after lapse.

## 4. Paste-able RFP language (template)

> **TEMPLATE — adapt to the specific solicitation before use.**
>
> The Contractor shall provide a contract-ledger reconciliation service
> that: (a) maintains an append-only record of each covered contract in
> which the original contract record is never altered and each amendment
> is recorded as a separate, linked entry stating its change to the
> authorized amount; (b) computes and displays, for each contract, the
> effective authorized amount as the base authorization plus the sum of
> all amendment changes, together with amount spent and percent spent;
> (c) generates expiry alerts no later than 90, 60, and 30 days before
> each contract's end date, and flags expired contracts; (d) reconciles
> the ledger against the State Contracts Database maintained under
> Neb. Rev. Stat. § 84-602.04 and reports every contract present in agency
> records but missing from the public database, and every material
> difference in authorized amount between the two; and (e) delivers the
> reconciliation report in a machine-readable format on a schedule set by
> the State. All records produced under this section shall preserve a
> complete, inspectable lineage from each base contract through each of
> its amendments.
