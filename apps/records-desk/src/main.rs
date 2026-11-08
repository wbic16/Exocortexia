//! records-desk — Phext Records Desk (Exocortexia demo suite, app 1).
//!
//! A public-records search, redaction, and export-packet demo built on
//! exo-core + exo-sso. The corpus is synthetic and deterministic
//! (`exo_core::Rng(7)`): ~400 public-records documents about public topics.
//! Redactions live in a *separate* store from the documents and are applied
//! at read time, so the original scroll bytes are never modified. Every
//! redaction is also written to a hash-chained audit log.
//!
//! Statute mapping: Neb. Rev. Stat. § 84-712 et seq. (public records) and
//! § 84-712.05 (records that may be withheld — the redaction categories).
//! See `docs/records-desk-statute.md` in the workspace root.
//!
//! Port: env `PORT` or 8801.

use std::sync::{Arc, Mutex};

use exo_core::http::{esc, form, url_encode, Request, Response};
use exo_core::{AuditLog, Redaction, Rng, Store, REDACTION_CATEGORIES};
use exo_sso::{Gate, Session, Sso, SsoConfig};

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// All mutable state for the app, held behind a single `Mutex` (the HTTP
/// layer is thread-per-connection, so handlers share this via `Arc`).
struct AppState {
    /// The public-records documents, one scroll per document.
    docs: Store,
    /// Redaction records (separate store — originals are never touched).
    redactions: Store,
    /// Scrolls backing the audit log.
    audit_store: Store,
    /// Hash-chained audit log over `audit_store`.
    audit: AuditLog,
}

// ---------------------------------------------------------------------------
// Corpus (deterministic, synthetic, no real personal information)
// ---------------------------------------------------------------------------

const DOC_COUNT: usize = 400;

/// Clearly synthetic senders (example.org addresses, invented names).
const SENDERS: &[&str] = &[
    "Jordan Example <jordan.example@example.org>",
    "Casey Placeholder <casey.placeholder@example.org>",
    "Riley Sample <riley.sample@example.org>",
    "Alex Demo <alex.demo@example.org>",
    "Sam Fictional <sam.fictional@example.org>",
    "Jamie Testcase <jamie.testcase@example.org>",
];

struct Topic {
    subject: &'static str,
    opening: &'static str,
    middle: &'static str,
    closing: &'static str,
}

const TOPICS: &[Topic] = &[
    Topic {
        subject: "Bridge maintenance schedule update",
        opening: "The county bridge crew completed its scheduled inspection of rural bridge decks and posted the maintenance rotation for the coming quarter.",
        middle: "Work includes deck patching, guardrail repair, and load-rating verification. Detour signage will be posted at least seven days before any lane closure, and school bus routes have been notified.",
        closing: "Questions about the bridge maintenance programme may be directed to the county roads office during regular business hours.",
    },
    Topic {
        subject: "InterRAI public meeting notes",
        opening: "Notes from the public meeting on the InterRAI assessment instrument, held in the county courthouse annex and open to all residents.",
        middle: "Staff summarised how InterRAI assessment data informs long-term care planning, and members of the public asked about comment periods, data stewardship, and how InterRAI findings are published. The InterRAI discussion record was entered into the public file without amendment.",
        closing: "The next InterRAI public meeting will be announced in the county legal notices and on the public bulletin board.",
    },
    Topic {
        subject: "Park shelter permit approvals",
        opening: "The parks department approved shelter and picnic permit applications for the upcoming season at the municipal parks.",
        middle: "Permit holders are reminded that shelter reservations are first-come after the lottery window, amplified sound ends at dusk, and cleanup deposits are refunded after inspection.",
        closing: "A full list of approved park permits is available at the parks office front counter.",
    },
    Topic {
        subject: "Budget hearing testimony summary",
        opening: "Summary of public testimony received at the annual budget hearing, transcribed from the hearing recording.",
        middle: "Residents spoke about road funding, library hours, and the proposed levy. The board took no action at the hearing; deliberation is scheduled for the next regular session.",
        closing: "Written budget comments remain open for fourteen days after the hearing date and become part of the public record.",
    },
    Topic {
        subject: "Road resurfacing notice",
        opening: "The public works department issued its resurfacing notice for arterial and collector streets scheduled this season.",
        middle: "Milling and overlay work will proceed block by block. Residents will receive door hangers forty-eight hours before work reaches their street, and emergency access will be maintained at all times.",
        closing: "The resurfacing map is posted in the city hall lobby and updated weekly.",
    },
    Topic {
        subject: "Library board hours announcement",
        opening: "The library board announced revised open hours and a public comment period on the draft meeting-room policy.",
        middle: "The draft policy covers room reservations by community groups, exhibit space, and after-hours access for public meetings. Comment forms are available at the circulation desk.",
        closing: "The library board will vote on the policy at its next regular meeting.",
    },
    Topic {
        subject: "Water main flushing schedule",
        opening: "The water utility published its annual water main flushing schedule for all pressure zones.",
        middle: "Flushing may cause temporary discoloration. Residents are advised to run cold taps for two minutes before laundry, and to report any persistent discoloration to the utility office.",
        closing: "Flushing dates by zone are posted at city hall and read aloud at the start of each council meeting.",
    },
    Topic {
        subject: "Zoning variance public notice",
        opening: "Public notice of zoning variance applications set for hearing before the board of adjustment.",
        middle: "Each application lists the parcel, the requested variance, and the hearing time. Affected neighbours within the statutory notice radius were mailed a copy of this notice.",
        closing: "Any resident may testify at the hearing or submit written comment before the hearing date.",
    },
];

fn doc_coord(i: usize) -> String {
    format!("1.1.1/1.1.1/1.1.{}", i + 1)
}

/// Build one synthetic document. `i` is the 0-based document index; the
/// topic cycles deterministically so every topic (including InterRAI) is
/// well represented, while the RNG varies sender, date, and figures.
fn build_doc(rng: &mut Rng, i: usize) -> String {
    let topic = &TOPICS[i % TOPICS.len()];
    let sender = SENDERS[(rng.below(SENDERS.len() as u64)) as usize];
    // Spread documents across roughly two years of public business.
    let day = 18_900 + rng.below(700) as i64;
    let reference = 1000 + rng.below(9000);
    let amount = 500 + rng.below(95_000);
    format!(
        "from={sender}\n\
         to=Public Records Desk <records@example.org>\n\
         date={date}\n\
         subject={subject} (ref {reference})\n\
         \n\
         {opening}\n\
         \n\
         {middle}\n\
         \n\
         Reference number for this item is {reference}. The associated public \
         cost estimate discussed in open session was ${amount}. No action item \
         in this document is confidential; it was prepared for the public file.\n\
         \n\
         {closing}\n",
        sender = sender,
        date = exo_core::iso_date(day),
        subject = topic.subject,
        reference = reference,
        opening = topic.opening,
        middle = topic.middle,
        amount = amount,
        closing = topic.closing,
    )
}

fn build_state() -> AppState {
    let mut docs = Store::new();
    let mut rng = Rng(7);
    for i in 0..DOC_COUNT {
        let text = build_doc(&mut rng, i);
        docs.set(&doc_coord(i), &text);
    }
    let mut audit_store = Store::new();
    let mut audit = AuditLog::new();
    audit.append(
        &mut audit_store,
        "records-desk",
        "seed_corpus",
        "1.1.1/1.1.1/1.1.1",
        &format!("seeded {} synthetic documents", DOC_COUNT),
    );
    AppState {
        docs,
        redactions: Store::new(),
        audit_store,
        audit,
    }
}

// ---------------------------------------------------------------------------
// Small local helpers
// ---------------------------------------------------------------------------

/// Minimal JSON string escaper (the contract's `esc` is for HTML; JSON
/// needs its own escaping).
fn json_escape(s: &str) -> String {
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

fn json_error(status: u16, message: &str) -> Response {
    Response {
        status,
        content_type: "application/json".to_string(),
        body: format!("{{\"ok\":false,\"error\":\"{}\"}}", json_escape(message)),
        headers: Vec::new(),
    }
}

fn not_found(what: &str) -> Response {
    Response {
        status: 404,
        content_type: "text/plain; charset=utf-8".to_string(),
        body: format!("not found: {}", what),
        headers: Vec::new(),
    }
}

/// Numeric suffix of a records-desk coordinate (`.../1.1.<n>`), used to sort
/// coordinates numerically rather than lexicographically.
fn coord_seq(coord: &str) -> usize {
    coord
        .rsplit('.')
        .next()
        .and_then(|n| n.parse::<usize>().ok())
        .unwrap_or(0)
}

/// The `subject=` header of a document, or a placeholder.
fn subject_of(text: &str) -> String {
    for line in text.lines() {
        if let Some(rest) = line.strip_prefix("subject=") {
            return rest.trim().to_string();
        }
    }
    "(no subject)".to_string()
}

fn floor_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx > s.len() {
        idx = s.len();
    }
    while idx > 0 && !s.is_char_boundary(idx) {
        idx -= 1;
    }
    idx
}

fn ceil_char_boundary(s: &str, mut idx: usize) -> usize {
    if idx > s.len() {
        idx = s.len();
    }
    while idx < s.len() && !s.is_char_boundary(idx) {
        idx += 1;
    }
    idx
}

/// Snippet of roughly +/-60 characters around the first case-insensitive
/// match of `q`. For an empty query, the first ~120 characters of the doc.
fn make_snippet(text: &str, q: &str) -> String {
    let flat = text.replace('\n', " ");
    if q.is_empty() {
        let end = floor_char_boundary(&flat, flat.len().min(120));
        return flat[..end].trim().to_string();
    }
    let needle = q.to_lowercase();
    let lower = flat.to_lowercase();
    match lower.find(&needle) {
        Some(idx) => {
            let start = floor_char_boundary(&flat, idx.saturating_sub(60));
            let end = ceil_char_boundary(&flat, (idx + needle.len() + 60).min(flat.len()));
            format!("...{}...", flat[start..end].trim())
        }
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// Search (pure logic over a Store — unit-tested below)
// ---------------------------------------------------------------------------

struct SearchHit {
    coord: String,
    subject: String,
    snippet: String,
}

/// Case-insensitive substring search over the full text of every document.
/// An empty query matches every document. At most `limit` hits, in
/// coordinate order.
fn search_docs(docs: &Store, q: &str, limit: usize) -> Vec<SearchHit> {
    let needle = q.to_lowercase();
    let mut coords = docs.coords();
    coords.sort_by_key(|c| coord_seq(c));
    let mut hits = Vec::new();
    for coord in coords {
        if hits.len() >= limit {
            break;
        }
        let text = match docs.get(&coord) {
            Some(t) => t,
            None => continue,
        };
        if !needle.is_empty() && !text.to_lowercase().contains(&needle) {
            continue;
        }
        hits.push(SearchHit {
            subject: subject_of(text),
            snippet: make_snippet(text, q),
            coord,
        });
    }
    hits
}

// ---------------------------------------------------------------------------
// Time/cost worksheet (pure — unit-tested below)
// ---------------------------------------------------------------------------

/// The anti-2076 worksheet: every number is derived from explicit,
/// stated assumptions so a requester can check the math line by line.
struct Worksheet {
    responsive_docs: usize,
    responsive_bytes: usize,
    pages: usize,
    review_minutes: usize,
    search_minutes: usize,
    total_hours: f64,
    hourly_rate: f64,
    cost: f64,
    delivery_working_days: usize,
}

/// Assumptions (also printed verbatim in the export packet):
/// - 2,000 bytes of plain text per page;
/// - review takes 2 minutes per page;
/// - search/assembly takes 5 minutes per responsive document;
/// - labour is billed at $28.00 per hour (assumption, stated in output);
/// - delivery assumes 6 productive hours per working day.
fn compute_worksheet(responsive_bytes: usize, responsive_docs: usize) -> Worksheet {
    let pages = if responsive_bytes == 0 {
        0
    } else {
        (responsive_bytes + 1999) / 2000
    };
    let review_minutes = pages * 2;
    let search_minutes = responsive_docs * 5;
    let total_hours = (review_minutes + search_minutes) as f64 / 60.0;
    let hourly_rate = 28.00_f64;
    let cost = total_hours * hourly_rate;
    let delivery_working_days = if total_hours <= 0.0 {
        0
    } else {
        (total_hours / 6.0).ceil() as usize
    };
    Worksheet {
        responsive_docs,
        responsive_bytes,
        pages,
        review_minutes,
        search_minutes,
        total_hours,
        hourly_rate,
        cost,
        delivery_working_days,
    }
}

// ---------------------------------------------------------------------------
// HTML shell
// ---------------------------------------------------------------------------

fn page_shell(title: &str, banner: &str, session: Option<&Session>, body: &str) -> String {
    let who = match session {
        Some(s) => format!(
            "<p>Signed in as {} (subject {}).</p>",
            esc(&s.handle),
            esc(&s.sub)
        ),
        None => String::new(),
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>{title}</title>\n<style>\n\
         body {{ font-family: sans-serif; color: #111; background: #fff; margin: 2em; max-width: 60em; }}\n\
         pre {{ white-space: pre-wrap; border: 1px solid #999; padding: 1em; background: #fafafa; }}\n\
         table {{ border-collapse: collapse; }} td, th {{ border: 1px solid #999; padding: 0.3em 0.6em; text-align: left; }}\n\
         .banner {{ border: 1px solid #111; padding: 0.5em 1em; margin-bottom: 1em; }}\n\
         label {{ display: block; margin-top: 0.6em; font-weight: bold; }}\n\
         </style>\n</head>\n<body>\n\
         <header>\n<p class=\"banner\">{banner}</p>{who}\
         <nav><a href=\"/\">Search</a> | <a href=\"/api/export\">Export packet (default query: InterRAI)</a></nav>\n\
         <h1>{title}</h1>\n</header>\n<main>\n{body}\n</main>\n\
         <footer><p>Phext Records Desk — demonstration software. All documents are synthetic; no real personal information is stored, searched, or exported.</p></footer>\n\
         </body>\n</html>",
        title = esc(title),
        banner = esc(banner),
        who = who,
        body = body,
    )
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn handle_index(
    state: &AppState,
    banner: &str,
    session: Option<&Session>,
    q: Option<&str>,
) -> Response {
    let mut body = String::new();
    body.push_str(
        "<p>Search the synthetic public-records corpus. Redactions are kept \
         in a separate layer: original documents are never modified.</p>",
    );
    body.push_str("<form method=\"get\" action=\"/\">");
    body.push_str("<label for=\"q\">Search query</label>");
    body.push_str(&format!(
        "<input id=\"q\" name=\"q\" type=\"text\" value=\"{}\">",
        esc(q.unwrap_or(""))
    ));
    body.push_str("<button type=\"submit\">Search</button>");
    body.push_str("</form>");
    if let Some(query) = q {
        let hits = search_docs(&state.docs, query, 50);
        body.push_str(&format!(
            "<h2>{} result(s) for &ldquo;{}&rdquo;</h2>",
            hits.len(),
            esc(query)
        ));
        if hits.is_empty() {
            body.push_str("<p>No documents matched.</p>");
        } else {
            body.push_str("<ul>");
            for hit in &hits {
                body.push_str(&format!(
                    "<li><a href=\"/doc?coord={}\">{}</a> — {}</li>",
                    url_encode(&hit.coord),
                    esc(&hit.subject),
                    esc(&hit.snippet)
                ));
            }
            body.push_str("</ul>");
        }
    }
    Response::html(page_shell("Phext Records Desk", banner, session, &body))
}

fn handle_api_search(state: &AppState, req: &Request) -> Response {
    let q = req.query.get("q").map(|s| s.as_str()).unwrap_or("");
    // Contract: an empty query returns all documents, limited to 50.
    let limit = if q.is_empty() { 50 } else { usize::MAX };
    let hits = search_docs(&state.docs, q, limit);
    let mut out = String::from("[");
    for (i, hit) in hits.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(&format!(
            "{{\"coord\":\"{}\",\"subject\":\"{}\",\"snippet\":\"{}\"}}",
            json_escape(&hit.coord),
            json_escape(&hit.subject),
            json_escape(&hit.snippet)
        ));
    }
    out.push(']');
    Response::json(out)
}

fn handle_doc(
    state: &AppState,
    banner: &str,
    session: Option<&Session>,
    req: &Request,
) -> Response {
    let coord = match req.query.get("coord") {
        Some(c) if !c.is_empty() => c.clone(),
        _ => return json_error(400, "missing coord query parameter"),
    };
    let original = match state.docs.get(&coord) {
        Some(t) => t.to_string(),
        None => return not_found(&coord),
    };
    let all_redactions = exo_core::load_redactions(&state.redactions);
    let mine: Vec<Redaction> = all_redactions
        .into_iter()
        .filter(|r| r.target == coord)
        .collect();
    // The redacted view is computed at read time; `original` is untouched.
    let redacted_view = exo_core::apply_redactions(&original, &mine);

    let mut options = String::new();
    for cat in REDACTION_CATEGORIES {
        options.push_str(&format!("<option value=\"{0}\">{0}</option>", esc(cat)));
    }
    let mut redaction_list = String::new();
    if mine.is_empty() {
        redaction_list.push_str("<p>No redactions recorded for this document.</p>");
    } else {
        redaction_list.push_str("<ul>");
        for r in &mine {
            redaction_list.push_str(&format!(
                "<li>bytes {}..{} — {} — {}</li>",
                r.start,
                r.end,
                esc(&r.category),
                esc(&r.reason)
            ));
        }
        redaction_list.push_str("</ul>");
    }

    let body = format!(
        "<p><a href=\"/\">Back to search</a> | <a href=\"#custodian-view\">custodian view (original)</a></p>\n\
         <h2>Redacted public view</h2>\n\
         <p>This is the view released to a requester. Redacted ranges are \
         replaced by category markers; the underlying scroll is unchanged.</p>\n\
         <pre>{redacted}</pre>\n\
         <section id=\"custodian-view\">\n<h2>Custodian view (original)</h2>\n\
         <p>For the records custodian only. These are the original bytes, \
         exactly as stored; applying or removing redactions never rewrites them.</p>\n\
         <pre>{original}</pre>\n</section>\n\
         <h2>Recorded redactions</h2>\n{redaction_list}\n\
         <h2>Add a redaction</h2>\n\
         <p>Byte offsets refer to the original document text. The category \
         must be one of the Neb. Rev. Stat. 84-712.05 exceptions.</p>\n\
         <form method=\"post\" action=\"/api/redact\">\n\
         <input type=\"hidden\" name=\"coord\" value=\"{coord}\">\n\
         <label for=\"start\">Start byte</label>\n\
         <input id=\"start\" name=\"start\" type=\"number\" min=\"0\" required>\n\
         <label for=\"end\">End byte</label>\n\
         <input id=\"end\" name=\"end\" type=\"number\" min=\"1\" required>\n\
         <label for=\"category\">Category (84-712.05 exception)</label>\n\
         <select id=\"category\" name=\"category\">{options}</select>\n\
         <label for=\"reason\">Reason</label>\n\
         <input id=\"reason\" name=\"reason\" type=\"text\" required>\n\
         <label for=\"actor\">Actor (your name or role)</label>\n\
         <input id=\"actor\" name=\"actor\" type=\"text\" value=\"custodian\">\n\
         <button type=\"submit\">Record redaction</button>\n\
         </form>",
        redacted = esc(&redacted_view),
        original = esc(&original),
        redaction_list = redaction_list,
        coord = esc(&coord),
        options = options,
    );
    Response::html(page_shell("Record document", banner, session, &body))
}

fn handle_redact(state: &mut AppState, req: &Request) -> Response {
    let fields = form(req);
    let get = |k: &str| fields.get(k).cloned().unwrap_or_default();
    let coord = get("coord");
    let category = get("category");
    let reason = get("reason");
    let mut actor = get("actor");
    if actor.trim().is_empty() {
        actor = "custodian".to_string();
    }
    let start: usize = match get("start").parse() {
        Ok(v) => v,
        Err(_) => return json_error(400, "start must be a non-negative integer"),
    };
    let end: usize = match get("end").parse() {
        Ok(v) => v,
        Err(_) => return json_error(400, "end must be a non-negative integer"),
    };
    let doc_len = match state.docs.get(&coord) {
        Some(t) => t.len(),
        None => return json_error(400, "unknown coord"),
    };
    if !(start < end && end <= doc_len) {
        return json_error(400, "require start < end <= document length in bytes");
    }
    if !REDACTION_CATEGORIES.contains(&category.as_str()) {
        return json_error(
            400,
            "category must be one of the REDACTION_CATEGORIES (Neb. Rev. Stat. 84-712.05)",
        );
    }
    if reason.trim().is_empty() {
        return json_error(400, "reason is required");
    }
    let id = format!("redact-{}", state.redactions.len() + 1);
    let redaction = Redaction {
        target: coord.clone(),
        start,
        end,
        category: category.clone(),
        reason: reason.clone(),
        id: id.clone(),
    };
    exo_core::add_redaction(&mut state.redactions, &redaction);
    state.audit.append(
        &mut state.audit_store,
        &actor,
        "redact",
        &coord,
        &format!(
            "category={} start={} end={} reason={}",
            category, start, end, reason
        ),
    );
    Response::json(format!("{{\"ok\":true,\"id\":\"{}\"}}", json_escape(&id)))
}

fn handle_export(state: &AppState, req: &Request) -> Response {
    let q = req
        .query
        .get("q")
        .cloned()
        .unwrap_or_else(|| "InterRAI".to_string());
    let hits = search_docs(&state.docs, &q, usize::MAX);
    let mut responsive_bytes = 0usize;
    let mut set_list = String::new();
    for hit in &hits {
        if let Some(text) = state.docs.get(&hit.coord) {
            responsive_bytes += text.len();
        }
        set_list.push_str(&format!("{}  {}\n", hit.coord, hit.subject));
    }
    let ws = compute_worksheet(responsive_bytes, hits.len());

    let redactions = exo_core::load_redactions(&state.redactions);
    let mut log = String::new();
    if redactions.is_empty() {
        log.push_str("(no redactions recorded)\n");
    } else {
        for r in &redactions {
            log.push_str(&format!(
                "target={} start={} end={} category={} reason={}\n",
                r.target, r.start, r.end, r.category, r.reason
            ));
        }
    }

    let body = format!(
        "PHEXT RECORDS DESK - PUBLIC RECORDS EXPORT PACKET\n\
         =================================================\n\
         Query: {q}\n\
         Corpus: {total} synthetic documents (demonstration data only)\n\
         \n\
         RESPONSIVE SET ({n} document(s), {bytes} bytes)\n\
         -------------------------------------------------\n\
         {set_list}\
         \n\
         REDACTION LOG ({rcount} entr(ies))\n\
         -------------------------------------------------\n\
         {log}\
         \n\
         TIME/COST WORKSHEET\n\
         -------------------------------------------------\n\
         Assumptions (stated so the math can be checked):\n\
         - 2,000 bytes of plain text per page.\n\
         - Review time: 2 minutes per page.\n\
         - Search and assembly time: 5 minutes per responsive document.\n\
         - Labour rate: ${rate:.2} per hour (assumption for this demo).\n\
         - Delivery: 6 productive hours per working day.\n\
         \n\
         Pages: {bytes} bytes / 2000, rounded up = {pages} page(s)\n\
         Review minutes: {pages} pages x 2 = {review} minute(s)\n\
         Search/assembly minutes: {n} document(s) x 5 = {search} minute(s)\n\
         Total time: {review} + {search} = {total_min} minutes = {hours:.2} hour(s)\n\
         Cost: {hours:.2} hour(s) x ${rate:.2} = ${cost:.2}\n\
         Delivery estimate: {hours:.2} hour(s) / 6 productive hours per day, rounded up = {days} working day(s)\n\
         \n\
         Every figure above is computed from the responsive set at export \
         time. No multi-year delivery timeline appears anywhere in this \
         worksheet: the work is search, review, and arithmetic.\n",
        q = q,
        total = state.docs.len(),
        n = ws.responsive_docs,
        bytes = ws.responsive_bytes,
        set_list = set_list,
        rcount = redactions.len(),
        log = log,
        rate = ws.hourly_rate,
        pages = ws.pages,
        review = ws.review_minutes,
        search = ws.search_minutes,
        total_min = ws.review_minutes + ws.search_minutes,
        hours = ws.total_hours,
        cost = ws.cost,
        days = ws.delivery_working_days,
    );
    Response::text(body)
}

// ---------------------------------------------------------------------------
// Router + main (SSO gate pattern per API-CONTRACT.md)
// ---------------------------------------------------------------------------

fn route(
    req: &Request,
    session: Option<Session>,
    banner: &str,
    state: &Arc<Mutex<AppState>>,
) -> Response {
    let session_ref = session.as_ref();
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => {
            let st = state.lock().unwrap();
            handle_index(
                &st,
                banner,
                session_ref,
                req.query.get("q").map(|s| s.as_str()),
            )
        }
        ("GET", "/api/search") => {
            let st = state.lock().unwrap();
            handle_api_search(&st, req)
        }
        ("GET", "/doc") => {
            let st = state.lock().unwrap();
            handle_doc(&st, banner, session_ref, req)
        }
        ("POST", "/api/redact") => {
            let mut st = state.lock().unwrap();
            handle_redact(&mut st, req)
        }
        ("GET", "/api/export") => {
            let st = state.lock().unwrap();
            handle_export(&st, req)
        }
        _ => not_found(&req.path),
    }
}

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8801);
    let state = Arc::new(Mutex::new(build_state()));
    let sso = SsoConfig::from_env().map(Sso::new);
    let banner = match &sso {
        Some(s) => s.banner_html().to_string(),
        None => "AUTH DISABLED — local demo mode (SSO not configured)".to_string(),
    };
    println!(
        "records-desk listening on port {} ({} synthetic documents)",
        port, DOC_COUNT
    );
    exo_core::http::serve(port, move |req| {
        if let Some(s) = &sso {
            match s.gate(req) {
                Gate::Respond(r) => return r,
                Gate::Authed(sess) => return route(req, Some(sess), &banner, &state),
                Gate::PassThrough => {}
            }
        }
        route(req, None, &banner, &state)
    })
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn test_docs() -> Store {
        let mut docs = Store::new();
        let mut rng = Rng(7);
        for i in 0..DOC_COUNT {
            let text = build_doc(&mut rng, i);
            docs.set(&doc_coord(i), &text);
        }
        docs
    }

    #[test]
    fn search_finds_known_doc() {
        let docs = test_docs();
        // The InterRAI topic is in the deterministic cycle, so a
        // case-insensitive search must find many hits whose subjects
        // name InterRAI.
        let hits = search_docs(&docs, "interrai", usize::MAX);
        assert!(
            hits.len() >= 40,
            "expected many InterRAI hits, got {}",
            hits.len()
        );
        assert!(hits.iter().all(|h| h.subject.contains("InterRAI")));
        assert!(hits.iter().all(|h| !h.snippet.is_empty()));
        // Case-insensitivity: upper-case query finds the same set.
        let upper = search_docs(&docs, "INTERRAI", usize::MAX);
        assert_eq!(hits.len(), upper.len());
        // A topic term unique to one cycle slot also resolves.
        let bridges = search_docs(&docs, "bridge maintenance", usize::MAX);
        assert!(!bridges.is_empty());
        // Empty query returns everything up to the caller's limit.
        let all = search_docs(&docs, "", 50);
        assert_eq!(all.len(), 50);
        // Nonsense query finds nothing.
        assert!(search_docs(&docs, "zzz-no-such-term", usize::MAX).is_empty());
    }

    #[test]
    fn redaction_view_differs_and_original_unchanged() {
        let docs = test_docs();
        // Doc index 1 is an InterRAI document in the cycle.
        let coord = doc_coord(1);
        let original = docs.get(&coord).unwrap().to_string();
        let idx = original.find("InterRAI").expect("doc mentions InterRAI");
        let mut rstore = Store::new();
        let redaction = Redaction {
            target: coord.clone(),
            start: idx,
            end: idx + "InterRAI".len(),
            category: REDACTION_CATEGORIES[0].to_string(),
            reason: "unit test redaction".to_string(),
            id: "test-1".to_string(),
        };
        exo_core::add_redaction(&mut rstore, &redaction);
        let loaded = exo_core::load_redactions(&rstore);
        let mine: Vec<Redaction> = loaded.into_iter().filter(|r| r.target == coord).collect();
        assert_eq!(mine.len(), 1);
        let view = exo_core::apply_redactions(&original, &mine);
        assert_ne!(view, original, "redacted view must differ");
        assert!(view.contains("[REDACTED"));
        assert!(
            !view.contains("InterRAI")
                || view.matches("InterRAI").count() < original.matches("InterRAI").count(),
            "the redacted occurrence must be masked in the view"
        );
        // The layer is separate: the stored original is byte-identical.
        assert_eq!(docs.get(&coord).unwrap(), original);
    }

    #[test]
    fn worksheet_math() {
        // 4,001 bytes over 3 responsive documents.
        let w = compute_worksheet(4001, 3);
        // Expected values derived from the stated assumptions, not magic.
        let expected_pages = (4001 + 1999) / 2000;
        assert_eq!(w.pages, expected_pages);
        assert_eq!(w.pages, 3);
        assert_eq!(w.review_minutes, expected_pages * 2);
        assert_eq!(w.search_minutes, 3 * 5);
        let expected_hours = (w.review_minutes + w.search_minutes) as f64 / 60.0;
        assert!((w.total_hours - expected_hours).abs() < 1e-9);
        assert!((w.cost - expected_hours * 28.00).abs() < 1e-6);
        assert_eq!(
            w.delivery_working_days,
            (expected_hours / 6.0).ceil() as usize
        );
        // Exact page boundary: 4,000 bytes is exactly 2 pages, not 3.
        assert_eq!(compute_worksheet(4000, 0).pages, 2);
        // Empty responsive set costs nothing and ships in zero days.
        let empty = compute_worksheet(0, 0);
        assert_eq!(empty.pages, 0);
        assert_eq!(empty.cost, 0.0);
        assert_eq!(empty.delivery_working_days, 0);
        // A large packet still resolves to days, not years.
        let big = compute_worksheet(2_000_000, 400);
        assert_eq!(big.pages, 1000);
        assert!(big.delivery_working_days < 30);
    }

    #[test]
    fn json_escape_handles_quotes_and_controls() {
        assert_eq!(json_escape("a\"b\\c\nd"), "a\\\"b\\\\c\\nd");
        assert_eq!(json_escape("plain"), "plain");
    }
}
