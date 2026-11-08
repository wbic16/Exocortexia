# Build contract (all pieces MUST compile against this)

Workspace deps ONLY: libphext = "0.4.0", sha2 = "0.10", hmac = "0.12", base64 = "0.22". No serde, no tokio, no web framework. Std-only HTTP.

## crate exo-core (crates/core), lib name exo_core
- pub fn sha256_hex(data: &[u8]) -> String
- pub fn now_unix() -> i64 ; pub fn today_epoch_day() -> i64 (unix/86400)
- pub struct Store { map: BTreeMap<String,String> } with:
  - Store::new() ; Store::from_phext(text:&str)->Store (libphext explode; keys = Coordinate::to_string())
  - to_phext(&self)->String (libphext implode)
  - get(&self, coord:&str)->Option<&str> ; set(&mut self, coord:&str, text:&str) ; remove ; coords(&self)->Vec<String> (sorted) ; len()
  - save(&self, path:&Path)->io::Result<()> ; load(path)->io::Result<Store>
  - coord validation: pub fn valid_coord(s:&str)->bool (via libphext to_coordinate + validate_coordinate)
- pub struct AuditLog { entries: Vec<AuditEntry> } stored as scrolls in a Store at coords "1.1.1/1.1.1/2.1.<seq>":
  - AuditEntry { seq:u64, ts:i64, actor:String, action:String, coord:String, payload_sha256:String, prev_hash:String, hash:String }
  - line format inside scroll: "seq|ts|actor|action|coord|payload_sha256|prev_hash|hash" (fields must not contain '|'; sanitize by replacing '|' with '/')
  - AuditLog::new() ; open(store:&Store)->AuditLog ; append(&mut self, store:&mut Store, actor,action,coord,payload:&str)->AuditEntry ; verify(&self)->Result<(),String> (recompute chain; Err describes first bad seq)
  - hash = sha256_hex(format!("{prev_hash}|{seq}|{ts}|{actor}|{action}|{coord}|{payload_sha256}"))
- Redaction: pub struct Redaction { target:String, start:usize, end:usize, category:String, reason:String, id:String }
  - stored in a Store at coords "1.1.1/1.1.1/3.1.<n>", scroll text lines: target=, start=, end=, category=, reason=
  - pub fn load_redactions(store:&Store)->Vec<Redaction> ; pub fn add_redaction(store:&mut Store, r:&Redaction)
  - pub fn apply_redactions(text:&str, rs:&[Redaction])->String : replaces byte ranges [start,end) in the target text with "[REDACTED {category}]", applied in descending start order; MUST NOT mutate input.
  - categories: pub const REDACTION_CATEGORIES: &[&str] — the Neb. Rev. Stat. 84-712.05 subsection labels (use short labels like "84-712.05(3) personnel", keep a list of ~8 plausible ones as &str constants).
- mini HTTP (module exo_core::http):
  - pub struct Request { method:String, path:String, query:HashMap<String,String>, headers:HashMap<String,String> (lowercased keys), body:String, cookies:HashMap<String,String> }
  - pub struct Response { status:u16, content_type:String, body:String, headers:Vec<(String,String)> }
  - Response::html(body) / ::text(body) / ::json(body) / ::redirect(location) ; .with_header(k,v) ; .set_cookie(value_string)
  - pub fn serve(port:u16, handler: impl Fn(&Request)->Response + Send + Sync + 'static) -> io::Result<()> : thread-per-connection TcpListener, parses request line/headers/content-length, url-decodes query + form body.
  - pub fn form(req:&Request)->HashMap<String,String> (parse urlencoded body)
  - pub fn esc(s:&str)->String (HTML escape)
  - pub fn url_decode / url_encode
- pub struct Rng(u64) : deterministic LCG; fn next(&mut self)->u64 ; fn below(&mut self, n:u64)->u64

## crate exo-sso (crates/sso), lib name exo_sso, depends on exo-core
- pub struct SsoConfig { edit_sso_url:String, sso_secret:String, session_secret:String, aud:String }
  - SsoConfig::from_env()->Option<SsoConfig> : None unless EDIT_SSO_URL, EXO_SSO_SECRET, EXO_SESSION_SECRET all set & secrets >= 32 chars. aud: env EXO_SSO_AUD or default "exocortexia".
- pub struct Session { sub:String, handle:String, exp:i64 }
- pub enum Gate { PassThrough, Authed(Session), Respond(Response) }
- pub struct Sso { cfg } with:
  - Sso::new(cfg)
  - gate(&self, req:&Request)->Gate : routes:
    - path "/auth/callback": read query "assertion"; verify -> success: Respond(302 to safe ret path from claim "ret" default "/") with Set-Cookie exo_session=<sealed>; failure: Respond(401 text "SSO assertion rejected: <why>")
    - path "/auth/login": Respond(302 to {edit_sso_url}?return={url_encode(req query "return" or "/")}) — if edit_sso_url already contains '?', use '&'.
    - path "/auth/logout" (POST only; GET is 405): Respond(302 "/") clearing cookie.
    - otherwise: valid exo_session cookie -> Authed; else Respond(302 "/auth/login?return=<current path>") for GET pages; still 302 (demo simplicity) — document.
  - verify_assertion(&self, token:&str)->Result<Session,String> : format b64url(json).b64url(hmac); HMAC-SHA256 with sso_secret over the b64url(json) part; constant-time compare (compare decoded bytes with xor fold). JSON flat parse (write tiny parser): claims sub (string, required), aud (must == cfg.aud), exp (number, required, now<exp), iat (number; if present require exp-iat<=120 and iat<=now+30), jti (string, required; replay cache Mutex<HashMap<String,i64>> swept by exp; duplicate -> Err "replay").
  - mint_assertion(secret, sub, handle, aud, ret, ttl_secs)->String : pub, used by the minter bin and tests.
  - seal_session / open_session with session_secret, session exp = now + 12*3600. Claims: sub, handle, exp.
  - banner_html(&self)->&str : when SSO configured, small "SSO: edit.phext.io" note; pass-through banner is produced by apps when config is None: "AUTH DISABLED — local demo mode (SSO not configured)".
- bin edit-minter (crates/sso/src/bin/edit-minter.rs): env MOCK_SSO_SECRET (or EXO_SSO_SECRET), APP_CALLBACK (full URL e.g. http://127.0.0.1:8801/auth/callback), MOCK_PORT default 8799, EXO_SSO_AUD default "exocortexia". Serves: any GET path: reads query "return" (default "/"), mints assertion sub="mock:citizen-1", handle="mock-citizen", ret=return, ttl 120, 302 -> {APP_CALLBACK}?assertion={token}. Also GET /health -> "ok".

## Apps: each a binary crate, deps: exo-core, exo-sso, libphext workspace deps as needed.
Common pattern in main.rs:
  port from env PORT or app default; build demo state (seeded, deterministic, in-code via exo_seed if needed or local seed fns); let sso = SsoConfig::from_env().map(Sso::new);
  exo_core::http::serve(port, move |req| { if let Some(s)=&sso { match s.gate(req) { Gate::Respond(r)=>return r, Gate::Authed(sess)=> return route(req, Some(sess)), Gate::PassThrough=>{} } } route(req, None) })
  Auth rule: when SSO configured, ALL routes require Authed EXCEPT gate-handled /auth/*. When not configured, everything open + banner on HTML pages.
  HTML: semantic (header/main/nav/footer, labels for every input, lang="en", title, sufficient contrast: dark text on white).
Every app: tests/ integration tests may live in src #[cfg(test)] or tests/ dir; MUST include unit tests for its core logic functions (not HTTP).
Every app: README.md (what, statute, run command, routes, data note) .

Ports: records-desk 8801, minutes-publisher 8802, contract-ledger 8803, data-catalog 8804, forms-router 8805.

## crate exo-seed (seed/), lib name exo_seed + bin seedgen
- pub fn records_corpus(n:usize)->Store : n synthetic public-records docs at coords "1.1.1/1.1.1/1.<i/1000+1>.<i%1000+1>" — hmm keep coords simple: use format "1.1.1/1.1.1/{}.{}.{}" no. Use shelf..: coord string = format!("1.1.1/1.1.1/1.1.{}", i+1) is only 3rd group varying third component — valid. Docs: memos/emails about public topics (bridge maintenance, InterRAI public meeting notes etc.), some containing the token "InterRAI", deterministic via Rng(42).
- pub fn minutes_corpus(boards:usize, years:usize)->Store
- pub fn contracts_corpus(n:usize, today_day:i64)->Store
- pub fn catalog_corpus()->Store
- bin seedgen: writes seed/out/records-100k.phext (100_000 docs), minutes.phext (500 boards x 10 years x 4 meetings), contracts-10k.phext; prints byte sizes + sha256 (use exo_core::sha256_hex).

## tools/loadsim (bin loadsim), deps exo-core, exo-seed
- mode http: args --base URL --path /x --concurrency C --requests N --p95-budget-ms B ; std threads, each does TcpStream GETs; collect latencies; print: requests, errors, error_rate, p50/p95/p99 ms; exit 1 if errors>0 or p95>budget (when budget given).
- mode store: args --docs N ; builds records_corpus(N), times: serialize, parse(explode), 1000 searches (substring "InterRAI") timed, audit chain build+verify for 1000 entries, redaction apply x1000. Prints timings. Exit 1 if audit verify fails or a mutation control (flip a byte in a copy of audit store, re-verify) does NOT fail — i.e. asserts the negative control correctly.
- arg parsing: hand-rolled from std::env::args.

## Conventions
- Deterministic seeds everywhere (Rng with fixed seed) so runs are reproducible.
- Dates: apps use epoch-day integers in data; render as day counts or ISO via a small civil-date conversion in exo-core: pub fn iso_date(epoch_day:i64)->String (implement civil-from-days algorithm).
