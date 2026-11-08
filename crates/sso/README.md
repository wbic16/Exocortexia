# exo-sso

A small, shared SSO gate for the Exocortexia demo apps, built on
`exo-core`'s std-only HTTP types. Demo plumbing — a shared-secret HMAC
scheme standing in for edit.phext.io sign-in, not a general OIDC
client.

## Protocol

1. Unauthenticated requests 302 to `/auth/login`, which 302s to
   `{EDIT_SSO_URL}?return=<path>`.
2. The identity side redirects back to `/auth/callback?assertion=…`
   where the assertion is
   `b64url(json) "." b64url(HMAC-SHA256(EXO_SSO_SECRET, b64url(json)))`.
3. `verify_assertion` checks, in order: signature (constant-time
   compare), `aud` equals the app audience, `exp` in the future,
   `exp − iat ≤ 120s` and `iat` not in the future, and `jti` not seen
   before (in-memory replay cache, swept by expiry).
4. On success the gate seals a 12-hour session — same wire format,
   signed with the independent `EXO_SESSION_SECRET` — into the
   `exo_session` cookie (`HttpOnly; SameSite=Lax; Path=/`) and 302s to
   the assertion's `ret` claim after safe-path validation (must start
   with `/`, no `//`, no `:`, no backslash).
5. `/auth/logout` clears the cookie and 302s to `/`.

## Configuration

`SsoConfig::from_env()` returns `None` (apps run open, showing the
"AUTH DISABLED — local demo mode" banner) unless all of these are set
and both secrets are ≥ 32 bytes:

- `EDIT_SSO_URL`, `EXO_SSO_SECRET`, `EXO_SESSION_SECRET`
- optional `EXO_SSO_AUD` (default `exocortexia`)

## Mock identity side

`edit-minter` stands in for edit.phext.io locally: it mints assertions
for a fixed mock citizen (`mock:citizen-1` / `mock-citizen`). Anyone
who can reach it can log in as that citizen — local demos only.

```sh
MOCK_SSO_SECRET=<same as app EXO_SSO_SECRET> \
APP_CALLBACK=http://127.0.0.1:8801/auth/callback \
MOCK_PORT=8799 cargo run -p exo-sso --bin edit-minter
# GET /health -> ok
```

## Tests

`cargo test -p exo-sso` — mint→verify happy path, and rejection of:
wrong secret, expired assertion, replayed `jti`, wrong audience, and
`exp − iat > 120`. Plus session seal/open, safe-path rules, the flat
JSON parser, and end-to-end gate flows.
