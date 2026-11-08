# SSO: signing in to Exocortexia with edit.phext.io

Exocortexia apps do not keep their own user database or passwords. They
trust [edit.phext.io](https://edit.phext.io) as the single sign-on (SSO)
authority, using the same consumer-side pattern proven by Mutable
(mutable.phext.io): edit.phext.io mints a short-lived signed assertion, the
app verifies it locally, and the app then mints its **own** session cookie.
Edit's session cookie is never accepted, stored, or forwarded by an
Exocortexia app.

The gate lives in `crates/sso` (`exo_sso::Sso`); every app runs every
request through it before routing.

## Flow

1. An unauthenticated `GET` is redirected to `/auth/login?return=<path>`.
2. `/auth/login` redirects to the edit-side SSO endpoint:
   `{EDIT_SSO_URL}?return=<url-encoded return path>` (using `&` instead of
   `?` if `EDIT_SSO_URL` already contains a query string). The user signs
   in at edit.phext.io there.
3. Edit redirects the browser back to the app's `/auth/callback` with a
   single query parameter, `assertion`.
4. The app verifies the assertion (below). On success it responds `302` to
   the assertion's safe return path (`ret` claim, default `/`) and sets its
   own `exo_session` cookie. On failure it responds
   `401 SSO assertion rejected: <why>`.
5. Later requests present `exo_session`; the gate opens it locally and lets
   the request through as that session's user. `/auth/logout` clears the
   cookie and redirects to `/`.

When SSO is configured, **all** routes require an authenticated session
except the gate-handled `/auth/*` paths themselves.

## Assertion format

```text
assertion = b64url(JSON claims) "." b64url(HMAC-SHA256)
```

- Base64url without padding, both parts.
- The signature is `HMAC-SHA256(key = EXO_SSO_SECRET, message = the
  b64url(JSON) part exactly as received)` — the signature covers the
  encoded first part, not the decoded JSON.
- Verification recomputes the HMAC and compares the decoded signature
  bytes in constant time (XOR fold over all bytes).

### Claims

| Claim | Type | Rule |
|---|---|---|
| `sub` | string | Required. The user's stable subject id at edit.phext.io. |
| `aud` | string | Must equal the app's audience: `exocortexia` (see `EXO_SSO_AUD`). Assertions minted for any other audience are rejected. |
| `exp` | number (unix seconds) | Required. The assertion must not be expired (`now < exp`). |
| `iat` | number (unix seconds) | If present: total lifetime must be at most 120 s (`exp - iat <= 120`) and `iat` must not be more than 30 s in the future (`iat <= now + 30`). |
| `jti` | string | Required. Unique assertion id; see replay protection. |
| `handle` | string | Optional display handle, carried into the app session. |
| `ret` | string | Optional return path the app redirects to after a successful callback. The app only honours safe local paths. |

### Lifetime and replay protection

Assertions live for **120 seconds** at most. Each app keeps an in-memory
replay cache (`Mutex<HashMap<jti, exp>>`, swept by expiry): the first use
of a `jti` is recorded, and any second presentation of the same assertion
is rejected with `replay`. An assertion is therefore single-use — a leaked
callback URL cannot be replayed, and after 120 s it is expired anyway.
The cache is per-process and resets on restart, which is safe because the
120 s expiry bounds the window regardless.

## The app session cookie

After a successful callback the app mints its **own** session, sealed with
`EXO_SESSION_SECRET` (a secret only the app knows — never shared with
edit):

- Cookie name: `exo_session`
- Value: the same `b64url(JSON).b64url(HMAC-SHA256)` construction, keyed
  with `EXO_SESSION_SECRET`, carrying claims `sub`, `handle`, `exp`.
- Lifetime: 12 hours (`exp = now + 12 * 3600`).
- Attributes: `HttpOnly`, `SameSite=Lax`, host-only (no `Domain`
  attribute, so the cookie is never sent to any other host, including
  edit.phext.io).

## Configuration

| Variable | Required | Meaning |
|---|---|---|
| `EDIT_SSO_URL` | yes | Edit-side SSO endpoint the app redirects sign-ins to. |
| `EXO_SSO_SECRET` | yes | Shared secret used to verify edit's assertion signatures. Must be ≥ 32 characters. |
| `EXO_SESSION_SECRET` | yes | This app's own session-sealing secret. Must be ≥ 32 characters. |
| `EXO_SSO_AUD` | no | Expected assertion audience. Default: `exocortexia`. |

SSO counts as configured only when `EDIT_SSO_URL`, `EXO_SSO_SECRET`, and
`EXO_SESSION_SECRET` are **all** set and both secrets are at least 32
characters (`SsoConfig::from_env()` returns `None` otherwise).

### Pass-through behaviour (SSO not configured)

If SSO is not configured, the gate is disabled entirely: every route is
open without sign-in, and HTML pages carry the banner
**"AUTH DISABLED — local demo mode (SSO not configured)"**. This exists so
the suite can be run and evaluated locally with zero setup. It is a demo
mode only — never run a public deployment unconfigured. When SSO *is*
configured, pages instead show a small "SSO: edit.phext.io" note.

## Local testing with edit-minter

`crates/sso` ships an `edit-minter` binary that stands in for the edit
side: it mints assertions with a mock secret and redirects to the app's
callback, so the full flow can be smoke-tested on one machine.

```sh
# Terminal 1 — the mock edit side (port 8799 by default).
# MOCK_SSO_SECRET must match the app's EXO_SSO_SECRET.
MOCK_SSO_SECRET="dev-only-secret-that-is-at-least-32-chars" \
APP_CALLBACK="http://127.0.0.1:8801/auth/callback" \
cargo run -p exo-sso --bin edit-minter

# Terminal 2 — an app, configured to trust the minter.
EDIT_SSO_URL="http://127.0.0.1:8799/login" \
EXO_SSO_SECRET="dev-only-secret-that-is-at-least-32-chars" \
EXO_SESSION_SECRET="a-different-dev-secret-also-32-chars!" \
cargo run -p records-desk

# Terminal 3 — walk the flow.
curl -i http://127.0.0.1:8801/                 # 302 -> /auth/login?return=/
curl -i "http://127.0.0.1:8801/auth/login"     # 302 -> minter ?return=...
# Follow the minter URL in a browser (or with curl -L and a cookie jar):
# the minter 302s to /auth/callback?assertion=..., the app verifies it,
# sets exo_session, and 302s to the return path.
curl -s http://127.0.0.1:8799/health           # "ok" — minter liveness check
```

The minter mints assertions for subject `mock:citizen-1`, handle
`mock-citizen`, ttl 120 s, audience `EXO_SSO_AUD` (default `exocortexia`).
Its port can be changed with `MOCK_PORT` (default 8799), and it also
accepts `EXO_SSO_SECRET` in place of `MOCK_SSO_SECRET`.

## Production notes

- **Edit side must mint for this audience.** Production sign-in requires
  the edit.phext.io / phext-lattice side to expose the SSO endpoint and
  mint assertions with `aud = "exocortexia"` signed with the shared
  `EXO_SSO_SECRET`. Wiring that endpoint up on the phext-lattice side is
  Will's deploy step; until it lands, production apps should stay in
  pass-through demo mode or behind the minter pattern on a private
  network.
- **Secrets are ≥ 32 characters**, generated randomly per deployment, and
  are never committed to the repository.
- **`EXO_SSO_SECRET` must never equal edit's `PHEXT_SESSION_SECRET`.** The
  assertion secret is a dedicated, separately rotatable credential shared
  only between edit's minter and the consuming apps; reusing edit's own
  session secret would let any app that learns it forge edit sessions.
  Likewise `EXO_SESSION_SECRET` is unique to each deployment and is never
  shared with edit at all.
- Rotate `EXO_SSO_SECRET` on both sides together; rotating it invalidates
  outstanding assertions only (≤ 120 s), not app sessions. Rotating
  `EXO_SESSION_SECRET` logs everyone out of that app.
- Assertions and sessions travel only over HTTPS in production; the
  `HttpOnly` + `SameSite=Lax` cookie and the 120 s single-use assertion
  assume TLS terminates in front of the app.
