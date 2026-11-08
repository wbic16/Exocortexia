//! minutes-publisher — Public Meetings Minutes Publisher (Exocortexia demo
//! suite, app 2).
//!
//! Publishes synthetic board meetings and checks them against Nebraska
//! Open Meetings Act timing rules (Neb. Rev. Stat. § 84-1413 and related
//! sections): agenda posted at least 24 hours before the meeting, minutes
//! available within 10 working days, and minutes retained for at least
//! 6 months for the bodies the statute names. A hash-chained
//! proof-of-posting audit log records every simulated posting, and
//! `GET /static` emits a self-contained snapshot as the published artifact.
//!
//! All boards, members, and votes are synthetic. Dates are stored as
//! epoch-day integers in scroll text.
//!
//! Port: env `PORT` or 8802.

use std::sync::{Arc, Mutex};

use exo_core::http::{esc, url_encode, Request, Response};
use exo_core::{AuditLog, Rng, Store};
use exo_sso::{Gate, Session, Sso, SsoConfig};

// ---------------------------------------------------------------------------
// Calendar logic (pure — unit-tested below)
// ---------------------------------------------------------------------------

/// Weekday of an epoch day, 0 = Sunday .. 6 = Saturday.
/// Epoch day 0 is 1970-01-01, a Thursday, so weekday = (day + 4) mod 7
/// under the 0 = Sunday convention (Thursday = 4).
pub fn weekday(epoch_day: i64) -> i64 {
    (epoch_day + 4).rem_euclid(7)
}

/// A working day is Monday..Friday.
pub fn is_working_day(epoch_day: i64) -> bool {
    (1..=5).contains(&weekday(epoch_day))
}

/// Number of working days in the half-open interval `(a, b]` — the days
/// after `a`, up to and including `b`. Returns 0 when `b <= a`.
pub fn working_days_between(a: i64, b: i64) -> i64 {
    if b <= a {
        return 0;
    }
    let mut count = 0i64;
    let mut day = a + 1;
    while day <= b {
        if is_working_day(day) {
            count += 1;
        }
        day += 1;
    }
    count
}

// ---------------------------------------------------------------------------
// Meeting model + compliance (pure — unit-tested below)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct Vote {
    pub member: String,
    pub motion: String,
    pub choice: String,
}

#[derive(Clone, Debug)]
pub struct Meeting {
    pub board: String,
    pub coord: String,
    pub meeting_day: i64,
    pub agenda_posted_day: i64,
    /// `None` when minutes have not been posted yet.
    pub minutes_posted_day: Option<i64>,
    pub votes: Vec<Vote>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MinutesStatus {
    /// Posted within 10 working days of the meeting.
    Ok,
    /// Posted after the 10-working-day window.
    Late,
    /// Not posted yet, but the window is still open.
    Pending,
    /// Not posted and the window has closed.
    Missing,
}

#[derive(Clone, Copy, Debug)]
pub struct Compliance {
    pub agenda_ok: bool,
    pub minutes: MinutesStatus,
    pub retention_ok: bool,
}

impl Compliance {
    /// A pending minutes posting is not a violation; everything else
    /// failing is.
    pub fn is_violation(&self) -> bool {
        !self.agenda_ok
            || matches!(self.minutes, MinutesStatus::Late | MinutesStatus::Missing)
            || !self.retention_ok
    }

    /// Single badge label, worst problem first.
    pub fn badge(&self) -> &'static str {
        if !self.agenda_ok {
            "LATE AGENDA"
        } else {
            match self.minutes {
                MinutesStatus::Late | MinutesStatus::Missing => "LATE MINUTES",
                MinutesStatus::Pending => "PENDING",
                MinutesStatus::Ok => {
                    if self.retention_ok {
                        "OK"
                    } else {
                        "LATE MINUTES"
                    }
                }
            }
        }
    }

    /// Human-readable list of what is wrong (empty when compliant or
    /// merely pending).
    pub fn issues(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if !self.agenda_ok {
            out.push("agenda posted less than 24 hours before the meeting");
        }
        match self.minutes {
            MinutesStatus::Late => out.push("minutes posted after the 10-working-day window"),
            MinutesStatus::Missing => out.push("minutes not posted within 10 working days"),
            _ => {}
        }
        if !self.retention_ok {
            out.push("minutes missing beyond the 6-month retention window");
        }
        out
    }
}

/// Compliance of one meeting as of `today` (epoch day).
///
/// - agenda_ok: agenda posted on an earlier day than the meeting
///   (`agenda_posted_day < meeting_day`). Day granularity approximates the
///   statutory 24 hours: a same-day posting is a violation.
/// - minutes: posted within 10 working days after the meeting is Ok;
///   unposted while the window is open is Pending; unposted after the
///   window closes is Missing.
/// - retention_ok: for meetings more than 180 days old, the minutes must
///   still be present (posted and still in the store — this record's
///   presence is what is being checked).
pub fn compliance(meeting: &Meeting, today: i64) -> Compliance {
    let agenda_ok = meeting.agenda_posted_day < meeting.meeting_day;
    let minutes = match meeting.minutes_posted_day {
        Some(posted) => {
            if working_days_between(meeting.meeting_day, posted) <= 10 {
                MinutesStatus::Ok
            } else {
                MinutesStatus::Late
            }
        }
        None => {
            if working_days_between(meeting.meeting_day, today) <= 10 {
                MinutesStatus::Pending
            } else {
                MinutesStatus::Missing
            }
        }
    };
    let retention_ok = if today - meeting.meeting_day > 180 {
        meeting.minutes_posted_day.is_some()
    } else {
        true
    };
    Compliance {
        agenda_ok,
        minutes,
        retention_ok,
    }
}

// ---------------------------------------------------------------------------
// Scroll text <-> Meeting
// ---------------------------------------------------------------------------

/// Parse a meeting scroll. Lines: `board=`, `meeting_day=`,
/// `agenda_posted_day=`, `minutes_posted_day=` (an integer epoch day, or
/// `none`/empty when not yet posted), then `vote=<member>|<motion>|<vote>`
/// lines. Malformed lines are skipped rather than failing the page.
pub fn parse_meeting(coord: &str, text: &str) -> Option<Meeting> {
    let mut board = String::new();
    let mut meeting_day: Option<i64> = None;
    let mut agenda_posted_day: Option<i64> = None;
    let mut minutes_posted_day: Option<i64> = None;
    let mut votes = Vec::new();
    for line in text.lines() {
        let (key, value) = match line.split_once('=') {
            Some(pair) => pair,
            None => continue,
        };
        match key {
            "board" => board = value.trim().to_string(),
            "meeting_day" => meeting_day = value.trim().parse().ok(),
            "agenda_posted_day" => agenda_posted_day = value.trim().parse().ok(),
            "minutes_posted_day" => {
                let v = value.trim();
                minutes_posted_day = if v.is_empty() || v == "none" {
                    None
                } else {
                    v.parse().ok()
                };
            }
            "vote" => {
                let parts: Vec<&str> = value.split('|').collect();
                if parts.len() == 3 {
                    votes.push(Vote {
                        member: parts[0].trim().to_string(),
                        motion: parts[1].trim().to_string(),
                        choice: parts[2].trim().to_string(),
                    });
                }
            }
            _ => {}
        }
    }
    Some(Meeting {
        board,
        coord: coord.to_string(),
        meeting_day: meeting_day?,
        agenda_posted_day: agenda_posted_day?,
        minutes_posted_day,
        votes,
    })
}

pub fn render_meeting_scroll(meeting: &Meeting) -> String {
    let mut out = format!(
        "board={}\nmeeting_day={}\nagenda_posted_day={}\nminutes_posted_day={}\n",
        meeting.board,
        meeting.meeting_day,
        meeting.agenda_posted_day,
        match meeting.minutes_posted_day {
            Some(d) => d.to_string(),
            None => "none".to_string(),
        }
    );
    for v in &meeting.votes {
        out.push_str(&format!("vote={}|{}|{}\n", v.member, v.motion, v.choice));
    }
    out
}

// ---------------------------------------------------------------------------
// Seed corpus (deterministic; clearly synthetic Nebraska-flavoured boards)
// ---------------------------------------------------------------------------

/// Twelve synthetic boards. Every name carries "Example" so no real body
/// is impersonated; the shapes (village board, NRD, school board, county,
/// city council) mirror the bodies Nebraska's Open Meetings Act covers.
pub const BOARD_NAMES: &[&str] = &[
    "Village of Example Board of Trustees",
    "Example County Board of Supervisors",
    "Prairie Example Public School Board",
    "Example Natural Resources District Board",
    "City of Example Falls City Council",
    "Example Rural Fire District Board",
    "North Example Sanitary Improvement District Board",
    "Example Township Board",
    "Sandhill Example Library Board",
    "Example County Weed Control Authority",
    "Village of Exampleton Water Board",
    "Example Community College Board of Governors",
];

const MEMBERS: &[&str] = &[
    "Alex Example",
    "Blair Sample",
    "Casey Placeholder",
    "Devon Fictional",
    "Emerson Demo",
];

const MOTIONS: &[&str] = &[
    "Approve minutes of the prior meeting",
    "Adopt the monthly claims and payroll report",
    "Approve the road gravel bid",
    "Set the public hearing date for the draft budget",
];

const MEETINGS_PER_BOARD: usize = 6;
/// Approximate spacing: oldest first, newest about a week ago. The oldest
/// meeting sits beyond the 180-day retention horizon on purpose.
const MEETING_OFFSETS: &[i64] = &[205, 158, 121, 84, 41, 9];

struct AppState {
    minutes: Store,
    audit_store: Store,
    audit: AuditLog,
}

fn meeting_coord(board_idx: usize, meeting_idx: usize) -> String {
    format!("1.1.1/1.1.1/2.{}.{}", board_idx + 1, meeting_idx + 1)
}

fn build_state() -> AppState {
    let today = exo_core::today_epoch_day();
    let mut store = Store::new();
    let mut audit_store = Store::new();
    let mut audit = AuditLog::new();
    // Optional keyed audit chain (F-04), as in records-desk.
    if let Ok(k) = std::env::var("EXO_AUDIT_KEY") {
        if !k.is_empty() {
            audit.set_key(k.as_bytes());
        }
    }
    let mut rng = Rng(23);
    for (b, board_name) in BOARD_NAMES.iter().enumerate() {
        for m in 0..MEETINGS_PER_BOARD {
            let meeting_day = today - MEETING_OFFSETS[m] + rng.below(3) as i64;
            // Most agendas go up 2-3 days ahead; a deterministic few are
            // posted the same day, which is a violation, so the demo has
            // something to flag.
            let agenda_posted_day = if (b * MEETINGS_PER_BOARD + m) % 11 == 0 {
                meeting_day
            } else {
                meeting_day - (2 + rng.below(2) as i64)
            };
            // Minutes: usually 2-7 days after; a deterministic few are
            // late; the most recent meeting of even boards is still
            // pending; one old meeting (board 3, oldest) never got its
            // minutes, which trips the retention check.
            let minutes_posted_day: Option<i64> = if b == 3 && m == 0 {
                None
            } else if m == MEETINGS_PER_BOARD - 1 && b % 2 == 0 {
                None
            } else if (b + m) % 9 == 0 {
                Some(meeting_day + 18)
            } else {
                Some(meeting_day + 2 + rng.below(6) as i64)
            };
            let mut votes = Vec::new();
            for motion in MOTIONS.iter().take(2) {
                for member in MEMBERS {
                    let choice = match rng.below(10) {
                        0..=7 => "yes",
                        8 => "no",
                        _ => "abstain",
                    };
                    votes.push(Vote {
                        member: member.to_string(),
                        motion: motion.to_string(),
                        choice: choice.to_string(),
                    });
                }
            }
            let meeting = Meeting {
                board: board_name.to_string(),
                coord: meeting_coord(b, m),
                meeting_day,
                agenda_posted_day,
                minutes_posted_day,
                votes,
            };
            let scroll = render_meeting_scroll(&meeting);
            store.set(&meeting.coord, &scroll);
            // Proof-of-posting: the seed-time "publication" is logged.
            audit.append(
                &mut audit_store,
                "minutes-publisher",
                "post_agenda",
                &meeting.coord,
                &format!("agenda for {} meeting_day {}", board_name, meeting_day),
            );
            if let Some(posted) = minutes_posted_day {
                audit.append(
                    &mut audit_store,
                    "minutes-publisher",
                    "post_minutes",
                    &meeting.coord,
                    &format!("minutes for {} posted_day {}", board_name, posted),
                );
            }
        }
    }
    AppState {
        minutes: store,
        audit_store,
        audit,
    }
}

fn all_meetings(state: &AppState) -> Vec<Meeting> {
    let mut meetings: Vec<Meeting> = state
        .minutes
        .coords()
        .iter()
        .filter_map(|coord| {
            state
                .minutes
                .get(coord)
                .and_then(|text| parse_meeting(coord, text))
        })
        .collect();
    meetings.sort_by(|a, b| {
        a.board
            .cmp(&b.board)
            .then(a.meeting_day.cmp(&b.meeting_day))
    });
    meetings
}

fn board_meetings<'a>(meetings: &'a [Meeting], board: &str) -> Vec<&'a Meeting> {
    meetings.iter().filter(|m| m.board == board).collect()
}

fn violation_count(meetings: &[&Meeting], today: i64) -> usize {
    meetings
        .iter()
        .filter(|m| compliance(m, today).is_violation())
        .count()
}

// ---------------------------------------------------------------------------
// Small local helpers
// ---------------------------------------------------------------------------

/// Minimal JSON string escaper (HTML uses `exo_core::http::esc`; JSON
/// needs its own).
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

fn not_found(what: &str) -> Response {
    Response {
        status: 404,
        content_type: "text/plain; charset=utf-8".to_string(),
        body: format!("not found: {}", esc(what)),
        headers: Vec::new(),
    }
}

const STYLE: &str = "body { font-family: sans-serif; color: #111; background: #fff; margin: 2em; max-width: 70em; } \
table { border-collapse: collapse; margin-bottom: 1.5em; } td, th { border: 1px solid #999; padding: 0.3em 0.6em; text-align: left; } \
.banner { border: 1px solid #111; padding: 0.5em 1em; margin-bottom: 1em; } \
.badge { font-weight: bold; }";

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
         <title>{title}</title>\n<style>{STYLE}</style>\n</head>\n<body>\n\
         <header>\n<p class=\"banner\">{banner}</p>{who}\
         <nav><a href=\"/\">Boards</a> | <a href=\"/static\">Static snapshot</a> | <a href=\"/api/compliance\">Compliance JSON</a></nav>\n\
         <h1>{title}</h1>\n</header>\n<main>\n{body}\n</main>\n\
         <footer><p>Minutes Publisher — demonstration software. All boards, members, and votes are synthetic.</p></footer>\n\
         </body>\n</html>",
        title = esc(title),
        banner = esc(banner),
        who = who,
        body = body,
    )
}

fn votes_table(votes: &[Vote]) -> String {
    let mut out = String::from(
        "<table><thead><tr><th>Member</th><th>Motion</th><th>Vote</th></tr></thead><tbody>",
    );
    for v in votes {
        out.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td></tr>",
            esc(&v.member),
            esc(&v.motion),
            esc(&v.choice)
        ));
    }
    out.push_str("</tbody></table>");
    out
}

fn day_label(day: i64) -> String {
    format!("{} (day {})", exo_core::iso_date(day), day)
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

fn handle_index(state: &AppState, banner: &str, session: Option<&Session>) -> Response {
    let today = exo_core::today_epoch_day();
    let meetings = all_meetings(state);
    let mut body = String::new();
    body.push_str(
        "<p>Synthetic Nebraska public bodies and their meeting records, \
         checked against Open Meetings Act timing: agenda at least 24 hours \
         ahead, minutes within 10 working days, minutes retained at least \
         6 months.</p>",
    );
    body.push_str(
        "<table><thead><tr><th>Board</th><th>Meetings</th><th>Violations</th></tr></thead><tbody>",
    );
    for board in BOARD_NAMES {
        let mine = board_meetings(&meetings, board);
        let violations = violation_count(&mine, today);
        body.push_str(&format!(
            "<tr><td><a href=\"/board?name={}\">{}</a></td><td>{}</td><td>{}</td></tr>",
            url_encode(board),
            esc(board),
            mine.len(),
            violations
        ));
    }
    body.push_str("</tbody></table>");
    Response::html(page_shell(
        "Minutes Publisher — Boards",
        banner,
        session,
        &body,
    ))
}

fn handle_board(
    state: &AppState,
    banner: &str,
    session: Option<&Session>,
    req: &Request,
) -> Response {
    let name = match req.query.get("name") {
        Some(n) if !n.is_empty() => n.clone(),
        _ => return not_found("missing board name"),
    };
    if !BOARD_NAMES.contains(&name.as_str()) {
        return not_found(&name);
    }
    let today = exo_core::today_epoch_day();
    let meetings = all_meetings(state);
    let mine = board_meetings(&meetings, &name);
    let mut body = format!(
        "<p><a href=\"/\">Back to boards</a></p><p>{} meeting(s), {} in violation as of {}.</p>",
        mine.len(),
        violation_count(&mine, today),
        esc(&day_label(today))
    );
    body.push_str(
        "<table><thead><tr><th>Meeting date</th><th>Agenda posted</th><th>Minutes posted</th><th>Status</th><th>Votes</th></tr></thead><tbody>",
    );
    for meeting in &mine {
        let c = compliance(meeting, today);
        let minutes_cell = match meeting.minutes_posted_day {
            Some(d) => esc(&day_label(d)),
            None => "not posted".to_string(),
        };
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"badge\">{}</td><td>{}</td></tr>",
            esc(&day_label(meeting.meeting_day)),
            esc(&day_label(meeting.agenda_posted_day)),
            minutes_cell,
            esc(c.badge()),
            votes_table(&meeting.votes)
        ));
    }
    body.push_str("</tbody></table>");
    Response::html(page_shell(&name, banner, session, &body))
}

fn handle_api_compliance(state: &AppState) -> Response {
    let today = exo_core::today_epoch_day();
    let meetings = all_meetings(state);
    let mut boards_json = String::new();
    let mut violations_json = String::new();
    for board in BOARD_NAMES {
        let mine = board_meetings(&meetings, board);
        if !boards_json.is_empty() {
            boards_json.push(',');
        }
        boards_json.push_str(&format!(
            "{{\"board\":\"{}\",\"meetings\":{},\"violations\":{}}}",
            json_escape(board),
            mine.len(),
            violation_count(&mine, today)
        ));
        for meeting in &mine {
            let c = compliance(meeting, today);
            if !c.is_violation() {
                continue;
            }
            if !violations_json.is_empty() {
                violations_json.push(',');
            }
            let issues: Vec<String> = c
                .issues()
                .iter()
                .map(|i| format!("\"{}\"", json_escape(i)))
                .collect();
            violations_json.push_str(&format!(
                "{{\"board\":\"{}\",\"coord\":\"{}\",\"meeting_day\":{},\"date\":\"{}\",\"badge\":\"{}\",\"issues\":[{}]}}",
                json_escape(board),
                json_escape(&meeting.coord),
                meeting.meeting_day,
                json_escape(&exo_core::iso_date(meeting.meeting_day)),
                json_escape(c.badge()),
                issues.join(",")
            ));
        }
    }
    Response::json(format!(
        "{{\"today\":{},\"boards\":[{}],\"violations\":[{}]}}",
        today, boards_json, violations_json
    ))
}

/// The published artifact: one self-contained HTML snapshot (inline style,
/// no external references) of every board and meeting, closing with the
/// proof-of-posting audit entries.
fn handle_static(state: &AppState, banner: &str, session: Option<&Session>) -> Response {
    let today = exo_core::today_epoch_day();
    let meetings = all_meetings(state);
    let mut body = String::new();
    body.push_str(
        "<p>Static publication snapshot. This page is the artifact a body \
         would post: it stands alone, with no scripts and no external \
         references.</p>",
    );
    for board in BOARD_NAMES {
        let mine = board_meetings(&meetings, board);
        body.push_str(&format!("<h2>{}</h2>", esc(board)));
        body.push_str(
            "<table><thead><tr><th>Meeting date</th><th>Agenda posted</th><th>Minutes posted</th><th>Status</th></tr></thead><tbody>",
        );
        for meeting in &mine {
            let c = compliance(meeting, today);
            let minutes_cell = match meeting.minutes_posted_day {
                Some(d) => esc(&day_label(d)),
                None => "not posted".to_string(),
            };
            body.push_str(&format!(
                "<tr><td>{}</td><td>{}</td><td>{}</td><td class=\"badge\">{}</td></tr>",
                esc(&day_label(meeting.meeting_day)),
                esc(&day_label(meeting.agenda_posted_day)),
                minutes_cell,
                esc(c.badge())
            ));
        }
        body.push_str("</tbody></table>");
    }
    // Proof of posting: the hash-chained audit log. The claim made
    // here is exactly what the chain proves (F-04): the entries
    // present are internally consistent. The head commitment below
    // is the anchor a reader can pin and later compare against —
    // if tail entries were dropped, the commitment changes.
    body.push_str("<h2>Proof of posting</h2>");
    match state.audit.verify() {
        Ok(()) => body.push_str(
            "<p>Audit chain verified: every entry present hash-chains to its \
             predecessor, with no gaps. This proves the internal consistency \
             of the entries shown — pin the head commitment below and \
             re-check it later to also detect dropped tail entries.</p>",
        ),
        Err(e) => body.push_str(&format!(
            "<p>Audit chain verification FAILED: {}</p>",
            esc(&e)
        )),
    }
    let (entry_count, head_hash) = state.audit.head();
    body.push_str(&format!(
        "<p>Entries: {}. Head hash: <code>{}</code>. Head commitment: <code>{}</code></p>",
        entry_count,
        esc(&head_hash),
        esc(&state.audit.head_commitment())
    ));
    body.push_str(
        "<table><thead><tr><th>Seq</th><th>Action</th><th>Coordinate</th><th>Hash (prefix)</th></tr></thead><tbody>",
    );
    for entry in &state.audit.entries {
        let prefix: String = entry.hash.chars().take(12).collect();
        body.push_str(&format!(
            "<tr><td>{}</td><td>{}</td><td>{}</td><td>{}</td></tr>",
            entry.seq,
            esc(&entry.action),
            esc(&entry.coord),
            esc(&prefix)
        ));
    }
    body.push_str("</tbody></table>");
    Response::html(page_shell(
        "Minutes Publisher — Static snapshot",
        banner,
        session,
        &body,
    ))
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
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            handle_index(&st, banner, session_ref)
        }
        ("GET", "/board") => {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            handle_board(&st, banner, session_ref, req)
        }
        ("GET", "/api/compliance") => {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            handle_api_compliance(&st)
        }
        ("GET", "/static") => {
            let st = state.lock().unwrap_or_else(|e| e.into_inner());
            handle_static(&st, banner, session_ref)
        }
        _ => not_found(&req.path),
    }
}

fn main() -> std::io::Result<()> {
    let port: u16 = std::env::var("PORT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(8802);
    let state = Arc::new(Mutex::new(build_state()));
    let sso = SsoConfig::from_env_for("minutes-publisher").map(Sso::new);
    let banner = match &sso {
        Some(s) => s.banner_html().to_string(),
        None => "AUTH DISABLED — local demo mode (SSO not configured)".to_string(),
    };
    println!(
        "minutes-publisher listening on port {} ({} boards x {} meetings)",
        port,
        BOARD_NAMES.len(),
        MEETINGS_PER_BOARD
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

    /// Epoch day for 2024-01-01, a Monday: 19,723 days after 1970-01-01.
    const MON_2024_01_01: i64 = 19_723;

    fn meeting(meeting_day: i64, agenda: i64, minutes: Option<i64>) -> Meeting {
        Meeting {
            board: "Test Board".to_string(),
            coord: "test".to_string(),
            meeting_day,
            agenda_posted_day: agenda,
            minutes_posted_day: minutes,
            votes: Vec::new(),
        }
    }

    #[test]
    fn weekday_math_on_known_dates() {
        // Epoch day 0 = Thursday 1970-01-01.
        assert_eq!(weekday(0), 4);
        assert!(is_working_day(0), "Thursday is a working day");
        assert!(!is_working_day(2), "day 2 = Saturday 1970-01-03");
        assert!(!is_working_day(3), "day 3 = Sunday 1970-01-04");
        assert!(is_working_day(4), "day 4 = Monday 1970-01-05");
        // A modern anchor: 2024-01-01 was a Monday; 2024-01-06 a Saturday.
        assert_eq!(weekday(MON_2024_01_01), 1);
        assert!(is_working_day(MON_2024_01_01));
        assert!(!is_working_day(MON_2024_01_01 + 5));
    }

    #[test]
    fn working_days_between_counts_monday_to_friday_only() {
        // Friday (day 1) -> Monday (day 4): Sat, Sun, Mon in (1,4] = 1.
        assert_eq!(working_days_between(1, 4), 1);
        // Monday (day 4) -> next Monday (day 11): Tue..Mon = 5 working.
        assert_eq!(working_days_between(4, 11), 5);
        // Empty and reversed intervals count zero.
        assert_eq!(working_days_between(4, 4), 0);
        assert_eq!(working_days_between(11, 4), 0);
        // Ten working days after Monday day 4 lands on Monday day 18.
        assert_eq!(working_days_between(4, 18), 10);
        assert_eq!(working_days_between(4, 19), 11);
    }

    #[test]
    fn agenda_same_day_is_a_violation() {
        let m = meeting(100, 100, Some(103));
        let c = compliance(&m, 110);
        assert!(!c.agenda_ok, "same-day agenda posting must fail");
        assert!(c.is_violation());
        assert_eq!(c.badge(), "LATE AGENDA");
        // Posted the day before: compliant on the agenda rule.
        let ok = meeting(100, 99, Some(103));
        assert!(compliance(&ok, 110).agenda_ok);
    }

    #[test]
    fn minutes_at_working_day_10_ok_day_11_late() {
        // Meeting on Monday day 4. Working day 10 after it is day 18;
        // working day 11 is day 19 (see working-days test above).
        let on_time = meeting(4, 3, Some(18));
        let c = compliance(&on_time, 30);
        assert_eq!(c.minutes, MinutesStatus::Ok);
        assert!(!c.is_violation());
        assert_eq!(c.badge(), "OK");

        let late = meeting(4, 3, Some(19));
        let c = compliance(&late, 30);
        assert_eq!(c.minutes, MinutesStatus::Late);
        assert!(c.is_violation());
        assert_eq!(c.badge(), "LATE MINUTES");
    }

    #[test]
    fn unposted_minutes_pending_then_missing() {
        let m = meeting(4, 3, None);
        // Within the window (4 working days elapsed): pending, no violation.
        let c = compliance(&m, 10);
        assert_eq!(c.minutes, MinutesStatus::Pending);
        assert!(!c.is_violation());
        assert_eq!(c.badge(), "PENDING");
        // Long after the window: missing, a violation.
        let c = compliance(&m, 60);
        assert_eq!(c.minutes, MinutesStatus::Missing);
        assert!(c.is_violation());
    }

    #[test]
    fn retention_applies_only_after_180_days() {
        // 200 days old with no minutes: retention fails.
        let old_missing = meeting(4, 3, None);
        let c = compliance(&old_missing, 4 + 200);
        assert!(!c.retention_ok);
        assert!(c.is_violation());
        // 200 days old with minutes posted on time: retention holds.
        let old_kept = meeting(4, 3, Some(10));
        let c = compliance(&old_kept, 4 + 200);
        assert!(c.retention_ok);
        assert!(!c.is_violation());
        // Young meeting with no minutes yet: retention not yet applicable.
        let young = meeting(4, 3, None);
        assert!(compliance(&young, 20).retention_ok);
    }

    #[test]
    fn scroll_round_trip_preserves_meeting() {
        let mut m = meeting(19_723, 19_720, Some(19_730));
        m.board = "Village of Example Board of Trustees".to_string();
        m.votes.push(Vote {
            member: "Alex Example".to_string(),
            motion: "Adopt the monthly claims and payroll report".to_string(),
            choice: "yes".to_string(),
        });
        let scroll = render_meeting_scroll(&m);
        let parsed = parse_meeting("coord-x", &scroll).expect("scroll parses");
        assert_eq!(parsed.board, m.board);
        assert_eq!(parsed.meeting_day, m.meeting_day);
        assert_eq!(parsed.agenda_posted_day, m.agenda_posted_day);
        assert_eq!(parsed.minutes_posted_day, m.minutes_posted_day);
        assert_eq!(parsed.votes.len(), 1);
        assert_eq!(parsed.votes[0].choice, "yes");
        // Unposted minutes round-trip through the `none` sentinel.
        let pending = meeting(19_723, 19_720, None);
        let parsed = parse_meeting("c", &render_meeting_scroll(&pending)).unwrap();
        assert_eq!(parsed.minutes_posted_day, None);
    }
}
