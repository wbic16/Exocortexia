# Security — audit, pen-test, and fix pass

- **Target:** this repository at `exo` HEAD `9660d3b` (the pre-fix tree).
- **Date:** 2026-11-07 (America/Chicago).
- **Scope:** `crates/core` (phext store, hash-chained audit log,
  redaction layer, hand-rolled std HTTP server), `crates/sso` (SSO
  assertion gate, sessions, mock minter), and the five apps
  (records-desk, minutes-publisher, contract-ledger, data-catalog,
  forms-router), plus `tools/loadsim` and `seed/`.
- **Method:** (1) full manual code audit of the crates and every app
  handler; (2) a live pen-test battery against the release binaries
  running on a ranch node, in both open (demo) mode and SSO mode with
  the mock minter — SSO replay/tamper/expiry/audience cases, session
  fixation, CSRF, stored/reflected XSS, traversal, malformed and
  oversized HTTP, redaction bypass, tracking-code derivation,
  state-machine bypass, SSRF probes (loopback and link-local targets
  only; no external site was touched); (3) fixes with a regression
  test per fix; (4) re-verification: full test suite, the repo's
  `scripts/verify.sh`, and a 49-case live re-check battery against the
  fixed binaries (49/49 passed, plus 4 fail-closed startup cases).
- **Finding IDs:** `A-F##` are audit findings, `P-F##` are pen-test
  findings. Where both lanes found the same defect, one row carries
  both IDs.

## Findings and status

| ID | Severity | Finding | Status |
|---|---|---|---|
| A-F01 / P-F01 | Critical (on public exposure) | records-desk rendered the **unredacted original** to every viewer of `/doc`; no custodian role existed | **Fixed** — original renders only for an authenticated staff session (`EXO_STAFF_SUBS`); never in demo mode |
| A-F03 / P-F02 | High | Search matched over originals; snippets/subjects/export leaked redacted bytes (boolean oracle + verbatim ±60-char snippets + original byte counts) | **Fixed** — search, snippets, subjects, and export worksheet bytes all operate on the redacted public view only |
| A-F02 | High | Auth **failed open**: missing/short SSO config silently disabled the gate; all apps bound `0.0.0.0` | **Fixed** — config is fail-closed (partial/invalid config or short secrets are startup errors; running open requires explicit `EXO_DEMO_MODE=1`); listener binds `127.0.0.1` unless `EXO_BIND` says otherwise |
| A-F05 | High | `edit-minter` shipped in the production crate, fell back to the real `EXO_SSO_SECRET`, and minted valid assertions for any unauthenticated caller | **Fixed** — the binary builds only with `--features mock-minter` (verified absent from a default workspace build); it requires `MOCK_SSO_SECRET` and never reads `EXO_SSO_SECRET` |
| A-F06 / P-F10 | High | Unbounded `Content-Length` allocation and unbounded header reading, parsed *before* auth: unauthenticated process-killing DoS | **Fixed** — 16 KiB head budget, 64-header cap, 1 MiB body cap, all rejected before allocation |
| A-F04 | High | Audit chain could not detect **tail truncation** (no length/head commitment) and, being unkeyed, could be fully recomputed by a store writer | **Fixed with operator step** — keyed chain available via `EXO_AUDIT_KEY`; malformed audit scrolls now fail verification instead of being silently skipped; `head()`/`head_commitment()`/`verify_with_head()` provide an anchor, and minutes-publisher's proof page prints the commitment and states exactly what the chain proves. Anchoring the commitment outside the store remains the operator's job |
| A-F07 / P-F04 (part) | Medium | forms-router staff actions unguarded: anyone could `POST /api/transition` any submission; `/api/export.csv` dumped all submissions and tracking codes | **Fixed** — with SSO configured, transitions and export require a staff identity (`EXO_STAFF_SUBS`), failing closed when unset |
| A-F17 / P-F04 (part) | Medium | Tracking code was 40-bit, deterministic, derived from sequential `seq` + the requester's own low-entropy field values — offline-derivable, and `/track` discloses all fields | **Fixed** — codes are now 128-bit CSPRNG values (`/dev/urandom`), stored with the submission; the deterministic derivation remains only as a content checksum |
| A-F09 / P-F05 | Medium | `iat` optional → the 120 s assertion lifetime cap was void for assertions omitting it (live-confirmed: a no-`iat`, 1-hour assertion was accepted) | **Fixed** — `iat` required; `exp − now` capped independently |
| A-F11 / P-F03 | Medium | No CSRF defence on any state-changing POST (tokens or Origin checks); only incidental `SameSite=Lax` | **Fixed** — per-session CSRF tokens on session-backed forms (records redact, forms submit) and staff actions; Origin/Host match enforced on every POST incl. demo mode and data-catalog check-links |
| A-F12 | Medium | Audit `actor` was a free-text form field; session identity never recorded | **Fixed** — actor is taken from the session; the form field is gone; demo mode records `demo-mode (unauthenticated)` |
| A-F13 | Medium | No security response headers anywhere | **Fixed** — baseline set emitted centrally: CSP `default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'`, `X-Content-Type-Options: nosniff`, `X-Frame-Options: DENY`, `Referrer-Policy: no-referrer` |
| A-F08 / P-F06 | Medium | Session cookie had no `Secure` flag (12 h token, no revocation) | **Fixed** — `Secure` on by default; `EXO_COOKIE_SECURE=0` is the documented opt-out for plain-HTTP demos off localhost |
| A-F10 | Medium | Shared default audience + per-process replay caches → one assertion redeemable once per app; sessions portable across apps sharing a session secret | **Fixed** — default audience is per-app (`exocortexia:<app>`); sessions carry and check the sealing app's `aud`. Residual: the replay cache is still per-process (bounded by the 120 s assertion lifetime). Docs corrected (SSO.md) |
| A-F16 | Medium | Unbounded thread-per-connection, per-read (not total) timeouts, no write timeout; data-catalog link check fans out synchronously | **Partially fixed** — connections bounded at 128, 15 s write timeout added, handler panics caught. Residual: timeouts remain per-read-operation; link checks remain synchronous over a fixed 8-dataset seed set |
| A-F21 / P-§10 | Low | SSRF-shaped link checker: fetched arbitrary `http://` URLs from store data with no address filtering (latent — URLs are compiled-in seed values; the pen test confirmed no user input can set a URL today) | **Fixed** — `check_url` refuses when *any* resolved address is private, link-local (incl. 169.254.169.254), CGNAT, multicast, or unspecified; loopback stays allowed by design (the demo's own datasets point there) |
| A-F23 / P-F09 | Low–Med | Duplicate `Content-Length` last-wins, `Transfer-Encoding` ignored, negative CL silently 0, bare-LF lines accepted — request-desync primitives behind a proxy | **Fixed** — duplicates and any `Transfer-Encoding` rejected, unparsable/oversized CL rejected, CRLF line endings required |
| A-F14 | Low | Assertion travels in a URL query (history/logs); rejection messages disclosed the expected audience | **Partially fixed** — client-facing rejections are now generic (detail goes to the server log). The query-string transport is the edit.phext.io protocol shape; mitigations are the 120 s lifetime and single-use `jti`. Accepted risk |
| A-F15 | Low | `SsoConfig` derived `Debug` over both raw secrets (latent log leak; no live leak existed) | **Fixed** — manual `Debug` with redacted fields, regression-tested |
| A-F18 / P-F08 | Low | CSV formula injection in forms export (leading `= + - @` not neutralised); no server-side required-field validation on submit | **Fixed** — cells with those leading characters are `'`-prefixed; empty-field submissions are refused with 400 |
| A-F19 | Low | Logout was a state-changing GET | **Fixed** — POST only; GET returns 405 |
| A-F20 | Low | Response headers emitted verbatim — response splitting latent in the framework (no user-controlled path reached it) | **Fixed** — header names/values containing CR/LF are dropped in `write_response` |
| A-F22 | Low | Mutex-poison cascade (`lock().unwrap()` everywhere); handler panics dropped connections silently | **Fixed** — locks recover via `into_inner()`; the HTTP layer catches handler panics and answers 500 |
| P-F07 | Low | Unescaped reflection of request input in `text/plain` 404 bodies (records, minutes) | **Fixed** — echoed values are HTML-escaped even in text bodies; `nosniff` now also set |
| P-F11 | Med–Low | All state was memory-only: restart silently erased redactions, submissions, and the audit trail | **Fixed for the mutating apps** — records-desk and forms-router persist their redaction/submission/audit stores as phext files under `EXO_STATE_DIR` and reload them at startup (verified across a restart). minutes/contract/catalog hold only deterministic seed data and mutate nothing at runtime — accepted as-is |
| — | Info | `cargo audit` not run: not installed on the build node; install failed on a toolchain conflict (node rustc 1.94.0 < `kstring 2.0.5`'s required 1.96.0). Manual review instead: the whole workspace resolves to ~10 third-party crates (libphext 0.4.0, sha2 0.10.9, hmac 0.12.1, base64 0.22.1, socket2 0.5.10 + RustCrypto/platform transitives), all current release lines | **Open follow-up** — `cargo install cargo-audit --locked`, or run on rustc ≥ 1.96 |
| — | Info (verified sound) | Constant-time HMAC compare; signature verified before payload parsing; aud/exp/future-`iat` checks; assertion/session secret separation (both directions tested); `safe_ret_path`; session-fixation resistance; systematic HTML/JSON escaping (pen test found **no** exploitable XSS, stored or reflected, in HTML contexts); no static-file serving exists (no traversal surface); no network-reachable phext parsing; forms state machine (no skips/backwards/replays; rejected transitions write nothing); **no real secrets anywhere in the tree** (only labelled test fixtures) | — |

## Pen-test battery — before / after

Confirmed before the fix pass (live evidence in the pen-test
report): original-bytes disclosure (F-01), search-oracle leak (F-02),
CSRF on redact/submit/transition in both modes (P-F03),
tracking-code derivation + bulk disclosure (P-F04), no-`iat`
lifetime bypass (P-F05), cookie without `Secure` (P-F06), 404
reflection (P-F07), CSV injection (P-F08), duplicate-CL last-wins
(P-F09), unbounded header/body allocation (P-F10), state loss on
restart (P-F11).

Re-check after fixes (same style of battery, run 2026-11-07 against
the fixed release binaries; 49 cases, **49 passed**):

- **Demo mode (26 cases):** security headers present on responses;
  evil-Origin redact/submit/check-links all `403`; legitimate redact
  `200`; redacted phrase absent from doc page, search, and export;
  no custodian section for non-staff; duplicate CL, 50 GB CL, bare-LF,
  1 MB header, and `Transfer-Encoding` all `400` with the server
  healthy afterwards; 404 reflection escaped; tracking code 128-bit
  (old derivation does not reproduce it); CSV cell exported as
  `'=2+5`; empty submission `400`.
- **Persistence (3 cases):** after a restart with the same
  `EXO_STATE_DIR`, the redaction still applies, the search oracle
  stays closed, and a pre-restart tracking code still resolves.
- **SSO mode (20 cases):** full minter→callback→session flow works;
  cookie carries `Secure`; replay `401`; no-`iat` assertion `401`;
  wrong-audience `401` with a generic body (no expected-audience
  leak); tokenless and evil-Origin redacts `403`; redact with the
  form's CSRF token `200`; non-staff session sees no custodian view;
  staff session (`EXO_STAFF_SUBS`) does; `/health` reachable
  unauthenticated; logout GET `405`, POST `302` with cookie cleared.
- **Fail-closed startup (4 cases):** no SSO env and no
  `EXO_DEMO_MODE` → refuses to start; partial config → refuses;
  short secrets → refuses; minter with only `EXO_SSO_SECRET` in its
  environment → exits demanding `MOCK_SSO_SECRET`.
- **Bind posture:** all apps now listen on `127.0.0.1` by default
  (`ss -ltn` verified), previously `0.0.0.0`.

## Re-verification numbers

- `cargo test --workspace`: **84 passed, 0 failed** (69 before the
  pass; +15 regression tests: 3 audit-chain, 4 HTTP parsing/origin,
  4 SSO, 1 records search-oracle, 2 forms, 1 data-catalog SSRF).
- `scripts/verify.sh`: **OK** — release build, full tests, seed
  generation (SHA-256 of all three seed corpora unchanged from the
  pre-pass receipts), 100k-doc store benchmarks, audit tamper
  negative control `detected`.
- Default workspace build verified to contain **no** `edit-minter`
  binary; `--features mock-minter` build produces it.

## New operator surface (from this pass)

| Variable | Meaning |
|---|---|
| `EXO_DEMO_MODE=1` | Explicit opt-in to run without SSO (open demo). Without it, unconfigured apps refuse to start. |
| `EXO_BIND` | Listen address for all apps (default `127.0.0.1`; `0.0.0.0` only behind a TLS-terminating proxy / trusted network). |
| `EXO_STAFF_SUBS` | Comma-separated staff `sub`/`handle` values for custodian view, forms transitions, CSV export. |
| `EXO_COOKIE_SECURE=0` | Omit the session cookie `Secure` attribute (plain-HTTP demo off localhost). |
| `EXO_AUDIT_KEY` | Server-only key for the audit hash chain (records/minutes/forms). Keep it out of the state store. |
| `EXO_STATE_DIR` | Directory where records-desk and forms-router persist their mutable stores as phext files. |
| `EXO_SSO_AUD` | Now defaults per-app; set explicitly when the identity side mints a shared audience. Existing deployments that relied on the shared `exocortexia` default must set `EXO_SSO_AUD=exocortexia` (and mint accordingly) or adopt per-app audiences. |

## Residual risks — stated plainly

- **The hand-rolled HTTP server is demo infrastructure, not a
  production web server.** It now has request caps, strict framing,
  a connection bound, and baseline headers — but no TLS (terminate
  TLS in front), no rate limiting, per-read rather than total
  request deadlines, and thread-per-connection concurrency. Do not
  expose it to the open internet as-is.
- **Demo mode is still total when explicitly enabled.**
  `EXO_DEMO_MODE=1` + `EXO_BIND=0.0.0.0` reproduces the old
  wide-open posture by operator choice; the pass changed the
  *defaults* and removed the *silent* failure, not the possibility.
- **Sessions are stateless.** There is no server-side revocation:
  logout clears the browser cookie, but a captured `exo_session`
  value works until its 12 h expiry. `Secure` + `HttpOnly` +
  short assertion lifetimes bound, but do not eliminate, this.
- **Assertions travel in URL query strings** (the edit.phext.io
  protocol shape), so they land in browser history and proxy/server
  logs. The 120 s lifetime and single-use `jti` are the mitigation.
- **Audit-chain anchoring is an operator duty.** Keyed chains
  (`EXO_AUDIT_KEY`) stop store-writers recomputing history, and
  `verify_with_head` detects truncation — but only against a head
  commitment actually pinned somewhere the store writer cannot
  reach. Nothing in the suite publishes it externally yet.
- **No rate limiting anywhere.** `/track` relies on the 128-bit
  code space; login redirects and link checks rely on the small
  scale of the deployment.
- **The SSO scheme is shared-secret HMAC demo plumbing**, not an
  OIDC client: no key rotation, no issuer discovery, no asymmetric
  signatures. Suitable for the edit.phext.io first-party setup it
  was built for; not a general identity solution.
