//! exo-sso — a small, shared SSO gate for the Exocortexia demo apps.
//!
//! Model: an app delegates login to edit.phext.io. The identity side
//! redirects the browser back to `/auth/callback` with a short-lived
//! signed *assertion*:
//!
//! ```text
//! token = b64url(json_claims) "." b64url(HMAC-SHA256(sso_secret, b64url(json_claims)))
//! ```
//!
//! The gate verifies the assertion (signature, audience, expiry, a
//! 120-second maximum lifetime, and a `jti` replay cache), then seals a
//! 12-hour local session into the `exo_session` cookie, signed with a
//! separate session secret. Assertions and sessions use the same wire
//! format but different secrets and claim sets, so one can never be
//! substituted for the other.
//!
//! This is demonstration plumbing for local demos — a shared-secret
//! HMAC scheme, not a general OIDC client.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use exo_core::http::{url_encode, Request, Response};
use exo_core::{now_unix, sha256_hex};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

type HmacSha256 = Hmac<Sha256>;

/// Session lifetime: 12 hours.
pub const SESSION_TTL_SECS: i64 = 12 * 3600;

/// Maximum assertion lifetime (`exp - iat`), per contract: 120 seconds.
pub const MAX_ASSERTION_LIFETIME_SECS: i64 = 120;

/// How far into the future an assertion's `iat` may sit (clock skew).
pub const IAT_FUTURE_SKEW_SECS: i64 = 30;

/// Banner apps render on HTML pages when no SSO is configured
/// (`SsoConfig::from_env()` returned `None`).
pub const AUTH_DISABLED_BANNER: &str = "AUTH DISABLED — local demo mode (SSO not configured)";

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// SSO configuration. `Debug` is implemented manually so the two
/// secrets can never be printed by a stray `{:?}` (F-15).
#[derive(Clone)]
pub struct SsoConfig {
    /// Base URL of the edit.phext.io SSO endpoint, e.g.
    /// `https://edit.phext.io/sso` (may itself carry a query string).
    pub edit_sso_url: String,
    /// Shared secret for verifying assertions from the identity side.
    pub sso_secret: String,
    /// Independent secret for sealing local `exo_session` cookies.
    pub session_secret: String,
    /// Audience this app accepts assertions for.
    pub aud: String,
    /// Staff identities (`sub` or `handle` values) allowed to perform
    /// staff actions (records custodian view, forms transitions and
    /// bulk export). From `EXO_STAFF_SUBS` (comma-separated). Empty
    /// means *nobody* is staff — staff actions fail closed.
    pub staff_subs: Vec<String>,
    /// Emit the `Secure` attribute on the session cookie. Default on;
    /// `EXO_COOKIE_SECURE=0` turns it off for plain-HTTP local demos
    /// on non-localhost addresses (browsers accept `Secure` cookies
    /// from `http://127.0.0.1` as a trustworthy origin).
    pub cookie_secure: bool,
}

impl std::fmt::Debug for SsoConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SsoConfig")
            .field("edit_sso_url", &self.edit_sso_url)
            .field("sso_secret", &"<redacted>")
            .field("session_secret", &"<redacted>")
            .field("aud", &self.aud)
            .field("staff_subs", &self.staff_subs)
            .field("cookie_secure", &self.cookie_secure)
            .finish()
    }
}

impl SsoConfig {
    /// Build from the environment, for the app named `app` (its slug,
    /// e.g. `"records-desk"`).
    ///
    /// Fail-closed (F-02): SSO is enabled only when `EDIT_SSO_URL`,
    /// `EXO_SSO_SECRET`, and `EXO_SESSION_SECRET` are all present and
    /// both secrets are at least 32 bytes. Running *without* SSO is
    /// possible only as an explicit opt-in via `EXO_DEMO_MODE=1`, in
    /// which case this returns `None`. A *partial* or invalid SSO
    /// configuration (a typo'd variable name, a too-short secret) is
    /// a startup error — the process panics with the reason rather
    /// than silently serving unauthenticated.
    ///
    /// Audience: `EXO_SSO_AUD`, defaulting to a per-app audience
    /// `exocortexia:<app>` so one assertion cannot be redeemed at
    /// every app in the suite (F-10). Set `EXO_SSO_AUD` explicitly
    /// when the identity side mints a shared audience.
    ///
    /// Staff: `EXO_STAFF_SUBS`, comma-separated `sub`/`handle` values.
    /// Cookie `Secure`: on unless `EXO_COOKIE_SECURE=0`.
    pub fn from_env_for(app: &str) -> Option<SsoConfig> {
        let edit_sso_url = std::env::var("EDIT_SSO_URL").ok();
        let sso_secret = std::env::var("EXO_SSO_SECRET").ok();
        let session_secret = std::env::var("EXO_SESSION_SECRET").ok();
        let any_set = [&edit_sso_url, &sso_secret, &session_secret]
            .iter()
            .any(|v| v.is_some());
        if !any_set {
            if std::env::var("EXO_DEMO_MODE").map(|v| v == "1").unwrap_or(false) {
                return None;
            }
            panic!(
                "SSO is not configured (EDIT_SSO_URL / EXO_SSO_SECRET / EXO_SESSION_SECRET \
                 are unset) and EXO_DEMO_MODE=1 was not given: refusing to start an \
                 unauthenticated server. Set the SSO variables, or set EXO_DEMO_MODE=1 \
                 for an explicitly open local demo."
            );
        }
        let (edit_sso_url, sso_secret, session_secret) = match (edit_sso_url, sso_secret, session_secret) {
            (Some(u), Some(a), Some(b)) => (u, a, b),
            _ => panic!(
                "partial SSO configuration: EDIT_SSO_URL, EXO_SSO_SECRET, and \
                 EXO_SESSION_SECRET must all be set (check for typo'd variable names). \
                 Refusing to start with the gate silently disabled."
            ),
        };
        if edit_sso_url.is_empty() || sso_secret.len() < 32 || session_secret.len() < 32 {
            panic!(
                "invalid SSO configuration: EDIT_SSO_URL must be non-empty and both \
                 secrets at least 32 bytes. Refusing to start with the gate silently disabled."
            );
        }
        let aud = std::env::var("EXO_SSO_AUD")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                if app.is_empty() || app == "exocortexia" {
                    "exocortexia".to_string()
                } else {
                    format!("exocortexia:{}", app)
                }
            });
        let staff_subs = std::env::var("EXO_STAFF_SUBS")
            .ok()
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect()
            })
            .unwrap_or_default();
        let cookie_secure = std::env::var("EXO_COOKIE_SECURE")
            .map(|v| v != "0")
            .unwrap_or(true);
        Some(SsoConfig {
            edit_sso_url,
            sso_secret,
            session_secret,
            aud,
            staff_subs,
            cookie_secure,
        })
    }

    /// [`SsoConfig::from_env_for`] with the shared default audience
    /// stem, for callers that are not one specific app.
    pub fn from_env() -> Option<SsoConfig> {
        Self::from_env_for("exocortexia")
    }
}

// ---------------------------------------------------------------------------
// Session and gate outcome
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct Session {
    pub sub: String,
    pub handle: String,
    pub exp: i64,
}

/// What the gate decided for one request. Apps route like:
///
/// * `Respond(r)` — return `r` immediately (login redirect, callback
///   result, logout).
/// * `Authed(session)` — SSO session present; route with the session.
/// * `PassThrough` — reserved for apps that whitelist public paths
///   before calling the gate; [`Sso::gate`] itself never yields it,
///   because with SSO configured every non-`/auth/*` route requires an
///   authenticated session.
pub enum Gate {
    PassThrough,
    Authed(Session),
    Respond(Response),
}

// ---------------------------------------------------------------------------
// Crypto helpers
// ---------------------------------------------------------------------------

fn hmac_sha256(secret: &[u8], data: &[u8]) -> Vec<u8> {
    let mut mac =
        HmacSha256::new_from_slice(secret).expect("HMAC accepts keys of any length");
    mac.update(data);
    mac.finalize().into_bytes().to_vec()
}

/// Constant-time equality: lengths must match first, then an xor-fold
/// over every byte (no early exit on content).
fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff: u8 = 0;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

fn b64_encode(data: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(data)
}

fn b64_decode(s: &str) -> Result<Vec<u8>, String> {
    URL_SAFE_NO_PAD
        .decode(s)
        .map_err(|e| format!("bad base64url encoding: {}", e))
}

/// Escape a string value for inclusion in hand-built JSON.
fn json_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
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

/// Build a signed token from a JSON claims string.
fn sign_token(secret: &str, json: &str) -> String {
    let payload = b64_encode(json.as_bytes());
    let sig = hmac_sha256(secret.as_bytes(), payload.as_bytes());
    format!("{}.{}", payload, b64_encode(&sig))
}

/// Verify a token's signature and return its claims.
///
/// * `Err` describes the failure (malformed, bad signature encoding,
///   signature mismatch, undecodable or unparseable payload).
pub fn verify_signed_token(secret: &str, token: &str) -> Result<HashMap<String, JsonValue>, String> {
    let (payload_b64, sig_b64) = match token.split_once('.') {
        Some((p, s)) if !p.is_empty() && !s.is_empty() && !s.contains('.') => (p, s),
        _ => return Err("malformed token (expected payload.signature)".to_string()),
    };
    let given_sig = b64_decode(sig_b64).map_err(|_| "bad signature encoding".to_string())?;
    let expected_sig = hmac_sha256(secret.as_bytes(), payload_b64.as_bytes());
    if !ct_eq(&given_sig, &expected_sig) {
        return Err("signature mismatch".to_string());
    }
    let json_bytes = b64_decode(payload_b64)?;
    let json = String::from_utf8(json_bytes).map_err(|_| "payload is not UTF-8".to_string())?;
    parse_flat_json(&json)
}

// ---------------------------------------------------------------------------
// Tiny flat-JSON parser (string / number / bool / null values only)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum JsonValue {
    Str(String),
    Num(i64),
    Bool(bool),
    Null,
}

impl JsonValue {
    pub fn as_str(&self) -> Option<&str> {
        match self {
            JsonValue::Str(s) => Some(s),
            _ => None,
        }
    }
    pub fn as_num(&self) -> Option<i64> {
        match self {
            JsonValue::Num(n) => Some(*n),
            _ => None,
        }
    }
}

/// Parse a flat JSON object: string keys, and string, integer, boolean,
/// or null values. No nesting, no arrays, no floats — by design, since
/// assertion/session claims never need them.
pub fn parse_flat_json(input: &str) -> Result<HashMap<String, JsonValue>, String> {
    let chars: Vec<char> = input.chars().collect();
    let mut pos: usize = 0;
    let mut out: HashMap<String, JsonValue> = HashMap::new();

    fn skip_ws(chars: &[char], pos: &mut usize) {
        while *pos < chars.len() && chars[*pos].is_whitespace() {
            *pos += 1;
        }
    }
    fn parse_string(chars: &[char], pos: &mut usize) -> Result<String, String> {
        if *pos >= chars.len() || chars[*pos] != '"' {
            return Err("expected string".to_string());
        }
        *pos += 1;
        let mut s = String::new();
        while *pos < chars.len() {
            let c = chars[*pos];
            *pos += 1;
            match c {
                '"' => return Ok(s),
                '\\' => {
                    if *pos >= chars.len() {
                        return Err("unterminated escape".to_string());
                    }
                    let e = chars[*pos];
                    *pos += 1;
                    match e {
                        '"' => s.push('"'),
                        '\\' => s.push('\\'),
                        '/' => s.push('/'),
                        'n' => s.push('\n'),
                        'r' => s.push('\r'),
                        't' => s.push('\t'),
                        'b' => s.push('\u{0008}'),
                        'f' => s.push('\u{000C}'),
                        'u' => {
                            if *pos + 4 > chars.len() {
                                return Err("bad \\u escape".to_string());
                            }
                            let hex: String = chars[*pos..*pos + 4].iter().collect();
                            *pos += 4;
                            let code = u32::from_str_radix(&hex, 16)
                                .map_err(|_| "bad \\u escape".to_string())?;
                            s.push(char::from_u32(code).unwrap_or('\u{FFFD}'));
                        }
                        _ => return Err("unknown escape".to_string()),
                    }
                }
                _ => s.push(c),
            }
        }
        Err("unterminated string".to_string())
    }
    fn parse_literal(
        chars: &[char],
        pos: &mut usize,
        lit: &str,
    ) -> Result<(), String> {
        let lit_chars: Vec<char> = lit.chars().collect();
        if *pos + lit_chars.len() <= chars.len()
            && chars[*pos..*pos + lit_chars.len()] == lit_chars[..]
        {
            *pos += lit_chars.len();
            Ok(())
        } else {
            Err(format!("expected {}", lit))
        }
    }

    skip_ws(&chars, &mut pos);
    if pos >= chars.len() || chars[pos] != '{' {
        return Err("expected '{'".to_string());
    }
    pos += 1;
    skip_ws(&chars, &mut pos);
    if pos < chars.len() && chars[pos] == '}' {
        return Ok(out);
    }
    loop {
        skip_ws(&chars, &mut pos);
        let key = parse_string(&chars, &mut pos)?;
        skip_ws(&chars, &mut pos);
        if pos >= chars.len() || chars[pos] != ':' {
            return Err("expected ':'".to_string());
        }
        pos += 1;
        skip_ws(&chars, &mut pos);
        if pos >= chars.len() {
            return Err("unexpected end of input".to_string());
        }
        let value = match chars[pos] {
            '"' => JsonValue::Str(parse_string(&chars, &mut pos)?),
            't' => {
                parse_literal(&chars, &mut pos, "true")?;
                JsonValue::Bool(true)
            }
            'f' => {
                parse_literal(&chars, &mut pos, "false")?;
                JsonValue::Bool(false)
            }
            'n' => {
                parse_literal(&chars, &mut pos, "null")?;
                JsonValue::Null
            }
            '-' | '0'..='9' => {
                let start = pos;
                if chars[pos] == '-' {
                    pos += 1;
                }
                while pos < chars.len() && chars[pos].is_ascii_digit() {
                    pos += 1;
                }
                // Floats are out of scope for flat claim sets.
                if pos < chars.len() && (chars[pos] == '.' || chars[pos] == 'e' || chars[pos] == 'E') {
                    return Err("non-integer numbers are not supported".to_string());
                }
                let text: String = chars[start..pos].iter().collect();
                let n: i64 = text
                    .parse()
                    .map_err(|_| "bad number".to_string())?;
                JsonValue::Num(n)
            }
            _ => return Err("unsupported value".to_string()),
        };
        out.insert(key, value);
        skip_ws(&chars, &mut pos);
        if pos >= chars.len() {
            return Err("unterminated object".to_string());
        }
        match chars[pos] {
            ',' => {
                pos += 1;
            }
            '}' => {
                pos += 1;
                break;
            }
            _ => return Err("expected ',' or '}'".to_string()),
        }
    }
    Ok(out)
}

// ---------------------------------------------------------------------------
// Safe return paths
// ---------------------------------------------------------------------------

/// Validate a post-login return path. Returns the path unchanged when it
/// is a same-origin absolute path, and `"/"` otherwise.
///
/// A safe path must start with `/`, must not start with `//`
/// (protocol-relative URL), and must contain no `:` (scheme smuggling)
/// and no backslash (Windows/URL-parser confusion), and no control or
/// whitespace characters.
pub fn safe_ret_path(ret: &str) -> String {
    let ok = ret.starts_with('/')
        && !ret.starts_with("//")
        && !ret.contains(':')
        && !ret.contains('\\')
        && !ret.chars().any(|c| c.is_whitespace() || c.is_control());
    if ok {
        ret.to_string()
    } else {
        "/".to_string()
    }
}

// ---------------------------------------------------------------------------
// Assertion minting (identity side; used by the mock minter and tests)
// ---------------------------------------------------------------------------

static MINT_COUNTER: AtomicU64 = AtomicU64::new(0);

/// Mint a signed assertion. This is the identity side of the protocol —
/// production assertions are minted by edit.phext.io; here it exists
/// for the `edit-minter` mock binary and for tests.
///
/// Claims: `sub`, `handle`, `aud`, `ret`, `iat` (now), `exp`
/// (now + `ttl_secs`), and a unique `jti` (timestamp + counter +
/// content hash, so two mints in the same second never collide).
pub fn mint_assertion(
    secret: &str,
    sub: &str,
    handle: &str,
    aud: &str,
    ret: &str,
    ttl_secs: i64,
) -> String {
    let now = now_unix();
    let n = MINT_COUNTER.fetch_add(1, Ordering::SeqCst);
    let jti = format!(
        "{}-{}-{}",
        now,
        n,
        &sha256_hex(format!("{}|{}|{}|{}", sub, aud, now, n).as_bytes())[..16]
    );
    let json = format!(
        "{{\"sub\":\"{}\",\"handle\":\"{}\",\"aud\":\"{}\",\"ret\":\"{}\",\"iat\":{},\"exp\":{},\"jti\":\"{}\"}}",
        json_escape(sub),
        json_escape(handle),
        json_escape(aud),
        json_escape(ret),
        now,
        now + ttl_secs,
        json_escape(&jti)
    );
    sign_token(secret, &json)
}

// ---------------------------------------------------------------------------
// The gate
// ---------------------------------------------------------------------------

pub struct Sso {
    pub cfg: SsoConfig,
    /// Replay cache: `jti` → assertion `exp`. Entries are swept lazily
    /// on each verification once their assertion has expired.
    replay: Mutex<HashMap<String, i64>>,
}

struct Claims {
    session: Session,
    ret: String,
}

impl Sso {
    pub fn new(cfg: SsoConfig) -> Sso {
        Sso {
            cfg,
            replay: Mutex::new(HashMap::new()),
        }
    }

    /// Associated-function form of [`mint_assertion`], for callers that
    /// prefer `Sso::mint_assertion(...)`.
    pub fn mint_assertion(
        secret: &str,
        sub: &str,
        handle: &str,
        aud: &str,
        ret: &str,
        ttl_secs: i64,
    ) -> String {
        mint_assertion(secret, sub, handle, aud, ret, ttl_secs)
    }

    fn verify_claims(&self, token: &str) -> Result<Claims, String> {
        let claims = verify_signed_token(&self.cfg.sso_secret, token)?;
        let now = now_unix();

        let sub = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing sub claim".to_string())?
            .to_string();
        let handle = claims
            .get("handle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();

        let aud = claims.get("aud").and_then(|v| v.as_str()).unwrap_or("");
        if aud != self.cfg.aud {
            return Err(format!(
                "wrong audience (expected {}, got {})",
                self.cfg.aud,
                if aud.is_empty() { "<missing>" } else { aud }
            ));
        }

        let exp = claims
            .get("exp")
            .and_then(|v| v.as_num())
            .ok_or_else(|| "missing exp claim".to_string())?;
        if now >= exp {
            return Err("expired assertion".to_string());
        }

        // `iat` is required (F-09): without it the issuer could opt
        // out of the lifetime cap entirely by omitting the claim.
        let iat = claims
            .get("iat")
            .and_then(|v| v.as_num())
            .ok_or_else(|| "missing iat claim".to_string())?;
        if exp - iat > MAX_ASSERTION_LIFETIME_SECS {
            return Err(format!(
                "assertion lifetime too long (exp-iat = {}s, max {}s)",
                exp - iat,
                MAX_ASSERTION_LIFETIME_SECS
            ));
        }
        if iat > now + IAT_FUTURE_SKEW_SECS {
            return Err("assertion issued in the future".to_string());
        }
        // Independently of `iat`: an assertion must not remain valid
        // far into the future from *now*.
        if exp - now > MAX_ASSERTION_LIFETIME_SECS + IAT_FUTURE_SKEW_SECS {
            return Err("assertion expiry too far in the future".to_string());
        }

        let jti = claims
            .get("jti")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| "missing jti claim".to_string())?;
        {
            let mut cache = self
                .replay
                .lock()
                .map_err(|_| "replay cache lock poisoned".to_string())?;
            cache.retain(|_, cached_exp| *cached_exp > now);
            if cache.contains_key(jti) {
                return Err("replay: jti already used".to_string());
            }
            cache.insert(jti.to_string(), exp);
        }

        let ret = claims
            .get("ret")
            .and_then(|v| v.as_str())
            .unwrap_or("/")
            .to_string();

        Ok(Claims {
            session: Session { sub, handle, exp },
            ret,
        })
    }

    /// Verify an assertion token, returning the session it establishes.
    /// See the crate docs for the full check list; a `jti` is consumed
    /// (recorded in the replay cache) only by a successful verification.
    pub fn verify_assertion(&self, token: &str) -> Result<Session, String> {
        Ok(self.verify_claims(token)?.session)
    }

    // -- sessions -----------------------------------------------------------

    /// Seal a session into an `exo_session` cookie value. The sealed
    /// claims are `sub`, `handle`, `aud` (this app's audience — a
    /// session sealed by one app does not open in another, even when
    /// an operator reuses one session secret across the suite, F-10),
    /// and `exp = now + 12h` — the session lifetime is fixed at seal
    /// time, regardless of how much life the originating assertion
    /// had left.
    pub fn seal_session(&self, session: &Session) -> String {
        let exp = now_unix() + SESSION_TTL_SECS;
        let json = format!(
            "{{\"sub\":\"{}\",\"handle\":\"{}\",\"aud\":\"{}\",\"exp\":{}}}",
            json_escape(&session.sub),
            json_escape(&session.handle),
            json_escape(&self.cfg.aud),
            exp
        );
        sign_token(&self.cfg.session_secret, &json)
    }

    /// Open an `exo_session` cookie value. `None` on any failure:
    /// bad signature, malformed claims, missing `sub`, wrong
    /// audience, or expiry.
    pub fn open_session(&self, value: &str) -> Option<Session> {
        let claims = verify_signed_token(&self.cfg.session_secret, value).ok()?;
        let exp = claims.get("exp").and_then(|v| v.as_num())?;
        if now_unix() >= exp {
            return None;
        }
        if claims.get("aud").and_then(|v| v.as_str()) != Some(self.cfg.aud.as_str()) {
            return None;
        }
        let sub = claims
            .get("sub")
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())?
            .to_string();
        let handle = claims
            .get("handle")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        Some(Session { sub, handle, exp })
    }

    /// True when `session` belongs to a staff identity
    /// (`EXO_STAFF_SUBS`). Staff actions fail closed: with SSO
    /// configured and no staff list, nobody qualifies.
    pub fn is_staff(&self, session: &Session) -> bool {
        self.cfg
            .staff_subs
            .iter()
            .any(|s| s == &session.sub || s == &session.handle)
    }

    /// A CSRF token bound to this session (F-11): HMAC of the
    /// session's identity and expiry under the session secret. A
    /// cross-site form cannot know it, and it dies with the session.
    pub fn csrf_token(&self, session: &Session) -> String {
        let mac = hmac_sha256(
            self.cfg.session_secret.as_bytes(),
            format!("csrf|{}|{}", session.sub, session.exp).as_bytes(),
        );
        b64_encode(&mac)
    }

    /// Constant-time check of a submitted CSRF token.
    pub fn csrf_ok(&self, session: &Session, token: &str) -> bool {
        ct_eq(token.as_bytes(), self.csrf_token(session).as_bytes())
    }

    /// The full `Set-Cookie` value string for a sealed session.
    fn session_cookie(&self, session: &Session) -> String {
        format!(
            "exo_session={}; HttpOnly; SameSite=Lax; Path=/; Max-Age={}{}",
            self.seal_session(session),
            SESSION_TTL_SECS,
            if self.cfg.cookie_secure { "; Secure" } else { "" }
        )
    }

    // -- the gate -----------------------------------------------------------

    /// Route one request through the SSO gate. See [`Gate`].
    ///
    /// Note on unauthenticated requests: every non-`/auth/*` path
    /// without a valid session gets a 302 to
    /// `/auth/login?return=<current path>` — including non-GET requests.
    /// That is a deliberate demo simplification: a production gate
    /// would 401 API-style requests instead of redirecting them.
    pub fn gate(&self, req: &Request) -> Gate {
        match req.path.as_str() {
            "/auth/callback" => Gate::Respond(self.handle_callback(req)),
            "/auth/login" => Gate::Respond(self.handle_login(req)),
            // Logout changes state, so it requires POST (F-19): a
            // cross-site <img> GET must not be able to log a user out.
            "/auth/logout" if req.method == "POST" => Gate::Respond(
                Response::redirect("/").set_cookie(format!(
                    "exo_session=; HttpOnly; SameSite=Lax; Path=/; Max-Age=0{}",
                    if self.cfg.cookie_secure { "; Secure" } else { "" }
                )),
            ),
            "/auth/logout" => Gate::Respond(Response {
                status: 405,
                content_type: "text/plain; charset=utf-8".to_string(),
                body: "logout requires POST".to_string(),
                headers: Vec::new(),
            }),
            // Liveness probes are not behind the gate: /health reveals
            // nothing and load balancers need it unauthenticated.
            "/health" => Gate::PassThrough,
            _ => match req
                .cookies
                .get("exo_session")
                .and_then(|v| self.open_session(v))
            {
                Some(session) => Gate::Authed(session),
                None => {
                    let mut return_to = req.path.clone();
                    if !req.query.is_empty() {
                        let mut keys: Vec<&String> = req.query.keys().collect();
                        keys.sort();
                        let pairs: Vec<String> = keys
                            .iter()
                            .map(|k| {
                                format!(
                                    "{}={}",
                                    url_encode(k),
                                    url_encode(req.query.get(*k).map(|v| v.as_str()).unwrap_or(""))
                                )
                            })
                            .collect();
                        return_to = format!("{}?{}", return_to, pairs.join("&"));
                    }
                    Gate::Respond(Response::redirect(format!(
                        "/auth/login?return={}",
                        url_encode(&return_to)
                    )))
                }
            },
        }
    }

    fn handle_callback(&self, req: &Request) -> Response {
        let token = match req.query.get("assertion") {
            Some(t) if !t.is_empty() => t.clone(),
            _ => {
                let mut r = Response::text("SSO assertion rejected: missing assertion");
                r.status = 401;
                return r;
            }
        };
        match self.verify_claims(&token) {
            Ok(claims) => Response::redirect(safe_ret_path(&claims.ret))
                .set_cookie(self.session_cookie(&claims.session)),
            Err(why) => {
                // The detailed reason (including the expected
                // audience, F-14) goes to the server log only; the
                // unauthenticated caller gets a generic rejection.
                eprintln!("sso: assertion rejected: {}", why);
                let mut r = Response::text(
                    "SSO assertion rejected: invalid or expired assertion".to_string(),
                );
                r.status = 401;
                r
            }
        }
    }

    fn handle_login(&self, req: &Request) -> Response {
        let return_to = req
            .query
            .get("return")
            .cloned()
            .unwrap_or_else(|| "/".to_string());
        let sep = if self.cfg.edit_sso_url.contains('?') {
            '&'
        } else {
            '?'
        };
        Response::redirect(format!(
            "{}{}return={}",
            self.cfg.edit_sso_url,
            sep,
            url_encode(&return_to)
        ))
    }

    /// Small banner noting that SSO is on. (When SSO is *not*
    /// configured there is no `Sso` to ask — apps render
    /// [`AUTH_DISABLED_BANNER`] themselves in that case.)
    pub fn banner_html(&self) -> &str {
        "<aside class=\"sso-note\">SSO: edit.phext.io</aside>"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const SSO_SECRET: &str = "test-sso-secret-0123456789abcdef0123456789";
    const SESSION_SECRET: &str = "test-session-secret-0123456789abcdef0123";
    const OTHER_SECRET: &str = "other-secret-0123456789abcdef0123456789ab";

    fn test_sso() -> Sso {
        Sso::new(SsoConfig {
            edit_sso_url: "http://127.0.0.1:8799/sso".to_string(),
            sso_secret: SSO_SECRET.to_string(),
            session_secret: SESSION_SECRET.to_string(),
            aud: "exocortexia".to_string(),
            staff_subs: vec!["mock:citizen-1".to_string()],
            cookie_secure: true,
        })
    }

    fn bare_request(path: &str) -> Request {
        Request {
            method: "GET".to_string(),
            path: path.to_string(),
            query: HashMap::new(),
            headers: HashMap::new(),
            body: String::new(),
            cookies: HashMap::new(),
        }
    }

    #[test]
    fn mint_then_verify_happy_path() {
        let sso = test_sso();
        let token = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/dashboard",
            120,
        );
        let session = sso
            .verify_assertion(&token)
            .expect("freshly minted assertion must verify");
        assert_eq!(session.sub, "mock:citizen-1");
        assert_eq!(session.handle, "mock-citizen");
        assert!(session.exp > now_unix());
    }

    #[test]
    fn wrong_secret_rejected() {
        let sso = test_sso();
        let token = mint_assertion(
            OTHER_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/",
            120,
        );
        let err = sso
            .verify_assertion(&token)
            .expect_err("assertion signed with the wrong secret must be rejected");
        assert!(err.contains("signature"), "got: {}", err);
    }

    #[test]
    fn expired_assertion_rejected() {
        let sso = test_sso();
        let token = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/",
            -10,
        );
        let err = sso
            .verify_assertion(&token)
            .expect_err("expired assertion must be rejected");
        assert!(err.contains("expired"), "got: {}", err);
    }

    #[test]
    fn replayed_jti_rejected() {
        let sso = test_sso();
        let token = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/",
            120,
        );
        sso.verify_assertion(&token)
            .expect("first use must succeed");
        let err = sso
            .verify_assertion(&token)
            .expect_err("second use of the same jti must be rejected as a replay");
        assert!(err.contains("replay"), "got: {}", err);
    }

    #[test]
    fn wrong_audience_rejected() {
        let sso = test_sso();
        let token = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "some-other-app",
            "/",
            120,
        );
        let err = sso
            .verify_assertion(&token)
            .expect_err("assertion for another audience must be rejected");
        assert!(err.contains("audience"), "got: {}", err);
    }

    #[test]
    fn overlong_lifetime_rejected() {
        let sso = test_sso();
        let token = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/",
            3600,
        );
        let err = sso
            .verify_assertion(&token)
            .expect_err("exp-iat > 120 must be rejected");
        assert!(err.contains("lifetime"), "got: {}", err);
    }

    #[test]
    fn session_seal_open_round_trip_and_tamper() {
        let sso = test_sso();
        let session = Session {
            sub: "mock:citizen-1".to_string(),
            handle: "mock-citizen".to_string(),
            exp: 0, // seal_session assigns the real 12h expiry
        };
        let sealed = sso.seal_session(&session);
        let opened = sso.open_session(&sealed).expect("sealed session must open");
        assert_eq!(opened.sub, "mock:citizen-1");
        assert_eq!(opened.handle, "mock-citizen");
        assert!(opened.exp > now_unix() + 11 * 3600);

        // Flip the last character of the signature: must not open.
        let mut tampered = sealed.clone();
        let last = tampered.pop().unwrap();
        tampered.push(if last == 'A' { 'B' } else { 'A' });
        assert!(sso.open_session(&tampered).is_none());
        // An assertion is not a session (different secret): must not open.
        let assertion = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/",
            120,
        );
        assert!(sso.open_session(&assertion).is_none());
    }

    #[test]
    fn safe_ret_path_rules() {
        assert_eq!(safe_ret_path("/dashboard"), "/dashboard");
        assert_eq!(safe_ret_path("/a/b?x=1"), "/a/b?x=1");
        assert_eq!(safe_ret_path("//evil.example"), "/");
        assert_eq!(safe_ret_path("https://evil.example"), "/");
        assert_eq!(safe_ret_path("/a:b"), "/");
        assert_eq!(safe_ret_path("/a\\b"), "/");
        assert_eq!(safe_ret_path("relative"), "/");
        assert_eq!(safe_ret_path(""), "/");
    }

    #[test]
    fn gate_flows() {
        let sso = test_sso();

        // Unauthenticated page request -> 302 to login with return path.
        match sso.gate(&bare_request("/records")) {
            Gate::Respond(r) => {
                assert_eq!(r.status, 302);
                assert_eq!(r.headers[0].0, "Location");
                assert_eq!(r.headers[0].1, "/auth/login?return=%2Frecords");
            }
            _ => panic!("expected a redirect response"),
        }

        // Login -> 302 to the identity side.
        let mut login_req = bare_request("/auth/login");
        login_req
            .query
            .insert("return".to_string(), "/records".to_string());
        match sso.gate(&login_req) {
            Gate::Respond(r) => {
                assert_eq!(r.status, 302);
                assert_eq!(
                    r.headers[0].1,
                    "http://127.0.0.1:8799/sso?return=%2Frecords"
                );
            }
            _ => panic!("expected a redirect response"),
        }

        // Callback with a good assertion -> 302 to the ret path + cookie.
        let token = mint_assertion(
            SSO_SECRET,
            "mock:citizen-1",
            "mock-citizen",
            "exocortexia",
            "/records",
            120,
        );
        let mut cb = bare_request("/auth/callback");
        cb.query.insert("assertion".to_string(), token);
        let cookie = match sso.gate(&cb) {
            Gate::Respond(r) => {
                assert_eq!(r.status, 302);
                assert_eq!(r.headers[0].1, "/records");
                let set_cookie = r
                    .headers
                    .iter()
                    .find(|(k, _)| k == "Set-Cookie")
                    .expect("callback must set the session cookie")
                    .1
                    .clone();
                assert!(set_cookie.starts_with("exo_session="));
                assert!(set_cookie.contains("HttpOnly"));
                assert!(set_cookie.contains("SameSite=Lax"));
                assert!(set_cookie.contains("Path=/"));
                set_cookie
            }
            _ => panic!("expected a redirect response"),
        };

        // The sealed cookie authenticates the next request.
        let sealed = cookie
            .split(';')
            .next()
            .unwrap()
            .strip_prefix("exo_session=")
            .unwrap()
            .to_string();
        let mut page = bare_request("/records");
        page.cookies.insert("exo_session".to_string(), sealed);
        match sso.gate(&page) {
            Gate::Authed(session) => assert_eq!(session.sub, "mock:citizen-1"),
            _ => panic!("expected Authed"),
        }

        // Callback with a garbage assertion -> 401 with the reason.
        let mut bad = bare_request("/auth/callback");
        bad.query
            .insert("assertion".to_string(), "not-a-token".to_string());
        match sso.gate(&bad) {
            Gate::Respond(r) => {
                assert_eq!(r.status, 401);
                assert!(r.body.starts_with("SSO assertion rejected: "));
            }
            _ => panic!("expected a 401 response"),
        }

        // Logout via GET is refused (F-19): state changes need POST.
        match sso.gate(&bare_request("/auth/logout")) {
            Gate::Respond(r) => assert_eq!(r.status, 405),
            _ => panic!("expected a 405 response"),
        }

        // Logout via POST -> 302 home, cookie cleared.
        let mut logout = bare_request("/auth/logout");
        logout.method = "POST".to_string();
        match sso.gate(&logout) {
            Gate::Respond(r) => {
                assert_eq!(r.status, 302);
                assert_eq!(r.headers[0].1, "/");
                assert!(r
                    .headers
                    .iter()
                    .any(|(k, v)| k == "Set-Cookie" && v.contains("Max-Age=0")));
            }
            _ => panic!("expected a redirect response"),
        }
    }

    #[test]
    fn assertion_without_iat_rejected() {
        // F-09 regression: a correctly-signed assertion that omits
        // `iat` must not slip past the lifetime cap.
        let sso = test_sso();
        let now = now_unix();
        let json = format!(
            "{{\"sub\":\"mock:citizen-1\",\"handle\":\"mock-citizen\",\"aud\":\"exocortexia\",\"ret\":\"/\",\"exp\":{},\"jti\":\"no-iat-1\"}}",
            now + 60
        );
        let token = sign_token(SSO_SECRET, &json);
        let err = sso
            .verify_assertion(&token)
            .expect_err("assertion without iat must be rejected");
        assert!(err.contains("iat"), "got: {}", err);
    }

    #[test]
    fn session_not_portable_across_audiences() {
        // F-10 regression: a session sealed by one app must not open
        // in another app, even with the same session secret.
        let one = test_sso();
        let mut cfg = one.cfg.clone();
        cfg.aud = "exocortexia:other-app".to_string();
        let other = Sso::new(cfg);
        let session = Session {
            sub: "mock:citizen-1".to_string(),
            handle: "mock-citizen".to_string(),
            exp: 0,
        };
        let sealed = one.seal_session(&session);
        assert!(one.open_session(&sealed).is_some());
        assert!(other.open_session(&sealed).is_none());
    }

    #[test]
    fn csrf_token_is_session_bound() {
        // F-11 support: tokens verify only for their own session.
        let sso = test_sso();
        let a = Session {
            sub: "user-a".to_string(),
            handle: "a".to_string(),
            exp: now_unix() + 3600,
        };
        let b = Session {
            sub: "user-b".to_string(),
            handle: "b".to_string(),
            exp: now_unix() + 3600,
        };
        let token = sso.csrf_token(&a);
        assert!(sso.csrf_ok(&a, &token));
        assert!(!sso.csrf_ok(&b, &token));
        assert!(!sso.csrf_ok(&a, "forged"));
    }

    #[test]
    fn config_debug_redacts_secrets() {
        // F-15 regression.
        let rendered = format!("{:?}", test_sso().cfg);
        assert!(!rendered.contains(SSO_SECRET));
        assert!(!rendered.contains(SESSION_SECRET));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn flat_json_parser_handles_claim_shapes() {
        let claims = parse_flat_json(
            "{\"sub\":\"a:b\",\"exp\":1893456000,\"admin\":true,\"note\":null,\"esc\":\"x\\\"y\"}",
        )
        .expect("flat claims must parse");
        assert_eq!(claims.get("sub").unwrap().as_str(), Some("a:b"));
        assert_eq!(claims.get("exp").unwrap().as_num(), Some(1893456000));
        assert_eq!(claims.get("admin"), Some(&JsonValue::Bool(true)));
        assert_eq!(claims.get("note"), Some(&JsonValue::Null));
        assert_eq!(claims.get("esc").unwrap().as_str(), Some("x\"y"));
        assert!(parse_flat_json("{\"a\":{\"nested\":1}}").is_err());
        assert!(parse_flat_json("not json").is_err());
    }
}
