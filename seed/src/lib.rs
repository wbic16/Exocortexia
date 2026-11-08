//! exo-seed: deterministic synthetic corpora for the Exocortexia suite.
//!
//! Every corpus is generated with a fixed-seed `exo_core::Rng`, so two calls
//! with the same arguments always produce byte-identical phext. All data is
//! synthetic: fictional people, vendors, and boards; public topics only; no
//! real personal information anywhere.
//!
//! Coordinate layout inside each returned `Store` (coordinates are internal
//! to their own phext, per phext convention):
//! - records:   `1.1.1/1.1.1/1.1.<i>`            (i starts at 1)
//! - minutes:   `1.1.1/1.1.1/2.<board>.<seq>`    (board, seq start at 1)
//! - contracts: `1.1.1/1.1.1/3.1.<i>` contracts, `1.1.1/1.1.1/3.2.<i>` amendments
//! - catalog:   `1.1.1/1.1.1/4.1.<k>` datasets,  `1.1.1/1.1.1/5.<k>.<f>` fields

use exo_core::{Rng, Store};

/// Fixed base epoch day for the minutes corpus (2023-03-28). Anchoring to a
/// constant keeps the corpus identical across runs and machines.
const MINUTES_BASE_DAY: i64 = 19_000;

/// Roughly 15% of record docs mention InterRAI (see `records_corpus`).
const INTERRAI_PERCENT: u64 = 15;

/// Roughly 20% of contracts get an amendment scroll (see `contracts_corpus`).
const AMENDMENT_PERCENT: u64 = 20;

/// Fictional staff names used as record senders/recipients.
const PEOPLE: &[&str] = &[
    "Alex Prairie",
    "Jordan Cedar",
    "Robin Sandhill",
    "Casey Platte",
    "Taylor Loess",
    "Morgan Bison",
    "Jamie Crane",
    "Riley Meadow",
    "Avery Homestead",
    "Quinn Trailhead",
];

/// Fictional office inboxes.
const OFFICES: &[&str] = &[
    "records@agency.example.ne.gov",
    "publicaffairs@agency.example.ne.gov",
    "facilities@agency.example.ne.gov",
    "itservices@agency.example.ne.gov",
    "procurement@agency.example.ne.gov",
    "clerks@agency.example.ne.gov",
];

/// Ordinary public-business subjects (index-aligned with `BODIES`).
const SUBJECTS: &[&str] = &[
    "Bridge maintenance schedule for the county road crew",
    "Public meeting notes: regional health services planning",
    "Snow removal contract status update",
    "Courthouse elevator inspection report",
    "Records digitization project milestone",
    "Public comment summary: parks improvement plan",
    "Fleet vehicle replacement timeline",
    "Open data portal uptime report",
    "Rural broadband grant application progress",
    "Water quality testing results posted",
    "Road resurfacing bids opening announcement",
    "Community center roof repair estimate",
];

/// Body text paired with `SUBJECTS`.
const BODIES: &[&str] = &[
    "The county road crew will begin scheduled bridge maintenance next month. \
     Detour signs will be posted at both approaches, and the public notice has \
     been filed with the clerk. Questions from residents can be directed to the \
     facilities office during regular business hours.",
    "Notes from the public planning meeting are attached for the record. The \
     committee reviewed service capacity, staffing vacancies, and the draft \
     budget for the coming fiscal year. All materials presented are public and \
     available on request.",
    "The snow removal contractor confirmed equipment readiness for the season. \
     Routes and priority streets are unchanged from last year. The signed \
     agreement is on file with the procurement office.",
    "The annual courthouse elevator inspection is complete and the unit passed \
     all checks. The inspector's certificate has been posted by the main \
     entrance as required.",
    "The digitization team scanned and indexed another batch of historical \
     ledgers this week. Scanned pages are being verified against the originals \
     before the originals return to the vault.",
    "Public comments received on the parks improvement plan have been \
     summarized and posted. The most frequent topics were trail lighting, \
     playground surfacing, and weekend hours.",
    "Fleet services published the vehicle replacement timeline for the next \
     two budget cycles. Vehicles are replaced on mileage and condition, and \
     the full list is available to any resident on request.",
    "The open data portal was available for the full reporting period. Two \
     datasets were refreshed and one new dataset was published. No incidents \
     required downtime.",
    "Staff submitted the quarterly progress report for the rural broadband \
     grant. Survey work is complete in two townships and construction bidding \
     opens next quarter.",
    "Monthly water quality testing results are posted at the utility office \
     and on the public bulletin board. All samples were within normal ranges \
     for the period.",
    "Sealed bids for the spring road resurfacing program will be opened in \
     public session at the courthouse annex. The bid tabulation will be \
     posted the same day.",
    "Facilities received the roof repair estimate for the community center. \
     The estimate and the engineer's letter are public documents and are \
     attached to this memo for the file.",
];

/// Sentence appended to the ~15% of record docs that mention InterRAI.
const INTERRAI_SENTENCE: &str = "This item references the InterRAI assessment \
    instrument and the public InterRAI training session announced for next \
    quarter; the InterRAI summary handout is a public document.";

/// Build `n` synthetic public-records documents (memos and emails about
/// public topics) at coords `1.1.1/1.1.1/1.1.<i+1>`.
///
/// Each document is a header block followed by body text:
///
/// ```text
/// from=<person or office>
/// to=<person or office>
/// date=<epoch day>
/// subject=<subject line>
///
/// <body>
/// ```
///
/// Roughly 15% of the documents mention InterRAI. Deterministic: the corpus
/// is driven by `Rng(42)`, so repeated calls return identical stores.
pub fn records_corpus(n: usize) -> Store {
    let mut rng = Rng(42);
    let mut store = Store::new();
    for i in 0..n {
        let from = PEOPLE[rng.below(PEOPLE.len() as u64) as usize];
        let to = if rng.below(2) == 0 {
            PEOPLE[rng.below(PEOPLE.len() as u64) as usize]
        } else {
            OFFICES[rng.below(OFFICES.len() as u64) as usize]
        };
        let topic = rng.below(SUBJECTS.len() as u64) as usize;
        let date = 19_000 + rng.below(700) as i64;
        let mentions_interrai = rng.below(100) < INTERRAI_PERCENT;

        let mut doc = String::new();
        doc.push_str(&format!("from={from}\n"));
        doc.push_str(&format!("to={to}\n"));
        doc.push_str(&format!("date={date}\n"));
        if mentions_interrai {
            doc.push_str(&format!(
                "subject={} (InterRAI reference)\n",
                SUBJECTS[topic]
            ));
        } else {
            doc.push_str(&format!("subject={}\n", SUBJECTS[topic]));
        }
        doc.push('\n');
        doc.push_str(BODIES[topic]);
        if mentions_interrai {
            doc.push(' ');
            doc.push_str(INTERRAI_SENTENCE);
        }
        doc.push('\n');

        let coord = format!("1.1.1/1.1.1/1.1.{}", i + 1);
        store.set(&coord, &doc);
    }
    store
}

/// Place names used to compose fictional board names.
const PLACES: &[&str] = &[
    "Cedar Bluff",
    "Prairie City",
    "Sandhill",
    "Platte River",
    "Loess Hills",
    "Bison Creek",
    "Crane Valley",
    "Meadow Grove",
    "Homestead",
    "Trailhead",
    "Juniper",
    "Elkhorn",
];

/// Fictional public board types.
const BOARD_TYPES: &[&str] = &[
    "County Board of Supervisors",
    "School Board",
    "Water District Board",
    "Planning Commission",
    "Library Board",
    "Airport Authority",
    "Public Power District Board",
    "Village Board of Trustees",
];

/// Build meeting records for `boards` boards over `years` years, four
/// meetings per board per year (quarterly), one scroll per meeting at
/// `1.1.1/1.1.1/2.<board>.<seq>`.
///
/// Each scroll is a set of `key=value` lines:
///
/// ```text
/// board=<board name>
/// meeting_day=<epoch day>
/// agenda_posted_day=<epoch day, before meeting_day>
/// minutes_posted_day=<epoch day, after meeting_day>
/// vote=<ayes>-<nays>
/// ```
///
/// Meeting days are anchored to the fixed base day 19000 plus deterministic
/// offsets (year * 365 + quarter * 91 + a small per-board offset), so the
/// corpus is identical on every run.
pub fn minutes_corpus(boards: usize, years: usize) -> Store {
    let mut rng = Rng(7);
    let mut store = Store::new();
    for b in 0..boards {
        let place = PLACES[b % PLACES.len()];
        let kind = BOARD_TYPES[(b / PLACES.len()) % BOARD_TYPES.len()];
        let board_name = format!("{place} {kind} {}", b + 1);
        for y in 0..years {
            for q in 0..4usize {
                let seq = y * 4 + q;
                let meeting_day =
                    MINUTES_BASE_DAY + (y as i64) * 365 + (q as i64) * 91 + (b % 27) as i64;
                let agenda_posted_day = meeting_day - (3 + rng.below(7) as i64);
                let minutes_posted_day = meeting_day + (1 + rng.below(13) as i64);
                let ayes = 3 + rng.below(5);
                let nays = rng.below(3);

                let mut rec = String::new();
                rec.push_str(&format!("board={board_name}\n"));
                rec.push_str(&format!("meeting_day={meeting_day}\n"));
                rec.push_str(&format!("agenda_posted_day={agenda_posted_day}\n"));
                rec.push_str(&format!("minutes_posted_day={minutes_posted_day}\n"));
                rec.push_str(&format!("vote={ayes}-{nays}\n"));

                let coord = format!("1.1.1/1.1.1/2.{}.{}", b + 1, seq + 1);
                store.set(&coord, &rec);
            }
        }
    }
    store
}

/// Fictional vendor names for the contracts corpus.
const VENDORS: &[&str] = &[
    "Prairie Systems LLC",
    "Heartland Data Works",
    "Sandhill Consulting Group",
    "Platte Valley Builders Inc",
    "Loess Analytics Co",
    "Bison Creek Software",
    "Crane & Associates",
    "Meadowlark Office Supply",
    "Homestead Fleet Services",
    "Elkhorn Security Group",
    "Juniper Health Services",
    "Trailmark Engineering",
];

/// Public agency names for the contracts corpus (synthetic contracts; the
/// contracts themselves are invented).
const AGENCIES: &[&str] = &[
    "Department of Transportation",
    "Department of Health and Human Services",
    "Department of Administrative Services",
    "Department of Education",
    "Department of Natural Resources",
    "Department of Correctional Services",
    "Game and Parks Commission",
    "State Patrol",
];

/// Build `n` synthetic state contracts at `1.1.1/1.1.1/3.1.<i+1>`, dated
/// relative to `today_day` (an epoch day, e.g. `exo_core::today_epoch_day()`).
///
/// Each contract scroll is a set of `key=value` lines:
///
/// ```text
/// id=CON-000001
/// vendor=<fictional vendor>
/// agency=<agency>
/// authorized=<dollars>
/// spent=<dollars, never more than authorized>
/// begin_day=<epoch day>
/// end_day=<epoch day, after begin_day>
/// in_public_db=true|false
/// ```
///
/// Roughly 20% of contracts also get an amendment scroll at
/// `1.1.1/1.1.1/3.2.<i+1>` carrying an `amends=<contract id>` line, the
/// amended authorized total, and an effective day inside the contract term.
/// Deterministic for a given `today_day` (driven by `Rng(1234)`).
pub fn contracts_corpus(n: usize, today_day: i64) -> Store {
    let mut rng = Rng(1234);
    let mut store = Store::new();
    for i in 0..n {
        let id = format!("CON-{:06}", i + 1);
        let vendor = VENDORS[rng.below(VENDORS.len() as u64) as usize];
        let agency = AGENCIES[rng.below(AGENCIES.len() as u64) as usize];
        let authorized = 10_000 + rng.below(990_000);
        let spent = rng.below(authorized + 1);
        let begin_day = today_day - rng.below(1000) as i64;
        let end_day = begin_day + 90 + rng.below(1005) as i64;
        let in_public_db = rng.below(100) < 80;

        let mut rec = String::new();
        rec.push_str(&format!("id={id}\n"));
        rec.push_str(&format!("vendor={vendor}\n"));
        rec.push_str(&format!("agency={agency}\n"));
        rec.push_str(&format!("authorized={authorized}\n"));
        rec.push_str(&format!("spent={spent}\n"));
        rec.push_str(&format!("begin_day={begin_day}\n"));
        rec.push_str(&format!("end_day={end_day}\n"));
        rec.push_str(&format!("in_public_db={in_public_db}\n"));

        let coord = format!("1.1.1/1.1.1/3.1.{}", i + 1);
        store.set(&coord, &rec);

        if rng.below(100) < AMENDMENT_PERCENT {
            let new_authorized = authorized + 1_000 + rng.below(100_000);
            let effective_day = begin_day + rng.below(60) as i64;
            let mut amd = String::new();
            amd.push_str(&format!("id={id}-A1\n"));
            amd.push_str(&format!("amends={id}\n"));
            amd.push_str(&format!("amends_coord={coord}\n"));
            amd.push_str(&format!("authorized={new_authorized}\n"));
            amd.push_str(&format!("effective_day={effective_day}\n"));
            let amd_coord = format!("1.1.1/1.1.1/3.2.{}", i + 1);
            store.set(&amd_coord, &amd);
        }
    }
    store
}

/// One field of a catalog dataset.
struct FieldSpec {
    name: &'static str,
    definition: &'static str,
    ty: &'static str,
    provenance: &'static str,
}

/// One catalog dataset and its fields.
struct DatasetSpec {
    name: &'static str,
    owner: &'static str,
    cadence_days: u64,
    last_updated_day: i64,
    source_url: &'static str,
    fields: &'static [FieldSpec],
}

/// The eight synthetic catalog datasets. Fixed data: no RNG, no clock, so
/// `catalog_corpus()` is trivially deterministic. `source_url` values use
/// the reserved `example.org` domain — they are placeholders, not real links.
const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        name: "Public Records Request Log",
        owner: "Agency Records Office",
        cadence_days: 7,
        last_updated_day: 19_812,
        source_url: "https://opendata.example.org/datasets/records-request-log",
        fields: &[
            FieldSpec {
                name: "request_id",
                definition: "Unique identifier assigned when a public records request is received.",
                ty: "string",
                provenance: "records-desk intake form",
            },
            FieldSpec {
                name: "received_day",
                definition: "Epoch day the request was received by the agency.",
                ty: "integer (epoch day)",
                provenance: "records-desk intake form",
            },
            FieldSpec {
                name: "status",
                definition: "Current disposition: open, fulfilled, partially fulfilled, denied, or withdrawn.",
                ty: "string (enum)",
                provenance: "records officer review",
            },
            FieldSpec {
                name: "fee_cents",
                definition: "Total copying and search fees charged, in cents.",
                ty: "integer (cents)",
                provenance: "agency fee schedule",
            },
        ],
    },
    DatasetSpec {
        name: "Board Meeting Minutes Index",
        owner: "Clerk of the Legislature (demo)",
        cadence_days: 30,
        last_updated_day: 19_805,
        source_url: "https://opendata.example.org/datasets/minutes-index",
        fields: &[
            FieldSpec {
                name: "board",
                definition: "Name of the public board that held the meeting.",
                ty: "string",
                provenance: "board clerk filing",
            },
            FieldSpec {
                name: "meeting_day",
                definition: "Epoch day the meeting was held.",
                ty: "integer (epoch day)",
                provenance: "board clerk filing",
            },
            FieldSpec {
                name: "agenda_posted_day",
                definition: "Epoch day the agenda was publicly posted before the meeting.",
                ty: "integer (epoch day)",
                provenance: "board clerk filing",
            },
            FieldSpec {
                name: "minutes_posted_day",
                definition: "Epoch day the approved minutes were publicly posted.",
                ty: "integer (epoch day)",
                provenance: "board clerk filing",
            },
        ],
    },
    DatasetSpec {
        name: "State Contract Ledger",
        owner: "Department of Administrative Services (demo)",
        cadence_days: 14,
        last_updated_day: 19_799,
        source_url: "https://opendata.example.org/datasets/contract-ledger",
        fields: &[
            FieldSpec {
                name: "contract_id",
                definition: "Unique identifier of the contract, including amendments.",
                ty: "string",
                provenance: "procurement office",
            },
            FieldSpec {
                name: "authorized",
                definition: "Maximum amount authorized under the contract, in whole dollars.",
                ty: "integer (dollars)",
                provenance: "signed contract document",
            },
            FieldSpec {
                name: "spent",
                definition: "Amount paid out under the contract to date, in whole dollars.",
                ty: "integer (dollars)",
                provenance: "accounting system extract",
            },
            FieldSpec {
                name: "in_public_db",
                definition: "Whether the contract also appears in the legacy public contracts database.",
                ty: "boolean",
                provenance: "cross-check against legacy database",
            },
        ],
    },
    DatasetSpec {
        name: "Agency Open Data Inventory",
        owner: "Office of the CIO (demo)",
        cadence_days: 90,
        last_updated_day: 19_790,
        source_url: "https://opendata.example.org/datasets/agency-inventory",
        fields: &[
            FieldSpec {
                name: "dataset_name",
                definition: "Published title of the dataset.",
                ty: "string",
                provenance: "agency data steward",
            },
            FieldSpec {
                name: "cadence_days",
                definition: "Promised refresh interval for the dataset, in days.",
                ty: "integer (days)",
                provenance: "agency data steward",
            },
            FieldSpec {
                name: "last_updated_day",
                definition: "Epoch day the dataset was most recently refreshed.",
                ty: "integer (epoch day)",
                provenance: "publishing pipeline log",
            },
        ],
    },
    DatasetSpec {
        name: "Public Meeting Notices",
        owner: "Secretary of State (demo)",
        cadence_days: 1,
        last_updated_day: 19_819,
        source_url: "https://opendata.example.org/datasets/meeting-notices",
        fields: &[
            FieldSpec {
                name: "body_name",
                definition: "Name of the public body issuing the notice.",
                ty: "string",
                provenance: "notice filing",
            },
            FieldSpec {
                name: "notice_day",
                definition: "Epoch day the notice was published.",
                ty: "integer (epoch day)",
                provenance: "notice filing",
            },
            FieldSpec {
                name: "location",
                definition: "Street address or public description of the meeting location.",
                ty: "string",
                provenance: "notice filing",
            },
        ],
    },
    DatasetSpec {
        name: "State Forms Catalog",
        owner: "Department of Administrative Services (demo)",
        cadence_days: 30,
        last_updated_day: 19_780,
        source_url: "https://opendata.example.org/datasets/forms-catalog",
        fields: &[
            FieldSpec {
                name: "form_number",
                definition: "Official form number printed on the form.",
                ty: "string",
                provenance: "forms management office",
            },
            FieldSpec {
                name: "owning_agency",
                definition: "Agency responsible for the form's content and revisions.",
                ty: "string",
                provenance: "forms management office",
            },
            FieldSpec {
                name: "revision_day",
                definition: "Epoch day of the form's current revision.",
                ty: "integer (epoch day)",
                provenance: "forms management office",
            },
        ],
    },
    DatasetSpec {
        name: "Public Salary Bands",
        owner: "Department of Administrative Services (demo)",
        cadence_days: 180,
        last_updated_day: 19_760,
        source_url: "https://opendata.example.org/datasets/salary-bands",
        fields: &[
            FieldSpec {
                name: "classification",
                definition: "Job classification title; bands are published without individual names.",
                ty: "string",
                provenance: "personnel division",
            },
            FieldSpec {
                name: "band_min",
                definition: "Bottom of the published pay band, in whole dollars per year.",
                ty: "integer (dollars)",
                provenance: "personnel division",
            },
            FieldSpec {
                name: "band_max",
                definition: "Top of the published pay band, in whole dollars per year.",
                ty: "integer (dollars)",
                provenance: "personnel division",
            },
        ],
    },
    DatasetSpec {
        name: "Road Project Status",
        owner: "Department of Transportation (demo)",
        cadence_days: 14,
        last_updated_day: 19_795,
        source_url: "https://opendata.example.org/datasets/road-projects",
        fields: &[
            FieldSpec {
                name: "project_id",
                definition: "Unique identifier of the road construction or maintenance project.",
                ty: "string",
                provenance: "project management system",
            },
            FieldSpec {
                name: "county",
                definition: "County where the project is located.",
                ty: "string",
                provenance: "project management system",
            },
            FieldSpec {
                name: "percent_complete",
                definition: "Engineer's estimate of completion, 0 to 100.",
                ty: "integer (percent)",
                provenance: "resident engineer report",
            },
        ],
    },
];

/// Build the synthetic open-data catalog: 8 dataset scrolls at
/// `1.1.1/1.1.1/4.1.<k+1>` plus one scroll per field at
/// `1.1.1/1.1.1/5.<k+1>.<f+1>`.
///
/// Dataset scroll lines:
///
/// ```text
/// name=<dataset name>
/// owner=<owning office>
/// cadence_days=<refresh interval>
/// last_updated_day=<epoch day>
/// source_url=<placeholder URL>
/// ```
///
/// Field scroll lines:
///
/// ```text
/// dataset=<dataset name>
/// field=<field name>
/// definition=<plain-language definition>
/// type=<data type>
/// provenance=<where the values come from>
/// ```
pub fn catalog_corpus() -> Store {
    let mut store = Store::new();
    for (k, ds) in DATASETS.iter().enumerate() {
        let mut rec = String::new();
        rec.push_str(&format!("name={}\n", ds.name));
        rec.push_str(&format!("owner={}\n", ds.owner));
        rec.push_str(&format!("cadence_days={}\n", ds.cadence_days));
        rec.push_str(&format!("last_updated_day={}\n", ds.last_updated_day));
        rec.push_str(&format!("source_url={}\n", ds.source_url));
        let coord = format!("1.1.1/1.1.1/4.1.{}", k + 1);
        store.set(&coord, &rec);

        for (f, field) in ds.fields.iter().enumerate() {
            let mut frec = String::new();
            frec.push_str(&format!("dataset={}\n", ds.name));
            frec.push_str(&format!("field={}\n", field.name));
            frec.push_str(&format!("definition={}\n", field.definition));
            frec.push_str(&format!("type={}\n", field.ty));
            frec.push_str(&format!("provenance={}\n", field.provenance));
            let fcoord = format!("1.1.1/1.1.1/5.{}.{}", k + 1, f + 1);
            store.set(&fcoord, &frec);
        }
    }
    store
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn records_size() {
        assert_eq!(records_corpus(10).len(), 10);
        assert_eq!(records_corpus(0).len(), 0);
    }

    #[test]
    fn records_deterministic() {
        let a = records_corpus(50).to_phext();
        let b = records_corpus(50).to_phext();
        assert_eq!(a, b);
    }

    #[test]
    fn records_have_headers_and_some_interrai() {
        let store = records_corpus(200);
        let first = store.get("1.1.1/1.1.1/1.1.1").expect("first doc");
        assert!(first.starts_with("from="));
        assert!(first.contains("\nto="));
        assert!(first.contains("\ndate="));
        assert!(first.contains("\nsubject="));
        let hits = store
            .coords()
            .iter()
            .filter(|c| {
                store
                    .get(c)
                    .map(|t| t.contains("InterRAI"))
                    .unwrap_or(false)
            })
            .count();
        assert!(hits > 0, "expected some InterRAI mentions in 200 docs");
        assert!(
            hits < 100,
            "InterRAI should be a minority (~15%), got {hits}"
        );
    }

    #[test]
    fn minutes_size_and_shape() {
        let store = minutes_corpus(3, 2);
        assert_eq!(store.len(), 3 * 2 * 4);
        let first = store.get("1.1.1/1.1.1/2.1.1").expect("first meeting");
        assert!(first.contains("board="));
        assert!(first.contains("meeting_day="));
        assert!(first.contains("agenda_posted_day="));
        assert!(first.contains("minutes_posted_day="));
        assert!(first.contains("vote="));
    }

    #[test]
    fn minutes_deterministic() {
        assert_eq!(
            minutes_corpus(5, 3).to_phext(),
            minutes_corpus(5, 3).to_phext()
        );
    }

    #[test]
    fn contracts_size_and_amendments() {
        let store = contracts_corpus(200, 19_900);
        let contracts = store
            .coords()
            .iter()
            .filter(|c| c.starts_with("1.1.1/1.1.1/3.1."))
            .count();
        assert_eq!(contracts, 200);
        let amendments = store
            .coords()
            .iter()
            .filter(|c| c.starts_with("1.1.1/1.1.1/3.2."))
            .count();
        assert!(amendments > 0, "expected ~20% amendments in 200 contracts");
        assert!(
            amendments < 100,
            "amendments should be a minority, got {amendments}"
        );
        let first_amd_coord = store
            .coords()
            .into_iter()
            .find(|c| c.starts_with("1.1.1/1.1.1/3.2."))
            .expect("an amendment");
        let amd = store.get(&first_amd_coord).expect("amendment text");
        assert!(amd.contains("amends=CON-"));
        let first = store.get("1.1.1/1.1.1/3.1.1").expect("first contract");
        assert!(first.contains("in_public_db="));
    }

    #[test]
    fn contracts_deterministic_for_same_today() {
        assert_eq!(
            contracts_corpus(50, 19_900).to_phext(),
            contracts_corpus(50, 19_900).to_phext()
        );
    }

    #[test]
    fn catalog_shape() {
        let store = catalog_corpus();
        let datasets = store
            .coords()
            .iter()
            .filter(|c| {
                store
                    .get(c)
                    .map(|t| t.starts_with("name="))
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(datasets, 8);
        let fields = store
            .coords()
            .iter()
            .filter(|c| {
                store
                    .get(c)
                    .map(|t| t.starts_with("dataset="))
                    .unwrap_or(false)
            })
            .count();
        assert!(fields >= 8, "each dataset should have fields, got {fields}");
        assert_eq!(store.len(), 8 + fields);
        let first = store.get("1.1.1/1.1.1/4.1.1").expect("first dataset");
        assert!(first.contains("owner="));
        assert!(first.contains("cadence_days="));
        assert!(first.contains("last_updated_day="));
        assert!(first.contains("source_url="));
    }
}
