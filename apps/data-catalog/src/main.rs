//! data-catalog (port 8804)
//!
//! A small data-governance catalog demo: eight synthetic Nebraska-flavoured
//! datasets, each described by a phext dataset scroll (name, owner, update
//! cadence, last-updated day, source URL) and one field scroll per field
//! (field name, definition, type, provenance). The catalog answers the two
//! questions a data steward asks first:
//!
//!   * Is this dataset **stale**? (today - last_updated_day > cadence_days)
//!   * Does its source link still **answer**? (checked on demand only —
//!     see `check_url`)
//!
//! Honesty note: the need for a catalog like this at OCIO is a *hypothesis*
//! (see docs/data-catalog-statute.md). The demo proves the mechanics, not
//! the demand.
//!
//! Auth: SSO gate per API-CONTRACT.md when configured; open local demo
//! mode with a banner when it is not.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::Arc;
use std::time::Duration;

use exo_core::http::{esc, Request, Response};
use exo_core::Store;
use exo_sso::{Gate, Session, Sso, SsoConfig};

const DEFAULT_PORT: u16 = 8804;

// ---------------------------------------------------------------------------
// Data model
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Dataset {
    pub coord: String,
    pub name: String,
    pub owner: String,
    pub cadence_days: i64,
    pub last_updated_day: i64,
    pub source_url: String,
    pub fields: Vec<FieldDef>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldDef {
    pub dataset: String,
    pub field: String,
    pub definition: String,
    pub type_name: String,
    pub provenance: String,
}

// ---------------------------------------------------------------------------
// Pure logic (unit tested below)
// ---------------------------------------------------------------------------

/// A dataset is stale when more days have passed since its last update
/// than its declared cadence allows. Exactly at cadence is still fresh.
pub fn is_stale(last_updated_day: i64, cadence_days: i64, today: i64) -> bool {
    today - last_updated_day > cadence_days
}

/// SSRF guard (F-21): is this IP one the link checker must never
/// connect to? Blocked: private ranges, link-local (including the
/// cloud metadata address 169.254.169.254), CGNAT shared space,
/// multicast, and unspecified — for IPv6 also unique-local and
/// link-local. Loopback is allowed: the demo's own datasets point at
/// loopback by design (this app's `/health`, the discard port), and
/// loopback reaches only the host the app already runs on.
pub fn ip_blocked(ip: &std::net::IpAddr) -> bool {
    match ip {
        std::net::IpAddr::V4(v4) => {
            if v4.is_loopback() {
                return false;
            }
            let octets = v4.octets();
            v4.is_private()
                || v4.is_link_local()
                || v4.is_multicast()
                || v4.is_unspecified()
                || v4.is_broadcast()
                || (octets[0] == 100 && (octets[1] & 0xC0) == 64)
        }
        std::net::IpAddr::V6(v6) => {
            if v6.is_loopback() {
                return false;
            }
            let seg0 = v6.segments()[0];
            v6.is_multicast()
                || v6.is_unspecified()
                || (seg0 & 0xFFC0) == 0xFE80
                || (seg0 & 0xFE00) == 0xFC00
        }
    }
}

/// Check whether an http:// URL answers, using only std networking.
///
/// Behaviour, honestly stated:
///   * `https://` URLs are NOT checked: returns
///     `(false, "https not checked in demo")`.
///   * Other non-http schemes return `(false, "unsupported scheme ...")`.
///   * SSRF guard: if the host resolves to *any* non-public address
///     (see `ip_blocked`), the check is refused before connecting —
///     this is what stops the checker being aimed at LAN services or
///     the cloud metadata endpoint if dataset URLs ever become data
///     rather than compiled-in seed values.
///   * The host is resolved with `ToSocketAddrs`, connected with a 5-second
///     `connect_timeout`, and sent a minimal `GET` request. Read/write
///     timeouts are also 5 seconds.
///   * Returns `(true, status_line)` for a 2xx/3xx status line,
///     `(false, status_line)` for other HTTP answers, and
///     `(false, "<what went wrong>")` when no HTTP answer arrives.
pub fn check_url(url: &str) -> (bool, String) {
    if url.starts_with("https://") {
        return (false, "https not checked in demo".to_string());
    }
    let Some(rest) = url.strip_prefix("http://") else {
        return (false, "unsupported scheme (http only in demo)".to_string());
    };
    // Split host[:port] from path.
    let (hostport, path) = match rest.find('/') {
        Some(idx) => (&rest[..idx], &rest[idx..]),
        None => (rest, "/"),
    };
    if hostport.is_empty() {
        return (false, "missing host".to_string());
    }
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => match p.parse::<u16>() {
            Ok(p) => (h, p),
            Err(_) => return (false, format!("bad port in url: {}", hostport)),
        },
        None => (hostport, 80),
    };

    let addr_str = format!("{}:{}", host, port);
    let resolved: Vec<std::net::SocketAddr> = match addr_str.to_socket_addrs() {
        Ok(a) => a.collect(),
        Err(e) => return (false, format!("cannot resolve {}: {}", addr_str, e)),
    };
    let Some(addr) = resolved.first().copied() else {
        return (false, format!("cannot resolve {}", addr_str));
    };
    // Check *every* resolved address, not just the one we connect to:
    // a name that mixes public and non-public answers is refused.
    if resolved.iter().any(|a| ip_blocked(&a.ip())) {
        return (
            false,
            "refused: host resolves to a non-public address (SSRF guard)".to_string(),
        );
    }

    let mut stream = match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
        Ok(s) => s,
        Err(e) => return (false, format!("connect failed: {}", e)),
    };
    let _ = stream.set_read_timeout(Some(Duration::from_secs(5)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));

    let request = format!(
        "GET {} HTTP/1.1\r\nHost: {}\r\nConnection: close\r\nUser-Agent: data-catalog-demo\r\n\r\n",
        path, hostport
    );
    if let Err(e) = stream.write_all(request.as_bytes()) {
        return (false, format!("write failed: {}", e));
    }

    let mut reader = BufReader::new(stream);
    let mut status_line = String::new();
    match reader.read_line(&mut status_line) {
        Ok(0) | Err(_) if status_line.is_empty() => {
            (false, "no response from server".to_string())
        }
        Ok(_) => {
            let line = status_line.trim().to_string();
            // Status line shape: "HTTP/1.1 200 OK"
            let code: Option<u32> = line
                .split_whitespace()
                .nth(1)
                .and_then(|c| c.parse::<u32>().ok());
            match code {
                Some(c) if (200..400).contains(&c) => (true, line),
                Some(_) => (false, line),
                None => (false, format!("unparseable response: {}", line)),
            }
        }
        Err(e) => (false, format!("read failed: {}", e)),
    }
}

/// Minimal JSON string escaping, local to this app (no serde in workspace).
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
// Seed data: 8 synthetic datasets, stored as phext scrolls
// ---------------------------------------------------------------------------

struct DatasetSpec {
    name: &'static str,
    owner: &'static str,
    cadence_days: i64,
    /// Days before the reference "today" the dataset was last updated.
    /// Chosen against cadence so the demo shows a mix of FRESH and STALE.
    age_days: i64,
    /// true -> source is this app's own /health (alive once running);
    /// false -> source is 127.0.0.1:9 (the discard port; nothing answers).
    live_source: bool,
    fields: &'static [(&'static str, &'static str, &'static str, &'static str)],
}

const DATASETS: &[DatasetSpec] = &[
    DatasetSpec {
        name: "State Contracts Extract",
        owner: "Department of Administrative Services",
        cadence_days: 30,
        age_days: 6,
        live_source: true,
        fields: &[
            ("contract_id", "Statewide unique identifier for the contract", "string", "Agency contract register"),
            ("vendor", "Legal name of the contracting vendor", "string", "Vendor application on file"),
            ("authorized_cents", "Authorized amount in cents, base plus amendments", "integer", "Executed contract and amendments"),
            ("end_date", "Contract end date, ISO 8601", "date", "Executed contract"),
        ],
    },
    DatasetSpec {
        name: "County Bridge Inventory",
        owner: "Department of Transportation",
        cadence_days: 365,
        age_days: 410,
        live_source: false,
        fields: &[
            ("bridge_id", "National Bridge Inventory structure number", "string", "County inspection reports"),
            ("county", "County where the structure sits", "string", "County records"),
            ("condition_rating", "Lowest component condition rating, 0-9", "integer", "Biennial inspection"),
            ("last_inspection", "Date of the most recent inspection", "date", "Inspection crew reports"),
        ],
    },
    DatasetSpec {
        name: "Public Meeting Notices Index",
        owner: "Secretary of State",
        cadence_days: 7,
        age_days: 3,
        live_source: false,
        fields: &[
            ("notice_id", "Unique identifier for the notice", "string", "Publishing agency submission"),
            ("body_name", "Public body holding the meeting", "string", "Publishing agency submission"),
            ("meeting_date", "Date and time of the meeting", "datetime", "Publishing agency submission"),
            ("agenda_url", "Link to the agenda document", "string", "Publishing agency submission"),
        ],
    },
    DatasetSpec {
        name: "State Road Construction Projects",
        owner: "Department of Transportation",
        cadence_days: 14,
        age_days: 21,
        live_source: false,
        fields: &[
            ("project_id", "Internal project control number", "string", "Project management system"),
            ("highway", "Highway designation and milepost range", "string", "Project plans"),
            ("status", "Current phase: design, letting, construction, complete", "string", "Project manager updates"),
            ("estimated_cost_cents", "Current cost estimate in cents", "integer", "Engineer's estimate"),
        ],
    },
    DatasetSpec {
        name: "Water Well Registry Extract",
        owner: "Department of Natural Resources",
        cadence_days: 90,
        age_days: 40,
        live_source: false,
        fields: &[
            ("well_id", "Registered well identifier", "string", "Driller registration filings"),
            ("county", "County of the well location", "string", "Registration filing"),
            ("use_code", "Declared use: irrigation, municipal, domestic, other", "string", "Registration filing"),
            ("completed_date", "Date drilling was completed", "date", "Driller log"),
        ],
    },
    DatasetSpec {
        name: "School District Boundary Files",
        owner: "Department of Education",
        cadence_days: 180,
        age_days: 200,
        live_source: false,
        fields: &[
            ("district_code", "State district code", "string", "Department district register"),
            ("district_name", "Official district name", "string", "Department district register"),
            ("boundary_version", "Version of the boundary geometry in this file", "string", "GIS steward change log"),
        ],
    },
    DatasetSpec {
        name: "State Fleet Vehicle Inventory",
        owner: "Department of Administrative Services",
        cadence_days: 30,
        age_days: 12,
        live_source: false,
        fields: &[
            ("vehicle_id", "Fleet asset tag number", "string", "Fleet management system"),
            ("agency", "Agency the vehicle is assigned to", "string", "Fleet management system"),
            ("odometer", "Last reported odometer reading, miles", "integer", "Fuel card and inspection reports"),
            ("status", "Active, surplus, or retired", "string", "Fleet management system"),
            ("acquired_date", "Date the vehicle entered the fleet", "date", "Purchase record"),
        ],
    },
    DatasetSpec {
        name: "Public Land Survey Corners",
        owner: "Office of the State Surveyor",
        cadence_days: 730,
        age_days: 300,
        live_source: false,
        fields: &[
            ("corner_id", "Survey corner identifier (township/range/section based)", "string", "County surveyor filings"),
            ("monument_type", "Type of monument found or set", "string", "Field survey notes"),
            ("last_visited", "Date the corner was last visited", "date", "Field survey notes"),
        ],
    },
];

/// Build the catalog as a phext `Store`.
///
/// Dataset scrolls live at "1.1.1/1.1.1/6.1.<i>" with lines
/// name=/owner=/cadence_days=/last_updated_day=/source_url=. Field scrolls
/// live nearby at "1.1.1/1.1.1/6.<10+i>.<f>" with lines
/// dataset=/field=/definition=/type=/provenance=.
///
/// `today` and `own_port` are supplied by the caller: last-updated days are
/// relative to `today`, and one dataset's source URL points at this app's
/// own /health endpoint on `own_port` so the link checker shows a live
/// result next to the dead ones (127.0.0.1:9, the discard port).
pub fn seed_store(today: i64, own_port: u16) -> Store {
    let mut store = Store::new();
    for (idx, spec) in DATASETS.iter().enumerate() {
        let i = idx + 1;
        let source_url = if spec.live_source {
            format!("http://127.0.0.1:{}/health", own_port)
        } else {
            format!(
                "http://127.0.0.1:9/{}",
                spec.name.to_lowercase().replace(' ', "-")
            )
        };
        let dcoord = format!("1.1.1/1.1.1/6.1.{}", i);
        let dscroll = format!(
            "name={}\nowner={}\ncadence_days={}\nlast_updated_day={}\nsource_url={}\n",
            spec.name,
            spec.owner,
            spec.cadence_days,
            today - spec.age_days,
            source_url
        );
        store.set(&dcoord, &dscroll);
        for (fidx, (field, definition, type_name, provenance)) in spec.fields.iter().enumerate() {
            let fcoord = format!("1.1.1/1.1.1/6.{}.{}", 10 + i, fidx + 1);
            let fscroll = format!(
                "dataset={}\nfield={}\ndefinition={}\ntype={}\nprovenance={}\n",
                spec.name, field, definition, type_name, provenance
            );
            store.set(&fcoord, &fscroll);
        }
    }
    store
}

// ---------------------------------------------------------------------------
// Scroll parsing
// ---------------------------------------------------------------------------

pub fn parse_kv(text: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            map.insert(k.trim().to_string(), v.trim().to_string());
        }
    }
    map
}

/// Load datasets (with their fields attached) from a catalog store.
/// A scroll is a dataset iff it carries `cadence_days=`; a field iff it
/// carries both `field=` and `dataset=`.
pub fn load_catalog(store: &Store) -> Vec<Dataset> {
    let mut datasets: Vec<Dataset> = Vec::new();
    let mut fields: Vec<FieldDef> = Vec::new();
    for coord in store.coords() {
        let Some(text) = store.get(&coord) else { continue };
        let kv = parse_kv(text);
        if kv.contains_key("cadence_days") {
            datasets.push(Dataset {
                coord: coord.clone(),
                name: kv.get("name").cloned().unwrap_or_default(),
                owner: kv.get("owner").cloned().unwrap_or_default(),
                cadence_days: kv
                    .get("cadence_days")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                last_updated_day: kv
                    .get("last_updated_day")
                    .and_then(|v| v.parse().ok())
                    .unwrap_or(0),
                source_url: kv.get("source_url").cloned().unwrap_or_default(),
                fields: Vec::new(),
            });
        } else if kv.contains_key("field") && kv.contains_key("dataset") {
            fields.push(FieldDef {
                dataset: kv.get("dataset").cloned().unwrap_or_default(),
                field: kv.get("field").cloned().unwrap_or_default(),
                definition: kv.get("definition").cloned().unwrap_or_default(),
                type_name: kv.get("type").cloned().unwrap_or_default(),
                provenance: kv.get("provenance").cloned().unwrap_or_default(),
            });
        }
    }
    datasets.sort_by(|a, b| a.name.cmp(&b.name));
    for d in datasets.iter_mut() {
        d.fields = fields
            .iter()
            .filter(|f| f.dataset == d.name)
            .cloned()
            .collect();
    }
    datasets
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
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>{}</title></head>\n<body>\n<header><h1>Data Catalog</h1><p>{}</p>{}<nav><a href=\"/\">Datasets</a> | <a href=\"/api/staleness\">API: staleness</a> | <a href=\"/api/links\">API: link check</a></nav></header>\n<main>\n{}\n</main>\n<footer><p>Synthetic demo data. Link checks run only when requested, never on page load.</p></footer>\n</body>\n</html>",
        esc(title),
        esc(banner),
        who, body
    )
}

fn index_page(datasets: &[Dataset], today: i64, banner: &str, sess: &Option<&Session>) -> Response {
    let mut cards = String::new();
    for d in datasets {
        let stale = is_stale(d.last_updated_day, d.cadence_days, today);
        let badge = if stale { "STALE" } else { "FRESH" };
        let mut rows = String::new();
        for f in &d.fields {
            rows.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>\n",
                esc(&f.field),
                esc(&f.definition),
                esc(&f.type_name),
                esc(&f.provenance)
            ));
        }
        cards.push_str(&format!(
            "<section>\n<h2>{} <span class=\"badge\">{}</span></h2>\n<p>Owner: {}. Update cadence: every {} days. Last updated: {} ({} days ago). Source: {}.</p>\n<table>\n<thead><tr><th scope=\"col\">Field</th><th scope=\"col\">Definition</th><th scope=\"col\">Type</th><th scope=\"col\">Provenance</th></tr></thead>\n<tbody>\n{}</tbody>\n</table>\n</section>\n",
            esc(&d.name),
            badge,
            esc(&d.owner),
            d.cadence_days,
            exo_core::iso_date(d.last_updated_day),
            today - d.last_updated_day,
            esc(&d.source_url),
            rows
        ));
    }
    let body = format!(
        "<h2>Datasets ({} total, freshness as of {})</h2>\n<p>Staleness rule: a dataset is STALE when today minus its last-updated day exceeds its declared cadence. Use the link-check endpoint to test source URLs on demand.</p>\n{}",
        datasets.len(),
        exo_core::iso_date(today),
        cards
    );
    Response::html(page_shell("Data Catalog", banner, sess, &body))
}

fn staleness_json(datasets: &[Dataset], today: i64) -> Response {
    let items: Vec<String> = datasets
        .iter()
        .map(|d| {
            format!(
                "{{\"name\":\"{}\",\"owner\":\"{}\",\"cadence_days\":{},\"last_updated_day\":{},\"last_updated\":\"{}\",\"age_days\":{},\"stale\":{},\"source_url\":\"{}\",\"fields\":{}}}",
                json_escape(&d.name),
                json_escape(&d.owner),
                d.cadence_days,
                d.last_updated_day,
                exo_core::iso_date(d.last_updated_day),
                today - d.last_updated_day,
                is_stale(d.last_updated_day, d.cadence_days, today),
                json_escape(&d.source_url),
                d.fields.len()
            )
        })
        .collect();
    Response::json(format!("{{\"datasets\":[{}]}}", items.join(",")))
}

/// Run the link checker over every dataset source URL. On demand only.
fn links_json(datasets: &[Dataset]) -> Response {
    let items: Vec<String> = datasets
        .iter()
        .map(|d| {
            let (ok, detail) = check_url(&d.source_url);
            format!(
                "{{\"name\":\"{}\",\"source_url\":\"{}\",\"ok\":{},\"detail\":\"{}\"}}",
                json_escape(&d.name),
                json_escape(&d.source_url),
                ok,
                json_escape(&detail)
            )
        })
        .collect();
    Response::json(format!("{{\"links\":[{}]}}", items.join(",")))
}

// ---------------------------------------------------------------------------
// Routing + main
// ---------------------------------------------------------------------------

fn route(req: &Request, sess: Option<&Session>, datasets: &[Dataset], banner: &str) -> Response {
    let today = exo_core::today_epoch_day();
    let sess_ref = &sess;
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/health") => Response::text("ok".to_string()),
        ("GET", "/") => index_page(datasets, today, banner, sess_ref),
        ("GET", "/api/staleness") => staleness_json(datasets, today),
        ("GET", "/api/links") => links_json(datasets),
        ("POST", "/api/check-links") => {
            // Triggering outbound checks is a POST: refuse cross-site
            // triggers (F-11 floor; the route changes no state).
            if !exo_core::http::origin_matches_host(req) {
                return Response {
                    status: 403,
                    content_type: "text/plain; charset=utf-8".to_string(),
                    body: "cross-origin request refused".to_string(),
                    headers: Vec::new(),
                };
            }
            links_json(datasets)
        }
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
    let store = seed_store(today, port);
    let datasets = Arc::new(load_catalog(&store));

    let sso = SsoConfig::from_env_for("data-catalog").map(Sso::new);
    let banner: String = match &sso {
        Some(s) => s.banner_html().to_string(),
        None => "AUTH DISABLED — local demo mode (SSO not configured)".to_string(),
    };

    eprintln!(
        "data-catalog listening on port {} ({} datasets)",
        port,
        datasets.len()
    );

    let handler_data = datasets.clone();
    exo_core::http::serve(port, move |req| {
        if let Some(s) = &sso {
            match s.gate(req) {
                Gate::Respond(r) => return r,
                Gate::Authed(sess) => {
                    return route(req, Some(&sess), &handler_data, &banner)
                }
                Gate::PassThrough => {}
            }
        }
        route(req, None, &handler_data, &banner)
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staleness_boundaries() {
        // Exactly at cadence is fresh; one day past it is stale.
        assert!(!is_stale(100, 30, 130));
        assert!(is_stale(100, 30, 131));
        assert!(!is_stale(100, 30, 100));
        assert!(!is_stale(100, 0, 100));
        assert!(is_stale(100, 0, 101));
        // A future "last updated" is never stale.
        assert!(!is_stale(200, 30, 150));
    }

    #[test]
    fn https_is_honestly_not_checked() {
        let (ok, detail) = check_url("https://example.com/data");
        assert!(!ok);
        assert_eq!(detail, "https not checked in demo");
    }

    #[test]
    fn non_http_schemes_rejected() {
        let (ok, detail) = check_url("ftp://example.com/data");
        assert!(!ok);
        assert!(detail.contains("unsupported scheme"));
        let (ok2, detail2) = check_url("not-a-url");
        assert!(!ok2);
        assert!(detail2.contains("unsupported scheme"));
    }

    #[test]
    fn dead_loopback_port_fails() {
        // Port 9 is the discard port; on a normal machine nothing answers
        // with HTTP there, so the check must report failure, with detail.
        let (ok, detail) = check_url("http://127.0.0.1:9/state-contracts-extract");
        assert!(!ok);
        assert!(!detail.is_empty());
    }

    #[test]
    fn ssrf_guard_blocks_non_public_addresses() {
        // F-21 regression: the metadata address, RFC-1918 ranges,
        // link-local, and CGNAT space are refused; loopback (the
        // demo's own design) and public addresses are not.
        use std::net::IpAddr;
        for blocked in [
            "169.254.169.254",
            "10.0.0.5",
            "172.16.4.4",
            "192.168.1.20",
            "100.64.0.1",
            "224.0.0.1",
            "0.0.0.0",
        ] {
            assert!(
                ip_blocked(&blocked.parse::<IpAddr>().unwrap()),
                "{} must be blocked",
                blocked
            );
        }
        for allowed in ["127.0.0.1", "8.8.8.8", "1.1.1.1", "203.0.113.9"] {
            assert!(
                !ip_blocked(&allowed.parse::<IpAddr>().unwrap()),
                "{} must be allowed",
                allowed
            );
        }
        // The guard fires before any connection attempt, with a
        // stated reason — 169.254.169.254 would otherwise hang or
        // answer depending on the host network.
        let (ok, detail) = check_url("http://169.254.169.254/latest/meta-data/");
        assert!(!ok);
        assert!(detail.contains("SSRF guard"), "got: {}", detail);
    }

    #[test]
    fn bad_port_reported() {
        let (ok, detail) = check_url("http://127.0.0.1:notaport/x");
        assert!(!ok);
        assert!(detail.contains("bad port"));
    }

    #[test]
    fn seed_has_eight_datasets_with_fields() {
        let store = seed_store(20_000, DEFAULT_PORT);
        let datasets = load_catalog(&store);
        assert_eq!(datasets.len(), 8);
        for d in &datasets {
            assert!(exo_core::valid_coord(&d.coord));
            assert!(
                (3..=5).contains(&d.fields.len()),
                "{} has {} fields",
                d.name,
                d.fields.len()
            );
            for f in &d.fields {
                assert_eq!(f.dataset, d.name);
                assert!(!f.definition.is_empty());
                assert!(!f.provenance.is_empty());
            }
        }
        // Mix of fresh and stale against the fixed reference day.
        assert!(datasets
            .iter()
            .any(|d| is_stale(d.last_updated_day, d.cadence_days, 20_000)));
        assert!(datasets
            .iter()
            .any(|d| !is_stale(d.last_updated_day, d.cadence_days, 20_000)));
        // Exactly one live (/health) source; the rest point at port 9.
        assert_eq!(
            datasets
                .iter()
                .filter(|d| d.source_url.contains("/health"))
                .count(),
            1
        );
        assert_eq!(
            datasets
                .iter()
                .filter(|d| d.source_url.starts_with("http://127.0.0.1:9/"))
                .count(),
            7
        );
    }

    #[test]
    fn json_escape_covers_quotes_and_controls() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        assert_eq!(json_escape("plain"), "plain");
    }
}
