//! forms-router (port 8805)
//!
//! A forms-intake demo built around an explicit state machine, designed
//! for public-records request intake (Neb. Rev. Stat. 84-712) and small
//! agency applications.
//!
//! Design points:
//!   * The form is **rendered from a phext spec scroll** — the spec is
//!     data, not code. The "public-records-request" form has exactly four
//!     fields (agency, subject, description, date_range) and **no name,
//!     email, or contact fields by design**: intake needs nothing personal
//!     to route a request, so the form never asks. The page says so, in
//!     large text: SYNTHETIC DATA ONLY — DO NOT ENTER PERSONAL INFORMATION.
//!   * Each submission is a phext scroll with a derived tracking code
//!     (first 10 hex chars of sha256(form + seq + values)) that the
//!     requester uses to follow status — no account, no email needed.
//!   * The lifecycle is a strict state machine:
//!     Received -> Routed -> InReview -> Closed, no skips, no way back.
//!     Every accepted transition appends a history line to the submission
//!     scroll and an entry to the hash-chained audit log. Rejected
//!     transitions write nothing and return HTTP 400.
//!
//! Auth: SSO gate per API-CONTRACT.md when configured; open local demo
//! mode with a banner when it is not. State lives behind `Mutex`es because
//! submissions and the audit log mutate per request.

use std::collections::HashMap;
use std::fmt;
use std::sync::{Arc, Mutex};

use exo_core::http::{esc, form, Request, Response};
use exo_core::{AuditLog, Store};
use exo_sso::{Gate, Session, Sso, SsoConfig};

const DEFAULT_PORT: u16 = 8805;
const SPEC_COORD: &str = "1.1.1/1.1.1/7.1.1";
const SUBMISSION_SHELF: &str = "1.1.1/1.1.1/7.2";

// ---------------------------------------------------------------------------
// The state machine
// ---------------------------------------------------------------------------

/// Submission lifecycle. Declaration order is lifecycle order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Received,
    Routed,
    InReview,
    Closed,
}

impl State {
    pub const ALL: [State; 4] = [State::Received, State::Routed, State::InReview, State::Closed];

    pub fn as_str(&self) -> &'static str {
        match self {
            State::Received => "received",
            State::Routed => "routed",
            State::InReview => "in_review",
            State::Closed => "closed",
        }
    }

    /// Parse a state name, tolerant of case and of `-`/space separators
    /// ("InReview", "in-review", "in review" all parse to InReview).
    pub fn parse(s: &str) -> Option<State> {
        let norm = s
            .trim()
            .to_lowercase()
            .replace(['-', ' '], "_")
            .replace("__", "_");
        match norm.as_str() {
            "received" => Some(State::Received),
            "routed" => Some(State::Routed),
            "in_review" | "inreview" => Some(State::InReview),
            "closed" => Some(State::Closed),
            _ => None,
        }
    }
}

impl fmt::Display for State {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The whole machine, in one place: exactly three legal transitions.
/// Everything else — skips, backwards moves, self-loops, anything out of
/// Closed — is illegal.
pub fn can_transition(from: State, to: State) -> bool {
    matches!(
        (from, to),
        (State::Received, State::Routed)
            | (State::Routed, State::InReview)
            | (State::InReview, State::Closed)
    )
}

// ---------------------------------------------------------------------------
// Submissions
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HistoryEntry {
    pub seq: u64,
    pub from: String,
    pub to: String,
    pub ts: i64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Submission {
    pub coord: String,
    pub code: String,
    pub form: String,
    pub state: State,
    /// Field values in spec order, as (field name, value) pairs.
    pub fields: Vec<(String, String)>,
    pub history: Vec<HistoryEntry>,
}

impl Submission {
    pub fn field(&self, name: &str) -> &str {
        self.fields
            .iter()
            .find(|(k, _)| k == name)
            .map(|(_, v)| v.as_str())
            .unwrap_or("")
    }

    /// Attempt a transition. On success the state changes and a history
    /// entry is appended. On failure nothing about the submission changes
    /// and the caller must not persist it (the route layer enforces this
    /// by only writing back on `Ok`).
    pub fn transition(&mut self, to: State, ts: i64) -> Result<(), String> {
        if !can_transition(self.state, to) {
            return Err(format!(
                "illegal transition: {} -> {} is not allowed",
                self.state, to
            ));
        }
        let from = self.state;
        let seq = self.history.len() as u64 + 1;
        self.history.push(HistoryEntry {
            seq,
            from: from.as_str().to_string(),
            to: to.as_str().to_string(),
            ts,
        });
        self.state = to;
        Ok(())
    }

    /// Serialise to the scroll text stored in the submissions Store:
    /// `code=` / `form=` / `state=` lines, one `k=v` line per field, then
    /// one `history=<seq>|<from>-><to>|<ts>` line per history entry.
    pub fn to_scroll(&self) -> String {
        let mut out = format!(
            "code={}\nform={}\nstate={}\n",
            self.code, self.form, self.state
        );
        for (k, v) in &self.fields {
            out.push_str(&format!("{}={}\n", k, v));
        }
        for h in &self.history {
            out.push_str(&format!("history={}|{}->{}|{}\n", h.seq, h.from, h.to, h.ts));
        }
        out
    }

    /// Parse a submission scroll back. Returns None without a code or with
    /// an unparseable state line.
    pub fn from_scroll(coord: &str, text: &str) -> Option<Submission> {
        let mut code = None;
        let mut form_name = String::new();
        let mut state = None;
        let mut fields = Vec::new();
        let mut history = Vec::new();
        for line in text.lines() {
            let Some((k, v)) = line.split_once('=') else {
                continue;
            };
            match k {
                "code" => code = Some(v.to_string()),
                "form" => form_name = v.to_string(),
                "state" => state = State::parse(v),
                "history" => {
                    let parts: Vec<&str> = v.split('|').collect();
                    if parts.len() == 3 {
                        if let (Ok(seq), Ok(ts)) =
                            (parts[0].parse::<u64>(), parts[2].parse::<i64>())
                        {
                            let (from, to) = parts[1]
                                .split_once("->")
                                .map(|(a, b)| (a.to_string(), b.to_string()))
                                .unwrap_or_default();
                            history.push(HistoryEntry { seq, from, to, ts });
                        }
                    }
                }
                _ => fields.push((k.to_string(), v.to_string())),
            }
        }
        Some(Submission {
            coord: coord.to_string(),
            code: code?,
            form: form_name,
            state: state?,
            fields,
            history,
        })
    }
}

/// Derive a tracking code: the first 10 hex characters of
/// sha256(form | seq | k1=v1 | k2=v2 | ...), with values in spec order.
/// Pure: the same form, sequence, and values always yield the same code,
/// so a code can be re-derived and checked without a lookup table.
pub fn tracking_code(form_name: &str, seq: u64, values: &[(String, String)]) -> String {
    let mut material = format!("{}|{}", form_name, seq);
    for (k, v) in values {
        material.push('|');
        material.push_str(k);
        material.push('=');
        material.push_str(v);
    }
    let digest = exo_core::sha256_hex(material.as_bytes());
    digest.chars().take(10).collect()
}

/// Field values must not break the scroll line format: fold CR/LF to a
/// space at intake. (Values are demo data; nothing personal should ever
/// be entered — see the page banner.)
pub fn sanitize_value(v: &str) -> String {
    v.chars()
        .map(|c| if c == '\n' || c == '\r' { ' ' } else { c })
        .collect()
}

// ---------------------------------------------------------------------------
// CSV export
// ---------------------------------------------------------------------------

/// RFC 4180 quoting: quote when the field contains a comma, quote, or
/// newline; double any embedded quotes.
pub fn csv_escape(field: &str) -> String {
    if field.contains(',') || field.contains('"') || field.contains('\n') || field.contains('\r') {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_string()
    }
}

/// The full export: header `code,form,state,agency,subject`, one row per
/// submission, CRLF line endings per RFC 4180.
pub fn submissions_csv(subs: &[Submission]) -> String {
    let mut out = String::from("code,form,state,agency,subject\r\n");
    for s in subs {
        out.push_str(&format!(
            "{},{},{},{},{}\r\n",
            csv_escape(&s.code),
            csv_escape(&s.form),
            csv_escape(s.state.as_str()),
            csv_escape(s.field("agency")),
            csv_escape(s.field("subject"))
        ));
    }
    out
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
// Form spec (a phext scroll; the form is rendered from it)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormSpec {
    pub name: String,
    pub title: String,
    pub fields: Vec<String>,
}

/// The spec scroll. Note what is NOT here: no name, email, phone, or any
/// contact field. Intake routes on agency + subject alone; the requester's
/// tracking code, handed back at submission, is how they follow status.
pub fn spec_store() -> Store {
    let mut store = Store::new();
    store.set(
        SPEC_COORD,
        "form=public-records-request\ntitle=Public Records Request\nfield=agency\nfield=subject\nfield=description\nfield=date_range\n",
    );
    store
}

/// Parse the spec scroll, preserving the declared field order.
pub fn parse_spec(store: &Store) -> Option<FormSpec> {
    let text = store.get(SPEC_COORD)?;
    let mut name = String::new();
    let mut title = String::new();
    let mut fields = Vec::new();
    for line in text.lines() {
        if let Some((k, v)) = line.split_once('=') {
            match k.trim() {
                "form" => name = v.trim().to_string(),
                "title" => title = v.trim().to_string(),
                "field" => fields.push(v.trim().to_string()),
                _ => {}
            }
        }
    }
    if name.is_empty() || fields.is_empty() {
        return None;
    }
    Some(FormSpec { name, title, fields })
}

/// "date_range" -> "Date range" for use as a form label.
pub fn humanize(field: &str) -> String {
    let spaced = field.replace('_', " ");
    let mut chars = spaced.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// App state (mutation behind Mutexes)
// ---------------------------------------------------------------------------

struct AuditState {
    log: AuditLog,
    store: Store,
}

struct App {
    spec: FormSpec,
    submissions: Mutex<Store>,
    audit: Mutex<AuditState>,
    seq: Mutex<u64>,
}

impl App {
    fn audit_append(&self, actor: &str, action: &str, coord: &str, payload: &str) {
        let mut audit = self.audit.lock().unwrap();
        let AuditState { log, store } = &mut *audit;
        log.append(store, actor, action, coord, payload);
    }

    fn all_submissions(&self) -> Vec<Submission> {
        let store = self.submissions.lock().unwrap();
        let mut subs: Vec<Submission> = store
            .coords()
            .iter()
            .filter_map(|c| store.get(c).and_then(|t| Submission::from_scroll(c, t)))
            .collect();
        subs.sort_by(|a, b| a.coord_tail().cmp(&b.coord_tail()));
        subs
    }

    fn find_by_code(&self, code: &str) -> Option<Submission> {
        self.all_submissions().into_iter().find(|s| s.code == code)
    }
}

impl Submission {
    fn coord_tail(&self) -> i64 {
        self.coord
            .rsplit('.')
            .next()
            .and_then(|t| t.parse::<i64>().ok())
            .unwrap_or(0)
    }
}

// ---------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------

fn page_shell(title: &str, banner: &str, sess: &Option<&Session>, body: &str) -> String {
    let who = match sess {
        Some(s) => format!("<p>Signed in as {}</p>", esc(&s.handle)),
        None => String::new(),
    };
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head><meta charset=\"utf-8\"><title>{}</title></head>\n<body>\n<header><h1>Forms Router</h1><p>{}</p>{}<nav><a href=\"/\">Submit a request</a> | <a href=\"/api/export.csv\">Export CSV</a></nav></header>\n<main>\n{}\n</main>\n<footer><p>Synthetic demo. This form collects no personal information by design.</p></footer>\n</body>\n</html>",
        esc(title),
        esc(banner),
        who, body
    )
}

/// Render the submission form from the spec scroll: one labelled input per
/// `field=` line, in spec order. `description` gets a textarea.
fn render_spec_form(spec: &FormSpec) -> String {
    let mut inputs = String::new();
    for field in &spec.fields {
        let id = format!("f-{}", field);
        if field == "description" {
            inputs.push_str(&format!(
                "<p><label for=\"{}\">{}</label><br><textarea id=\"{}\" name=\"{}\" rows=\"4\" cols=\"60\" required></textarea></p>\n",
                esc(&id),
                esc(&humanize(field)),
                esc(&id),
                esc(field)
            ));
        } else {
            inputs.push_str(&format!(
                "<p><label for=\"{}\">{}</label><br><input id=\"{}\" name=\"{}\" type=\"text\" size=\"60\" required></p>\n",
                esc(&id),
                esc(&humanize(field)),
                esc(&id),
                esc(field)
            ));
        }
    }
    format!(
        "<h3>{}</h3>\n<form method=\"post\" action=\"/api/submit\">\n{}<p><button type=\"submit\">Submit request</button></p>\n</form>",
        esc(&spec.title),
        inputs
    )
}

fn index_page(app: &App, banner: &str, sess: &Option<&Session>) -> Response {
    let body = format!(
        "<h2><strong>SYNTHETIC DATA ONLY — DO NOT ENTER PERSONAL INFORMATION</strong></h2>\n<p>This demonstration form asks only for the agency, subject, description, and date range of a records request. It has <strong>no name, email, or contact fields by design</strong>: you will receive a tracking code to follow your request, and no account or personal information is needed.</p>\n{}\n<h3>Track a request</h3>\n<form method=\"get\" action=\"/track\">\n<p><label for=\"track-code\">Tracking code</label><br><input id=\"track-code\" name=\"code\" type=\"text\" size=\"20\" required></p>\n<p><button type=\"submit\">Look up status</button></p>\n</form>",
        render_spec_form(&app.spec)
    );
    Response::html(page_shell("Forms Router", banner, sess, &body))
}

fn submitted_page(sub: &Submission, banner: &str, sess: &Option<&Session>) -> Response {
    let body = format!(
        "<h2>Request received</h2>\n<p>Your tracking code is:</p>\n<p><strong>{}</strong></p>\n<p>Keep this code. Use it on the <a href=\"/track?code={}\">tracking page</a> to follow your request through routed, in review, and closed. Current state: {}.</p>",
        esc(&sub.code),
        esc(&sub.code),
        esc(sub.state.as_str())
    );
    Response::html(page_shell("Request received", banner, sess, &body))
}

fn track_page(app: &App, code: &str, banner: &str, sess: &Option<&Session>) -> Response {
    let Some(sub) = app.find_by_code(code) else {
        return Response {
            status: 404,
            content_type: "text/html; charset=utf-8".to_string(),
            body: page_shell(
                "Not found",
                banner,
                sess,
                "<h2>No submission found</h2><p>No submission carries that tracking code. Check the code and try again.</p>",
            ),
            headers: Vec::new(),
        };
    };
    let mut field_rows = String::new();
    for (k, v) in &sub.fields {
        field_rows.push_str(&format!(
            "<tr><th scope=\"row\">{}</th><td>{}</td></tr>\n",
            esc(&humanize(k)),
            esc(v)
        ));
    }
    let mut timeline = String::new();
    for h in &sub.history {
        timeline.push_str(&format!(
            "<li>{} → {} — {} ({})</li>\n",
            esc(&h.from),
            esc(&h.to),
            exo_core::iso_date(h.ts.div_euclid(86_400)),
            h.ts
        ));
    }
    let body = format!(
        "<h2>Request {}</h2>\n<p>Form: {}. Current state: <strong>{}</strong>.</p>\n<table>\n<tbody>\n{}</tbody>\n</table>\n<h3>History</h3>\n<ol>\n{}</ol>",
        esc(&sub.code),
        esc(&sub.form),
        esc(sub.state.as_str()),
        field_rows,
        timeline
    );
    Response::html(page_shell("Track request", banner, sess, &body))
}

// ---------------------------------------------------------------------------
// API routes
// ---------------------------------------------------------------------------

fn json_response(status: u16, body: String) -> Response {
    Response {
        status,
        content_type: "application/json".to_string(),
        body,
        headers: Vec::new(),
    }
}

fn transition_route(req: &Request, app: &App) -> Response {
    let form_map = form(req);
    let code = form_map.get("code").cloned().unwrap_or_default();
    let to_raw = form_map.get("to").cloned().unwrap_or_default();
    let Some(to) = State::parse(&to_raw) else {
        return json_response(
            400,
            format!(
                "{{\"error\":\"unknown target state: {}\"}}",
                json_escape(&to_raw)
            ),
        );
    };
    let Some(mut sub) = app.find_by_code(&code) else {
        return json_response(
            404,
            format!(
                "{{\"error\":\"no submission with code: {}\"}}",
                json_escape(&code)
            ),
        );
    };
    let ts = exo_core::now_unix();
    match sub.transition(to, ts) {
        Err(e) => {
            // Rejected attempts are NOT written: the store and the audit
            // log keep no trace of them; only the 400 response reports it.
            json_response(400, format!("{{\"error\":\"{}\"}}", json_escape(&e)))
        }
        Ok(()) => {
            let scroll = sub.to_scroll();
            {
                let mut store = app.submissions.lock().unwrap();
                store.set(&sub.coord, &scroll);
            }
            app.audit_append("staff", "transition", &sub.coord, &scroll);
            json_response(
                200,
                format!(
                    "{{\"code\":\"{}\",\"state\":\"{}\"}}",
                    json_escape(&sub.code),
                    sub.state.as_str()
                ),
            )
        }
    }
}

fn export_csv(app: &App) -> Response {
    Response {
        status: 200,
        content_type: "text/csv; charset=utf-8".to_string(),
        body: submissions_csv(&app.all_submissions()),
        headers: vec![(
            "Content-Disposition".to_string(),
            "attachment; filename=\"submissions.csv\"".to_string(),
        )],
    }
}

// ---------------------------------------------------------------------------
// Routing + main
// ---------------------------------------------------------------------------

fn route(req: &Request, sess: Option<&Session>, app: &App, banner: &str) -> Response {
    let sess_ref = &sess;
    match (req.method.as_str(), req.path.as_str()) {
        ("GET", "/") => index_page(app, banner, sess_ref),
        ("GET", "/health") => Response::text("ok".to_string()),
        ("POST", "/api/submit") => submit_route(req, app, banner, sess_ref),
        ("POST", "/api/transition") => transition_route(req, app),
        ("GET", "/track") => {
            let code = req.query.get("code").cloned().unwrap_or_default();
            track_page(app, &code, banner, sess_ref)
        }
        ("GET", "/api/export.csv") => export_csv(app),
        _ => Response {
            status: 404,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: "not found".to_string(),
            headers: Vec::new(),
        },
    }
}

/// Submit handler: creates the submission (state=received), writes its
/// scroll, appends the audit entry, and renders the confirmation page
/// with the tracking code, in the standard page chrome.
fn submit_route(
    req: &Request,
    app: &App,
    banner: &str,
    sess: &Option<&Session>,
) -> Response {
    let form_map: HashMap<String, String> = form(req);
    let values: Vec<(String, String)> = app
        .spec
        .fields
        .iter()
        .map(|f| {
            (
                f.clone(),
                sanitize_value(form_map.get(f).map(|v| v.as_str()).unwrap_or("")),
            )
        })
        .collect();
    let seq = {
        let mut guard = app.seq.lock().unwrap();
        *guard += 1;
        *guard
    };
    let code = tracking_code(&app.spec.name, seq, &values);
    let ts = exo_core::now_unix();
    let sub = Submission {
        coord: format!("{}.{}", SUBMISSION_SHELF, seq),
        code,
        form: app.spec.name.clone(),
        state: State::Received,
        fields: values,
        history: vec![HistoryEntry {
            seq: 1,
            from: "new".to_string(),
            to: "received".to_string(),
            ts,
        }],
    };
    let scroll = sub.to_scroll();
    {
        let mut store = app.submissions.lock().unwrap();
        store.set(&sub.coord, &scroll);
    }
    app.audit_append("public", "submit", &sub.coord, &scroll);
    submitted_page(&sub, banner, sess)
}

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(DEFAULT_PORT);

    let spec = parse_spec(&spec_store()).expect("built-in form spec must parse");
    let app = Arc::new(App {
        spec,
        submissions: Mutex::new(Store::new()),
        audit: Mutex::new(AuditState {
            log: AuditLog::new(),
            store: Store::new(),
        }),
        seq: Mutex::new(0),
    });

    let sso = SsoConfig::from_env().map(Sso::new);
    let banner: String = match &sso {
        Some(s) => s.banner_html().to_string(),
        None => "AUTH DISABLED — local demo mode (SSO not configured)".to_string(),
    };

    eprintln!(
        "forms-router listening on port {} (form: {})",
        port, app.spec.name
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

    fn fresh_submission() -> Submission {
        Submission {
            coord: "1.1.1/1.1.1/7.2.1".to_string(),
            code: "abcdef0123".to_string(),
            form: "public-records-request".to_string(),
            state: State::Received,
            fields: vec![
                ("agency".to_string(), "Department of Transportation".to_string()),
                ("subject".to_string(), "Bridge reports".to_string()),
            ],
            history: vec![HistoryEntry {
                seq: 1,
                from: "new".to_string(),
                to: "received".to_string(),
                ts: 1_700_000_000,
            }],
        }
    }

    #[test]
    fn exactly_three_legal_transitions_of_sixteen() {
        let mut legal = 0;
        for from in State::ALL {
            for to in State::ALL {
                let expected = matches!(
                    (from, to),
                    (State::Received, State::Routed)
                        | (State::Routed, State::InReview)
                        | (State::InReview, State::Closed)
                );
                assert_eq!(
                    can_transition(from, to),
                    expected,
                    "can_transition({}, {})",
                    from,
                    to
                );
                if can_transition(from, to) {
                    legal += 1;
                }
            }
        }
        assert_eq!(legal, 3);
    }

    #[test]
    fn every_illegal_transition_is_rejected_without_mutation() {
        for from in State::ALL {
            for to in State::ALL {
                if can_transition(from, to) {
                    continue;
                }
                let mut sub = fresh_submission();
                sub.state = from;
                let history_len = sub.history.len();
                let result = sub.transition(to, 1_700_000_100);
                assert!(result.is_err(), "{} -> {} must be rejected", from, to);
                assert!(
                    result.unwrap_err().contains("illegal transition"),
                    "error should explain the rejection"
                );
                assert_eq!(sub.state, from, "rejected transition must not move state");
                assert_eq!(
                    sub.history.len(),
                    history_len,
                    "rejected transition must not append history"
                );
            }
        }
    }

    #[test]
    fn full_legal_chain_walks_to_closed() {
        let mut sub = fresh_submission();
        assert!(sub.transition(State::Routed, 10).is_ok());
        assert!(sub.transition(State::InReview, 20).is_ok());
        assert!(sub.transition(State::Closed, 30).is_ok());
        assert_eq!(sub.state, State::Closed);
        assert_eq!(sub.history.len(), 4); // intake + 3 transitions
        assert_eq!(sub.history[1].from, "received");
        assert_eq!(sub.history[1].to, "routed");
        assert_eq!(sub.history[3].to, "closed");
        // Closed is terminal: everything out of it is illegal.
        for to in State::ALL {
            assert!(sub.transition(to, 40).is_err());
        }
    }

    #[test]
    fn tracking_code_is_stable_and_shaped() {
        let values = vec![
            ("agency".to_string(), "State Patrol".to_string()),
            ("subject".to_string(), "Logs".to_string()),
        ];
        let a = tracking_code("public-records-request", 7, &values);
        let b = tracking_code("public-records-request", 7, &values);
        assert_eq!(a, b, "same inputs must give the same code");
        assert_eq!(a.len(), 10);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        // Any change in form, seq, or values changes the code.
        assert_ne!(a, tracking_code("public-records-request", 8, &values));
        assert_ne!(a, tracking_code("other-form", 7, &values));
        let mut changed = values.clone();
        changed[1].1 = "Other".to_string();
        assert_ne!(a, tracking_code("public-records-request", 7, &changed));
    }

    #[test]
    fn csv_quoting_rules() {
        assert_eq!(csv_escape("plain"), "plain");
        assert_eq!(csv_escape("a,b"), "\"a,b\"");
        assert_eq!(csv_escape("say \"hi\""), "\"say \"\"hi\"\"\"");
        assert_eq!(csv_escape("line\nbreak"), "\"line\nbreak\"");
        assert_eq!(csv_escape(""), "");
    }

    #[test]
    fn csv_export_shape_and_quoting() {
        let mut sub = fresh_submission();
        sub.fields[1].1 = "Reports, 2024 \"final\"".to_string();
        let csv = submissions_csv(&[sub]);
        let lines: Vec<&str> = csv.split("\r\n").collect();
        assert_eq!(lines[0], "code,form,state,agency,subject");
        assert_eq!(
            lines[1],
            "abcdef0123,public-records-request,received,Department of Transportation,\"Reports, 2024 \"\"final\"\"\""
        );
    }

    #[test]
    fn spec_parses_and_has_no_contact_fields() {
        let spec = parse_spec(&spec_store()).expect("spec must parse");
        assert_eq!(spec.name, "public-records-request");
        assert_eq!(
            spec.fields,
            vec![
                "agency".to_string(),
                "subject".to_string(),
                "description".to_string(),
                "date_range".to_string()
            ]
        );
        for forbidden in ["name", "email", "phone", "contact", "address"] {
            assert!(
                !spec.fields.iter().any(|f| f.contains(forbidden)),
                "spec must not collect '{}'",
                forbidden
            );
        }
    }

    #[test]
    fn submission_scroll_roundtrips() {
        let mut sub = fresh_submission();
        sub.transition(State::Routed, 1_700_000_200).unwrap();
        let text = sub.to_scroll();
        let back = Submission::from_scroll(&sub.coord, &text).expect("must parse back");
        assert_eq!(sub, back);
        assert!(text.contains("history=2|received->routed|1700000200"));
    }

    #[test]
    fn state_parse_variants() {
        assert_eq!(State::parse("received"), Some(State::Received));
        assert_eq!(State::parse("ROUTED"), Some(State::Routed));
        assert_eq!(State::parse("in_review"), Some(State::InReview));
        assert_eq!(State::parse("InReview"), Some(State::InReview));
        assert_eq!(State::parse("in-review"), Some(State::InReview));
        assert_eq!(State::parse("closed"), Some(State::Closed));
        assert_eq!(State::parse("archived"), None);
        assert_eq!(State::parse(""), None);
    }

    #[test]
    fn humanize_labels() {
        assert_eq!(humanize("agency"), "Agency");
        assert_eq!(humanize("date_range"), "Date range");
        assert_eq!(humanize(""), "");
    }

    #[test]
    fn sanitize_folds_newlines() {
        assert_eq!(sanitize_value("a\nb\rc"), "a b c");
        assert_eq!(sanitize_value("clean"), "clean");
    }
}
