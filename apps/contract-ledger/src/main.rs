//! contract-ledger (port 8803)
//!
//! A feeder / reconciler for the Nebraska State Contracts Database
//! (Neb. Rev. Stat. 84-602.04, enacted by LB 429). The app keeps a local,
//! deterministic ledger of state contracts as phext scrolls, overlays
//! amendments (the base contract scroll is never modified), computes the
//! effective authorized amount, and flags two things a reconciler cares
//! about:
//!
//!   * expiry alerts (expired, or expiring within 30 / 60 / 90 days), and
//!   * contracts that are MISSING FROM PUBLIC DATABASE (`in_public_db=no`) —
//!     i.e. contracts that exist in agency records but were never uploaded
//!     to the public State Contracts Database.
//!
//! All demo data is synthetic and generated deterministically with
//! `exo_core::Rng(11)` so every run serves identical numbers.
//!
//! Auth: when the SSO environment is configured the app sits behind the
//! shared edit.phext.io gate (see API-CONTRACT.md); when it is not, the app
//! runs open and every HTML page carries the local-demo banner.

use std::collections::HashMap;
use std::sync::Arc;

use exo_core::http::{esc, Request, Response};
use exo_core::{Rng, Store};
use exo_sso::{Gate, Session, Sso, SsoConfig};

const DEFAULT_PORT: u16 = 8803;
const BASE_SHELF: &str = "1.1.1/1.1.1/5.1";
const AMEND_SHELF: &str = "1.1.1/1.1.1/5.2";

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

/// One base contract, parsed from a phext scroll. The scroll is the record;
/// this struct is only a view over it. Base scrolls are never modified after
/// seeding — changes arrive as amendment scrolls.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Contract {
    pub coord: String,
    pub id: String,
    pub vendor: String,
    pub agency: String,
    /// Authorized amount as written on the base contract, in cents.
    pub authorized: i64,
    /// Amount spent to date, in cents.
    pub spent: i64,
    pub begin_day: i64,
    pub end_day: i64,
    /// Whether the contract appears in the public State Contracts Database.
    pub in_public_db: bool,
}

/// One amendment scroll. `amends` names the base contract's coordinate;
/// `delta_authorized` (cents, may be negative for a de-obligation) adjusts
/// the effective authorized amount without touching the base scroll.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Amendment {
    pub coord: String,
    pub amends: String,
    pub delta_authorized: i64,
    pub note: String,
}

// ---------------------------------------------------------------------------
// Pure logic (unit tested below)
// ---------------------------------------------------------------------------

/// Expiry alert bucket for a contract: `Some("EXPIRED")` when the contract
/// already ended, otherwise the smallest threshold (30, 60, 90 days) that is
/// greater than or equal to `days_left`, or `None` when the contract runs
/// more than 90 days past today.
pub fn alert_bucket(days_left: i64) -> Option<&'static str> {
    if days_left < 0 {
        Some("EXPIRED")
    } else if days_left <= 30 {
        Some("30")
    } else if days_left <= 60 {
        Some("60")
    } else if days_left <= 90 {
        Some("90")
    } else {
        None
    }
}

/// Effective authorized amount: the base authorization plus the sum of every
/// amendment delta. This is the only place the overlay is applied.
pub fn effective_authorized(base: i64, amendments: &[Amendment]) -> i64 {
    base + amendments.iter().map(|a| a.delta_authorized).sum::<i64>()
}

/// Percentage of `authorized` that has been spent. Returns 0.0 for a
/// non-positive authorization instead of dividing by zero.
pub fn spend_pct(spent: i64, authorized: i64) -> f64 {
    if authorized <= 0 {
        0.0
    } else {
        spent as f64 * 100.0 / authorized as f64
    }
}

/// Render cents as a dollar string, e.g. 123456789 -> "$1,234,567.89".
pub fn fmt_money(cents: i64) -> String {
    let sign = if cents < 0 { "-" } else { "" };
    let abs = cents.unsigned_abs();
    let dollars = abs / 100;
    let rem = abs % 100;
    let digits = dollars.to_string();
    let mut grouped = String::new();
    for (i, ch) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            grouped.push(',');
        }
        grouped.push(ch);
    }
    format!("{}${}.{:02}", sign, grouped, rem)
}

/// A ten-cell text progress bar for a percentage, e.g. 42.0 -> "####------".
/// Percentages outside 0..=100 are clamped for display only.
pub fn pct_bar(pct: f64) -> String {
    let filled = (pct / 10.0).round().clamp(0.0, 10.0) as usize;
    let mut bar = "#".repeat(filled);
    bar.push_str(&"-".repeat(10 - filled));
    bar
}

/// Minimal JSON string escaping, local to this app (the workspace has no
/// serde). Escapes quotes, backslash, and control characters.
pub fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

// ---------------------------------------------------------------------------
// Scroll parsing
// ---------------------------------------------------------------------------

/// Parse `key=value` lines from a scroll into a map. Lines without an `=`
/// are ignored; a value may itself contain `=`.
pub fn parse_kv(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

fn kv_i64(kv: &HashMap<String, String>, key: &str) -> i64 {
    kv.get(key).and_then(|v| v.parse::<i64>().ok()).unwrap_or(0)
}

/// Numeric value of the last component of a coordinate (".../5.1.27" -> 27),
/// used to order scrolls numerically rather than lexicographically.
fn coord_tail(coord: &str) -> i64 {
    coord
        .rsplit('.')
        .next()
        .and_then(|t| t.parse::<i64>().ok())
        .unwrap_or(0)
}

pub fn parse_contract(coord: &str, kv: &HashMap<String, String>) -> Option<Contract> {
    let id = kv.get("id")?.clone();
    Some(Contract {
        coord: coord.to_string(),
        id,
        vendor: kv.get("vendor").cloned().unwrap_or_default(),
        agency: kv.get("agency").cloned().unwrap_or_default(),
        authorized: kv_i64(kv, "authorized"),
        spent: kv_i64(kv, "spent"),
        begin_day: kv_i64(kv, "begin_day"),
        end_day: kv_i64(kv, "end_day"),
        in_public_db: kv.get("in_public_db").map(|v| v == "yes").unwrap_or(false),
    })
}

pub fn parse_amendment(coord: &str, kv: &HashMap<String, String>) -> Option<Amendment> {
    let amends = kv.get("amends")?.clone();
    Some(Amendment {
        coord: coord.to_string(),
        amends,
        delta_authorized: kv_i64(kv, "delta_authorized"),
        note: kv.get("note").cloned().unwrap_or_default(),
    })
}

/// Split a store into (contracts, amendments). A scroll is an amendment iff
/// it carries an `amends=` line; a base contract iff it carries `vendor=`.
/// Contracts come back sorted by id; amendments by scroll order.
pub fn load_ledger(store: &Store) -> (Vec<Contract>, Vec<Amendment>) {
    let mut contracts = Vec::new();
    let mut amendments = Vec::new();
    for coord in store.coords() {
        let Some(text) = store.get(&coord) else { continue };
        let kv = parse_kv(text);
        if kv.contains_key("amends") {
            if let Some(a) = parse_amendment(&coord, &kv) {
                amendments.push(a);
            }
        } else if kv.contains_key("vendor") {
            if let Some(c) = parse_contract(&coord, &kv) {
                contracts.push(c);
            }
        }
    }
    contracts.sort_by(|a, b| a.id.cmp(&b.id));
    amendments.sort_by_key(|a| coord_tail(&a.coord));
    (contracts, amendments)
}

/// All amendments that overlay one base contract, in scroll order.
pub fn amendments_for(amendments: &[Amendment], base_coord: &str) -> Vec<Amendment> {
    amendments
        .iter()
        .filter(|a| a.amends == base_coord)
        .cloned()
        .collect()
}

// ---------------------------------------------------------------------------
// Seed data (deterministic, Rng(11))
// ---------------------------------------------------------------------------

const VENDORS: &[&str] = &[
    "Prairie Systems Group",
    "Platte River Software LLC",
    "Sandhills Data Works",
    "Cornhusker Cloud Inc",
    "Loup Valley Consulting",
    "Pioneer Integration Co",
    "Homestead Hosting LLC",
    "Missouri River Analytics",
    "Capitol Web Builders",
    "Great Plains GIS LLC",
    "Niobrara Networks",
    "Elkhorn Office Supply Co",
];

const AGENCIES: &[&str] = &[
    "Office of the Chief Information Officer",
    "Department of Transportation",
    "Department of Health and Human Services",
    "Department of Administrative Services",
    "Department of Natural Resources",
    "Department of Education",
    "State Patrol",
    "Department of Correctional Services",
];

const AMEND_NOTES: &[&str] = &[
    "Extension of term and added scope",
    "Rate adjustment per contract clause",
    "Additional deliverable authorized",
    "Partial de-obligation of unused funds",
    "Scope reduction after phase review",
    "Added hosting and support year",
];

/// Expiry offsets (days relative to `today`) assigned round-robin so the
/// demo always shows every alert state: expired, inside the 30/60/90-day
/// windows, and comfortably far out.
const END_OFFSETS: &[i64] = &[-15, 10, 45, 75, 200, 5, 29, 59, 89, 400];

/// Build the full ledger as a phext `Store`: ~60 base contracts at
/// "1.1.1/1.1.1/5.1.<i>" plus amendment scrolls at "1.1.1/1.1.1/5.2.<j>".
///
/// `today` is a fixed reference day supplied by the caller (the app passes
/// `exo_core::today_epoch_day()`), so end days are generated *relative to
/// today* and the alert spread is identical on every run: some expired,
/// some inside each alert window, some far out.
pub fn seed_store(today: i64) -> Store {
    let mut store = Store::new();
    let mut rng = Rng(11);
    let mut amend_seq: i64 = 0;

    for i in 1..=60i64 {
        let vendor = VENDORS[(rng.below(VENDORS.len() as u64)) as usize];
        let agency = AGENCIES[(rng.below(AGENCIES.len() as u64)) as usize];
        // $25,000.00 .. ~$2,025,000.00 authorized.
        let authorized: i64 = 2_500_000 + rng.below(200_000_000) as i64;
        // 0%..100% spent.
        let spent: i64 = authorized * (rng.below(101) as i64) / 100;
        let duration: i64 = 180 + rng.below(900) as i64;
        let end_day = today + END_OFFSETS[((i - 1) as usize) % END_OFFSETS.len()];
        let begin_day = end_day - duration;
        // Every seventh contract (i % 7 == 3) never reached the public
        // database, so the reconciler flag is exercised on every run.
        let in_public_db = if i % 7 == 3 { "no" } else { "yes" };

        let coord = format!("{}.{}", BASE_SHELF, i);
        let scroll = format!(
            "id=CTR-{:04}\nvendor={}\nagency={}\nauthorized={}\nspent={}\nbegin_day={}\nend_day={}\nin_public_db={}\n",
            i, vendor, agency, authorized, spent, begin_day, end_day, in_public_db
        );
        store.set(&coord, &scroll);

        // Amendments: every third contract has one; every eleventh has a
        // second. Deltas range from a $100k de-obligation to +$400k.
        let n_amends = if i % 11 == 0 {
            2
        } else if i % 3 == 0 {
            1
        } else {
            0
        };
        for _ in 0..n_amends {
            amend_seq += 1;
            let delta: i64 = rng.below(50_000_000) as i64 - 10_000_000;
            let note = AMEND_NOTES[(rng.below(AMEND_NOTES.len() as u64)) as usize];
            let acoord = format!("{}.{}", AMEND_SHELF, amend_seq);
            let ascroll = format!(
                "amends={}\ndelta_authorized={}\nnote={}\n",
                coord, delta, note
            );
            store.set(&acoord, &ascroll);
        }
    }
    store
}

// ---------------------------------------------------------------------------
// App state
// ---------------------------------------------------------------------------

struct App {
    contracts: Vec<Contract>,
    amendments: Vec<Amendment>,
}

impl App {
    fn effective(&self, c: &Contract) -> i64 {
        effective_authorized(c.authorized, &amendments_for(&self.amendments, &c.coord))
    }
}

// ---------------------------------------------------------------------------
// Rendering + JSON
// ---------------------------------------------------------------------------

fn page_shell(title: &str, banner: &str, sess: &Option<&Session>, body: &str) -> String {
    let who = match sess {
        Some(s) => format!("<p>Signed in as {}</p>", esc(&s.handle)),
        None => String::new(),
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>{}</title></head>\n<body>\n<header><h1>Contract Ledger</h1><p>{}</p>{}<nav><a href=\"/\">All contracts</a> | <a href=\"/api/contracts\">API: contracts</a> | <a href=\"/api/alerts\">API: alerts</a></nav></header>\n<main>\n{}\n</main>\n<footer><p>Synthetic demo data. Feeder / reconciler for the State Contracts Database (Neb. Rev. Stat. 84-602.04).</p></footer>\n</body>\n</html>",
        esc(title),
        esc(banner),
        who, body
    )
}

fn alert_badge(bucket: Option<&str>) -> String {
    match bucket {
        Some("EXPIRED") => "<span class=\"badge\">EXPIRED</span>".to_string(),
        Some(b) => format!("<span class=\"badge\">EXPIRES WITHIN {} DAYS</span>", b),
        None => String::new(),
    }
}

fn index_page(app: &App, today: i64, banner: &str, sess: &Option<&Session>) -> Response {
    let mut rows = String::new();
    for c in &app.contracts {
        let eff = app.effective(c);
        let pct = spend_pct(c.spent, eff);
        let days_left = c.end_day - today;
        let mut badges = alert_badge(alert_bucket(days_left));
        if !c.in_public_db {
            badges.push_str(" <span class=\"badge\">MISSING FROM PUBLIC DATABASE</span>");
        }
        rows.push_str(&format!(
            "<tr><td><a href=\"/contract?id={}\">{}</a></td><td>{}</td><td>{}</td><td>{}</td><td>{}</td><td><span aria-label=\"{} percent spent\">[{}] {:.1}%</span></td><td>{}</td><td>{}</td></tr>\n",
            esc(&c.id),
            esc(&c.id),
            esc(&c.vendor),
            esc(&c.agency),
            fmt_money(eff),
            fmt_money(c.spent),
            format!("{:.1}", pct),
            pct_bar(pct),
            pct,
            exo_core::iso_date(c.end_day),
            badges
        ));
    }
    let body = format!(
        "<h2>Contracts ({} total, as of {})</h2>\n<table>\n<thead><tr><th scope=\"col\">ID</th><th scope=\"col\">Vendor</th><th scope=\"col\">Agency</th><th scope=\"col\">Authorized (effective)</th><th scope=\"col\">Spent</th><th scope=\"col\">Spend</th><th scope=\"col\">Ends</th><th scope=\"col\">Flags</th></tr></thead>\n<tbody>\n{}</tbody>\n</table>",
        app.contracts.len(),
        exo_core::iso_date(today),
        rows
    );
    Response::html(page_shell("Contract Ledger", banner, sess, &body))
}

fn detail_page(app: &App, id: &str, today: i64, banner: &str, sess: &Option<&Session>) -> Response {
    let Some(c) = app.contracts.iter().find(|c| c.id == id) else {
        return Response {
            status: 404,
            content_type: "text/html; charset=utf-8".to_string(),
            body: page_shell(
                "Contract not found",
                banner,
                sess,
                "<h2>Contract not found</h2><p>No contract with that id exists in the demo ledger.</p>",
            ),
            headers: Vec::new(),
        };
    };
    let amends = amendments_for(&app.amendments, &c.coord);
    let eff = effective_authorized(c.authorized, &amends);
    let pct = spend_pct(c.spent, eff);
    let days_left = c.end_day - today;
    let mut badges = alert_badge(alert_bucket(days_left));
    if !c.in_public_db {
        badges.push_str(" <span class=\"badge\">MISSING FROM PUBLIC DATABASE</span>");
    }

    let mut lineage = format!(
        "<tr><td>Base contract ({})</td><td>{}</td><td>{}</td></tr>\n",
        esc(&c.coord),
        fmt_money(c.authorized),
        fmt_money(c.authorized)
    );
    let mut running = c.authorized;
    for a in &amends {
        running += a.delta_authorized;
        lineage.push_str(&format!(
            "<tr><td>Amendment ({}) — {}</td><td>{}{}</td><td>{}</td></tr>\n",
            esc(&a.coord),
            esc(&a.note),
            if a.delta_authorized < 0 { "-" } else { "+" },
            fmt_money(a.delta_authorized.abs()),
            fmt_money(running)
        ));
    }

    let body = format!(
        "<h2>Contract {} — {}</h2>\n<p>Agency: {}</p>\n<p>Term: {} to {} ({} days left) {}</p>\n<p>Spent: {} of {} effective authorized ([{}] {:.1}%). In public database: {}.</p>\n<h3>Amendment lineage</h3>\n<p>The base scroll is never modified; the effective authorized amount is the base plus every amendment delta.</p>\n<table>\n<thead><tr><th scope=\"col\">Scroll</th><th scope=\"col\">Delta</th><th scope=\"col\">Running authorized</th></tr></thead>\n<tbody>\n{}</tbody>\n</table>\n<p>Effective authorized: <strong>{}</strong></p>",
        esc(&c.id),
        esc(&c.vendor),
        esc(&c.agency),
        exo_core::iso_date(c.begin_day),
        exo_core::iso_date(c.end_day),
        days_left,
        badges,
        fmt_money(c.spent),
        fmt_money(eff),
        pct_bar(pct),
        pct,
        if c.in_public_db { "yes" } else { "no" },
        lineage,
        fmt_money(eff)
    );
    Response::html(page_shell("Contract detail", banner, sess, &body))
}

fn contract_json(app: &App, c: &Contract, today: i64) -> String {
    let eff = app.effective(c);
    let days_left = c.end_day - today;
    let bucket = match alert_bucket(days_left) {
        Some(b) => format!("\"{}\"", b),
        None => "null".to_string(),
    };
    format!(
        "{{\"id\":\"{}\",\"vendor\":\"{}\",\"agency\":\"{}\",\"coord\":\"{}\",\"authorized_base\":{},\"authorized_effective\":{},\"spent\":{},\"spend_pct\":{:.1},\"begin_day\":{},\"end_day\":{},\"end_date\":\"{}\",\"days_left\":{},\"alert\":{},\"in_public_db\":{}}}",
        json_escape(&c.id),
        json_escape(&c.vendor),
        json_escape(&c.agency),
        json_escape(&c.coord),
        c.authorized,
        eff,
        c.spent,
        spend_pct(c.spent, eff),
        c.begin_day,
        c.end_day,
        exo_core::iso_date(c.end_day),
        days_left,
        bucket,
        c.in_public_db
    )
}

fn contracts_json(app: &App, today: i64) -> Response {
    let items: Vec<String> = app.contracts.iter().map(|c| contract_json(app, c, today)).collect();
    Response::json(format!("{{\"contracts\":[{}]}}", items.join(",")))
}

/// Alerts: only contracts that carry an alert bucket and/or the
/// missing-from-public-database flag.
fn alerts_json(app: &App, today: i64) -> Response {
    let mut items = Vec::new();
    for c in &app.contracts {
        let days_left = c.end_day - today;
        let bucket = alert_bucket(days_left);
        if bucket.is_none() && c.in_public_db {
            continue;
        }
        let bucket_json = match bucket {
            Some(b) => format!("\"{}\"", b),
            None => "null".to_string(),
        };
        items.push(format!(
            "{{\"id\":\"{}\",\"vendor\":\"{}\",\"agency\":\"{}\",\"alert\":{},\"missing_from_public_db\":{},\"days_left\":{},\"end_date\":\"{}\"}}",
            json_escape(&c.id),
            json_escape(&c.vendor),
            json_escape(&c.agency),
            bucket_json,
            !c.in_public_db,
            days_left,
            exo_core::iso_date(c.end_day)
        ));
    }
    Response::json(format!("{{\"alerts\":[{}]}}", items.join(",")))
}

// ---------------------------------------------------------------------------
// Routing + main
// ---------------------------------------------------------------------------

fn route(req: &Request, sess: Option<&Session>, app: &App, banner: &str) -> Response {
    let today = exo_core::today_epoch_day();
    let sess_ref = &sess;
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => index_page(app, today, banner, sess_ref),
        ("GET", "/contract") => {
            let id = req.query.get("id").cloned().unwrap_or_default();
            detail_page(app, &id, today, banner, sess_ref)
        }
        ("GET", "/api/contracts") => contracts_json(app, today),
        ("GET", "/api/alerts") => alerts_json(app, today),
        ("GET", "/health") => Response::text("ok".to_string()),
        _ => Response {
            status: 404,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: "not found".to_string(),
            headers: Vec::new(),
        },
    }
}

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let today = exo_core::today_epoch_day();
    let store = seed_store(today);
    let (contracts, amendments) = load_ledger(&store);
    let app = Arc::new(App {
        contracts,
        amendments,
    });

    let sso = SsoConfig::from_env().map(Sso::new);
    let banner: String = match &sso {
        Some(s) => s.banner_html().to_string(),
        None => "AUTH DISABLED — local demo mode (SSO not configured)".to_string(),
    };

    eprintln!(
        "contract-ledger listening on port {} ({} contracts, {} amendments)",
        port,
        app.contracts.len(),
        app.amendments.len()
    );

    let handler_app = app.clone();
    exo_core::http::serve(port, move |req| {
        if let Some(s) = &sso {
            match s.gate(req) {
                Gate::Respond(r) => return r,
                Gate::Authed(sess) => {
                    return route(req, Some(&sess), &handler_app, &banner)
                }
                Gate::PassThrough => {}
            }
        }
        route(req, None, &handler_app, &banner)
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn amend(delta: i64) -> Amendment {
        Amendment {
            coord: "1.1.1/1.1.1/5.2.1".to_string(),
            amends: "1.1.1/1.1.1/5.1.1".to_string(),
            delta_authorized: delta,
            note: "test".to_string(),
        }
    }

    #[test]
    fn alert_bucket_boundaries() {
        assert_eq!(alert_bucket(-1), Some("EXPIRED"));
        assert_eq!(alert_bucket(-400), Some("EXPIRED"));
        assert_eq!(alert_bucket(0), Some("30"));
        assert_eq!(alert_bucket(30), Some("30"));
        assert_eq!(alert_bucket(31), Some("60"));
        assert_eq!(alert_bucket(60), Some("60"));
        assert_eq!(alert_bucket(61), Some("90"));
        assert_eq!(alert_bucket(90), Some("90"));
        assert_eq!(alert_bucket(91), None);
        assert_eq!(alert_bucket(10_000), None);
    }

    #[test]
    fn effective_authorized_sums_deltas() {
        assert_eq!(effective_authorized(1_000_00, &[]), 1_000_00);
        assert_eq!(
            effective_authorized(1_000_00, &[amend(250_00), amend(-50_00)]),
            1_200_00
        );
        assert_eq!(effective_authorized(0, &[amend(-1)]), -1);
    }

    #[test]
    fn spend_pct_guards_zero() {
        assert_eq!(spend_pct(0, 0), 0.0);
        assert_eq!(spend_pct(50, 0), 0.0);
        assert_eq!(spend_pct(50, -10), 0.0);
        assert!((spend_pct(1, 4) - 25.0).abs() < f64::EPSILON);
        assert!((spend_pct(3, 2) - 150.0).abs() < f64::EPSILON);
    }

    #[test]
    fn money_formatting() {
        assert_eq!(fmt_money(0), "$0.00");
        assert_eq!(fmt_money(99), "$0.99");
        assert_eq!(fmt_money(123_456_789), "$1,234,567.89");
        assert_eq!(fmt_money(-5_050), "-$50.50");
    }

    #[test]
    fn bar_clamps() {
        assert_eq!(pct_bar(0.0), "----------");
        assert_eq!(pct_bar(100.0), "##########");
        assert_eq!(pct_bar(150.0), "##########");
        assert_eq!(pct_bar(-5.0), "----------");
        assert_eq!(pct_bar(42.0), "####------");
    }

    #[test]
    fn json_escape_covers_quotes_and_controls() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        assert_eq!(json_escape("plain"), "plain");
    }

    #[test]
    fn seed_is_deterministic_and_complete() {
        let today = 20_000;
        let s1 = seed_store(today);
        let s2 = seed_store(today);
        assert_eq!(s1.to_phext(), s2.to_phext(), "Rng(11) seed must reproduce");

        let (contracts, amendments) = load_ledger(&s1);
        assert_eq!(contracts.len(), 60);
        assert!(!amendments.is_empty());
        for c in &contracts {
            assert!(exo_core::valid_coord(&c.coord), "bad coord {}", c.coord);
            assert!(c.coord.starts_with("1.1.1/1.1.1/5.1."));
        }
        for a in &amendments {
            assert!(contracts.iter().any(|c| c.coord == a.amends));
        }
        // The reconciler flag and every alert bucket must be exercised.
        assert!(contracts.iter().any(|c| !c.in_public_db));
        let buckets: Vec<Option<&str>> = contracts
            .iter()
            .map(|c| alert_bucket(c.end_day - today))
            .collect();
        assert!(buckets.contains(&Some("EXPIRED")));
        assert!(buckets.contains(&Some("30")));
        assert!(buckets.contains(&Some("60")));
        assert!(buckets.contains(&Some("90")));
        assert!(buckets.contains(&None));
    }

    #[test]
    fn seed_reference_day_shifts_end_days() {
        let (a, _) = load_ledger(&seed_store(20_000));
        let (b, _) = load_ledger(&seed_store(20_100));
        for (x, y) in a.iter().zip(b.iter()) {
            assert_eq!(x.id, y.id);
            assert_eq!(y.end_day - x.end_day, 100);
            assert_eq!(x.authorized, y.authorized, "amounts stay fixed");
        }
    }
}
